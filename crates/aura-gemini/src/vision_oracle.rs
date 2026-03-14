//! Vision Oracle: uses Gemini 3 Flash to extract precise click coordinates
//! from full-resolution screenshots when the Accessibility API cannot find
//! the target element.

use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

const DEFAULT_MODEL: &str = "gemini-3-flash-preview";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
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
        }
    }

    /// Send a full-resolution screenshot to Gemini 3 Flash and get back
    /// precise logical coordinates for the click target nearest (hint_x, hint_y).
    ///
    /// - `screenshot_b64`: base64-encoded JPEG from CapturedFrame.jpeg_base64
    /// - `hint_x`, `hint_y`: Gemini Live's approximate logical coords
    /// - `img_w`, `img_h`: the JPEG image dimensions (pixels)
    /// - `screen_w`, `screen_h`: logical screen dimensions (macOS points)
    ///
    /// Returns `Ok((logical_x, logical_y))` on success.
    pub async fn find_element_coordinates(
        &self,
        screenshot_b64: &str,
        hint_x: f64,
        hint_y: f64,
        img_w: u32,
        img_h: u32,
        screen_w: u32,
        screen_h: u32,
    ) -> Result<(f64, f64)> {
        // Convert logical hint coords to image-space pixels for the prompt
        let img_hint_x = (hint_x / screen_w as f64 * img_w as f64) as u32;
        let img_hint_y = (hint_y / screen_h as f64 * img_h as f64) as u32;

        let prompt = format!(
            "You are a UI element locator. Given a screenshot, find the clickable UI element \
             nearest to pixel ({}, {}) in this {}x{} image. \
             Return ONLY the center point as [y, x] normalized to 0-1000. No other text.",
            img_hint_x, img_hint_y, img_w, img_h
        );

        let body = serde_json::json!({
            "contents": [{
                "parts": [
                    { "text": prompt },
                    { "inline_data": { "mime_type": "image/jpeg", "data": screenshot_b64 } }
                ]
            }],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 50
            }
        });

        let url = format!(
            "{}/{}:generateContent?key={}",
            REST_BASE, self.model, self.api_key
        );

        let start = std::time::Instant::now();
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Vision oracle HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Vision oracle got HTTP {status}: {err_body}");
        }

        let json: Value = resp
            .json()
            .await
            .context("Vision oracle failed to parse response JSON")?;

        // Extract text from response: candidates[0].content.parts[0].text
        let text = json
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|v| v.as_str())
            .context("Vision oracle response missing text field")?;

        let elapsed = start.elapsed();

        let (norm_y, norm_x) = parse_normalized_coords(text)
            .context(format!("Vision oracle returned unparseable coords: {text:?}"))?;

        // Note: denormalize takes (norm_x, norm_y) — swap from Gemini's [y,x] to standard (x,y)
        let (logical_x, logical_y) = denormalize(norm_x, norm_y, screen_w, screen_h);

        tracing::info!(
            hint_x,
            hint_y,
            norm_y,
            norm_x,
            logical_x,
            logical_y,
            elapsed_ms = elapsed.as_millis() as u64,
            "Vision oracle returned coordinates"
        );

        Ok((logical_x, logical_y))
    }
}

/// Parse normalized [y, x] coordinates from Gemini Flash response text.
/// Returns (norm_y, norm_x) both in 0.0..=1000.0 range, or None on failure.
fn parse_normalized_coords(text: &str) -> Option<(f64, f64)> {
    // Strategy 1: Try to find bracketed [y, x] pattern first (most reliable)
    if let Some(start) = text.find('[') {
        if let Some(end) = text[start..].find(']') {
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
}
