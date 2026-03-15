//! Vision Oracle: uses Gemini 3 Flash to extract precise click coordinates
//! from full-resolution screenshots when the Accessibility API cannot find
//! the target element.

use anyhow::{Context, Result};
use serde_json::Value;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_MODEL: &str = "gemini-3-flash-preview";
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
const CIRCUIT_BREAKER_COOLDOWN_SECS: u64 = 30;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
const REST_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Denormalize Gemini's 0-1000 coordinates to logical screen pixels.
fn denormalize(norm_x: f64, norm_y: f64, screen_w: u32, screen_h: u32) -> (f64, f64) {
    let lx = (norm_x / 1000.0) * screen_w as f64;
    let ly = (norm_y / 1000.0) * screen_h as f64;
    (lx, ly)
}

pub struct VisionOracle {
    client: reqwest::Client,
    api_key: String,
    model: String,
    consecutive_failures: AtomicU32,
    tripped_until: AtomicU64, // epoch millis
}

impl VisionOracle {
    pub fn new(api_key: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("failed to build reqwest client");
        Self {
            client,
            api_key: api_key.to_string(),
            model: DEFAULT_MODEL.to_string(),
            consecutive_failures: AtomicU32::new(0),
            tripped_until: AtomicU64::new(0),
        }
    }

    /// Send a full-resolution screenshot to Gemini 3 Flash and get back
    /// precise logical coordinates for the click target nearest (hint_x, hint_y).
    ///
    /// - `screenshot_b64`: base64-encoded JPEG from CapturedFrame.jpeg_base64
    /// - `hint_x`, `hint_y`: Gemini Live's approximate logical coords
    /// - `img_w`, `img_h`: the JPEG image dimensions (pixels)
    /// - `screen_w`, `screen_h`: logical screen dimensions (macOS points)
    /// - `target`: optional description of the UI element to find
    /// - `display_origin_x`, `display_origin_y`: global display origin for multi-monitor support
    ///
    /// Returns `Ok(Some((logical_x, logical_y)))` on success, `Ok(None)` if target not found.
    #[allow(clippy::too_many_arguments)]
    pub async fn find_element_coordinates(
        &self,
        screenshot_b64: &str,
        hint_x: f64,
        hint_y: f64,
        _img_w: u32,
        _img_h: u32,
        screen_w: u32,
        screen_h: u32,
        target: Option<&str>,
        display_origin_x: f64,
        display_origin_y: f64,
    ) -> Result<Option<(f64, f64)>> {
        if screen_w == 0 || screen_h == 0 {
            anyhow::bail!("Invalid screen dimensions: {}x{}", screen_w, screen_h);
        }

        // Normalize hint coords to 0-1000 (same coordinate system as oracle output)
        let norm_hint_x = (hint_x / screen_w as f64 * 1000.0) as u32;
        let norm_hint_y = (hint_y / screen_h as f64 * 1000.0) as u32;

        let prompt = match target {
            Some(desc) => format!(
                "You are a precise UI click targeting system for macOS.\n\n\
                 Target: {desc}\n\
                 Hint: approximately [{norm_hint_y}, {norm_hint_x}] (0-1000 normalized)\n\n\
                 Rules:\n\
                 - Find the element matching the target description, not just the nearest element to the hint\n\
                 - Return the CENTER of the element, not an edge\n\
                 - If multiple elements match, prefer the one closest to the hint\n\
                 - If the target is on a canvas or content area (not a UI control), return the hint unchanged\n\
                 - If the target is not visible or is covered by another element, return [-1, -1]\n\
                 - Return ONLY [y, x] normalized to 0-1000. No other text."
            ),
            None => format!(
                "You are a precise UI click targeting system for macOS.\n\n\
                 Hint: approximately [{norm_hint_y}, {norm_hint_x}] (0-1000 normalized)\n\n\
                 Find the nearest clickable UI element to the hint coordinates.\n\
                 If no clickable element is visible near the hint, return [-1, -1].\n\
                 Return ONLY the center point as [y, x] normalized to 0-1000. No other text."
            ),
        };

        let body = serde_json::json!({
            "contents": [{
                "parts": [
                    { "text": prompt },
                    {
                        "inline_data": { "mime_type": "image/jpeg", "data": screenshot_b64 }
                    }
                ]
            }],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 100,
                "mediaResolution": "MEDIA_RESOLUTION_ULTRA_HIGH"
            }
        });

        let url = format!("{}/{}:generateContent", REST_BASE, self.model);

        let start = std::time::Instant::now();
        let resp: reqwest::Response = self
            .client
            .post(&url)
            .query(&[("key", &self.api_key)])
            .json(&body)
            .send()
            .await
            .context("Vision oracle HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let err_body: String = resp.text().await.unwrap_or_default();
            let err_body_safe = if err_body.len() > 200 {
                &err_body[..200]
            } else {
                &err_body
            };
            anyhow::bail!("Vision oracle got HTTP {status}: {err_body_safe}");
        }

        let json: Value = resp
            .json()
            .await
            .context("Vision oracle failed to parse response JSON")?;

        // Extract text from response: candidates[0].content.parts[0].text
        let text = json
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|v: &Value| v.as_str())
            .context("Vision oracle response missing text field")?;

        let elapsed = start.elapsed();

        // Check for NotFound sentinel BEFORE parsing coordinates
        if is_not_found_sentinel(text) {
            tracing::info!(
                elapsed_ms = elapsed.as_millis() as u64,
                "Vision oracle: target not visible (returned [-1, -1])"
            );
            return Ok(None);
        }

        let (norm_y, norm_x) = parse_normalized_coords(text).context(format!(
            "Vision oracle returned unparseable coords: {text:?}"
        ))?;

        let (local_x, local_y) = denormalize(norm_x, norm_y, screen_w, screen_h);
        let global_x = local_x + display_origin_x;
        let global_y = local_y + display_origin_y;

        tracing::info!(
            hint_x,
            hint_y,
            norm_y,
            norm_x,
            logical_x = global_x,
            logical_y = global_y,
            elapsed_ms = elapsed.as_millis() as u64,
            "Vision oracle returned coordinates"
        );

        Ok(Some((global_x, global_y)))
    }

    pub fn is_available(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now >= self.tripped_until.load(Ordering::Acquire)
    }

    pub fn record_failure(&self) {
        let prev = self.consecutive_failures.fetch_add(1, Ordering::AcqRel);
        if prev + 1 >= CIRCUIT_BREAKER_THRESHOLD {
            let trip_until = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
                + CIRCUIT_BREAKER_COOLDOWN_SECS * 1000;
            self.tripped_until.store(trip_until, Ordering::Release);
            tracing::warn!(
                cooldown_secs = CIRCUIT_BREAKER_COOLDOWN_SECS,
                "Vision oracle circuit breaker tripped"
            );
        }
    }

    pub fn record_success(&self) {
        let prev = self.consecutive_failures.swap(0, Ordering::AcqRel);
        if prev >= CIRCUIT_BREAKER_THRESHOLD {
            tracing::info!("Vision oracle circuit breaker recovered");
        }
    }
}

/// Parse normalized [y, x] coordinates from Gemini Flash response text.
/// Returns (norm_y, norm_x) both in 0.0..=1000.0 range, or None on failure.
fn parse_normalized_coords(text: &str) -> Option<(f64, f64)> {
    // Strategy 1: Try to find bracketed [y, x] pattern first (most reliable)
    if let Some(start) = text.find('[')
        && let Some(end) = text[start..].find(']')
    {
        let inner = &text[start + 1..start + end];
        let nums: Vec<f64> = inner
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        if nums.len() >= 2 {
            return validate_coords(nums[0], nums[1]);
        }
    }

    // Strategy 2: Fallback — find two comma-separated numbers anywhere
    let numbers: Vec<f64> = text
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .filter(|n| (0.0..=1000.0).contains(n))
        .collect();

    if numbers.len() >= 2 {
        let y = numbers[numbers.len() - 2];
        let x = numbers[numbers.len() - 1];
        return validate_coords(y, x);
    }

    None
}

/// Check if oracle returned the "not found" sentinel [-1, -1]
fn is_not_found_sentinel(text: &str) -> bool {
    if let Some(start) = text.find('[') {
        if let Some(end) = text[start..].find(']') {
            let inner = &text[start + 1..start + end];
            let nums: Vec<f64> = inner
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<f64>().ok())
                .collect();
            if nums.len() >= 2 && nums[0] == -1.0 && nums[1] == -1.0 {
                return true;
            }
        }
    }
    false
}

fn validate_coords(y: f64, x: f64) -> Option<(f64, f64)> {
    if y == 0.0 && x == 0.0 {
        return None;
    }
    if !(0.0..=1000.0).contains(&y) || !(0.0..=1000.0).contains(&x) {
        return None;
    }
    Some((y, x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bracketed_coords() {
        assert_eq!(parse_normalized_coords("[456, 723]"), Some((456.0, 723.0)));
    }

    #[test]
    fn parse_no_brackets() {
        assert_eq!(parse_normalized_coords("456, 723"), Some((456.0, 723.0)));
    }

    #[test]
    fn parse_no_spaces() {
        assert_eq!(parse_normalized_coords("[456,723]"), Some((456.0, 723.0)));
    }

    #[test]
    fn parse_with_preamble_text() {
        let text = "The center of the button is at [456, 723].";
        assert_eq!(parse_normalized_coords(text), Some((456.0, 723.0)));
    }

    #[test]
    fn reject_zero_zero() {
        assert_eq!(parse_normalized_coords("[0, 0]"), None);
    }

    #[test]
    fn reject_out_of_range() {
        assert_eq!(parse_normalized_coords("[1500, 723]"), None);
    }

    #[test]
    fn reject_garbage() {
        assert_eq!(parse_normalized_coords("no coordinates here"), None);
    }

    #[test]
    fn reject_single_number() {
        assert_eq!(parse_normalized_coords("[456]"), None);
    }

    #[test]
    fn parse_preamble_with_numbers() {
        let text = "Element 3 is 5px wide. The center is at [456, 723].";
        assert_eq!(parse_normalized_coords(text), Some((456.0, 723.0)));
    }

    #[test]
    fn denormalize_center() {
        // (500, 500) on 1920x1080 screen → center pixel
        let (lx, ly) = denormalize(500.0, 500.0, 1920, 1080);
        assert!((lx - 960.0).abs() < 0.1);
        assert!((ly - 540.0).abs() < 0.1);
    }

    #[test]
    fn denormalize_top_left() {
        let (lx, ly) = denormalize(0.0, 0.0, 1920, 1080);
        assert!((lx - 0.0).abs() < 0.1);
        assert!((ly - 0.0).abs() < 0.1);
    }

    #[test]
    fn denormalize_bottom_right() {
        let (lx, ly) = denormalize(1000.0, 1000.0, 1920, 1080);
        assert!((lx - 1920.0).abs() < 0.1);
        assert!((ly - 1080.0).abs() < 0.1);
    }

    #[test]
    fn parse_decimal_coords() {
        assert_eq!(
            parse_normalized_coords("[456.5, 723.2]"),
            Some((456.5, 723.2))
        );
    }

    #[test]
    fn parse_zero_y_nonzero_x() {
        assert_eq!(parse_normalized_coords("[0, 500]"), Some((0.0, 500.0)));
    }

    #[test]
    fn parse_nonzero_y_zero_x() {
        assert_eq!(parse_normalized_coords("[500, 0]"), Some((500.0, 0.0)));
    }

    #[test]
    fn parse_max_boundary() {
        assert_eq!(
            parse_normalized_coords("[1000, 1000]"),
            Some((1000.0, 1000.0))
        );
    }

    #[test]
    fn parse_first_bracket_pair_wins() {
        let text = "First [100, 200] then [456, 723]";
        assert_eq!(parse_normalized_coords(text), Some((100.0, 200.0)));
    }

    #[test]
    fn reject_empty_brackets() {
        assert_eq!(parse_normalized_coords("[]"), None);
    }

    #[test]
    fn reject_whitespace_brackets() {
        assert_eq!(parse_normalized_coords("[  ,  ]"), None);
    }

    #[test]
    fn not_found_sentinel_detected() {
        assert!(is_not_found_sentinel("[-1, -1]"));
        assert!(is_not_found_sentinel("The target is not visible [-1, -1]"));
    }

    #[test]
    fn not_found_sentinel_rejects_valid_coords() {
        assert!(!is_not_found_sentinel("[456, 723]"));
        assert!(!is_not_found_sentinel("no coordinates"));
        assert!(!is_not_found_sentinel("[-1, 500]")); // only one -1
    }

    #[test]
    fn normalize_hint_coords() {
        let norm_x = (960.0_f64 / 1920.0 * 1000.0) as u32;
        let norm_y = (540.0_f64 / 1080.0 * 1000.0) as u32;
        assert_eq!(norm_x, 500);
        assert_eq!(norm_y, 500);
    }

    #[test]
    fn circuit_breaker_initially_available() {
        let oracle = VisionOracle::new("fake-key");
        assert!(oracle.is_available());
    }

    #[test]
    fn circuit_breaker_trips_after_threshold() {
        let oracle = VisionOracle::new("fake-key");
        oracle.record_failure();
        oracle.record_failure();
        oracle.record_failure();
        assert!(!oracle.is_available());
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let oracle = VisionOracle::new("fake-key");
        oracle.record_failure();
        oracle.record_failure();
        oracle.record_success();
        assert!(oracle.is_available());
        oracle.record_failure();
        assert!(oracle.is_available()); // only 1 failure, not 3
    }
}
