use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use core_graphics::display::CGDisplay;
use image::codecs::jpeg::JpegEncoder;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum width to send to Gemini (downscale retina captures).
const MAX_WIDTH: u32 = 1920;
/// JPEG quality (0-100). 80 balances readability vs bandwidth.
const JPEG_QUALITY: u8 = 80;

/// Maximum width for on-demand high-res captures (get_screen_context, SoM, tool verification).
const ONDEMAND_MAX_WIDTH: u32 = 2560;
/// JPEG quality for on-demand captures. 92 gives crisp text without excessive size.
const ONDEMAND_JPEG_QUALITY: u8 = 92;

/// A captured frame ready to send to Gemini.
pub struct CapturedFrame {
    /// Base64-encoded JPEG data.
    pub jpeg_base64: String,
    /// Simple hash for change detection.
    pub hash: u64,
    /// Width of the JPEG image (after downscale).
    pub width: u32,
    /// Height of the JPEG image (after downscale).
    pub height: u32,
    /// Retina scale factor (e.g. 2.0 on Retina displays).
    /// Ratio of raw pixel width to logical display width.
    pub scale_factor: f64,
    /// Logical display width in macOS points (used for coordinate mapping).
    pub logical_width: u32,
    /// Logical display height in macOS points (used for coordinate mapping).
    pub logical_height: u32,
}

unsafe extern "C" {
    fn CGGetDisplaysWithPoint(
        point: core_graphics::geometry::CGPoint,
        max_displays: u32,
        displays: *mut u32,
        count: *mut u32,
    ) -> i32;
}

/// Get the display ID containing the mouse cursor.
fn active_display_id() -> Option<u32> {
    // SAFETY: CGGetDisplaysWithPoint is a CoreGraphics C API. We pass valid mutable
    // pointers to stack-allocated u32 values with max_displays=1, so the function
    // writes at most one display ID. The event source and mouse event are created
    // via safe Rust wrappers and remain valid for the duration of the call.
    unsafe {
        let mut display_id: u32 = 0;
        let mut count: u32 = 0;
        let event_source = core_graphics::event_source::CGEventSource::new(
            core_graphics::event_source::CGEventSourceStateID::CombinedSessionState,
        )
        .ok()?;
        let mouse_event = core_graphics::event::CGEvent::new(event_source).ok()?;
        let point = mouse_event.location();
        let result = CGGetDisplaysWithPoint(point, 1, &mut display_id, &mut count);
        if result == 0 && count > 0 {
            Some(display_id)
        } else {
            None
        }
    }
}

/// Capture the active display at streaming quality (1920px, Q80).
/// Used by the 2 FPS capture loop.
pub fn capture_screen() -> Result<CapturedFrame> {
    capture_screen_with_params(MAX_WIDTH, JPEG_QUALITY)
}

/// Capture the active display at high resolution (2560px, Q92).
/// Used for on-demand captures: get_screen_context, SoM annotation,
/// tool-triggered verification frames. Costs ~2x more tokens per frame
/// but provides 33% more detail for coordinate targeting.
pub fn capture_screen_high_res() -> Result<CapturedFrame> {
    capture_screen_with_params(ONDEMAND_MAX_WIDTH, ONDEMAND_JPEG_QUALITY)
}

/// Internal capture with configurable resolution and quality.
fn capture_screen_with_params(max_width: u32, jpeg_quality: u8) -> Result<CapturedFrame> {
    let display_id = active_display_id().unwrap_or(CGDisplay::main().id);
    let display = CGDisplay::new(display_id);
    let cg_image = CGDisplay::image(&display)
        .context("Failed to capture screen — is Screen Recording permission granted?")?;

    let width = cg_image.width();
    let height = cg_image.height();

    // Compute retina scale factor: raw pixel width / logical display width.
    // CGDisplay::image() returns retina (physical) pixels; bounds() returns logical points.
    let raw_width = width as f64;
    let display_bounds = display.bounds();
    let logical_width = display_bounds.size.width;
    let scale_factor = if logical_width > 0.0 {
        raw_width / logical_width
    } else {
        1.0
    };
    let bytes_per_row = cg_image.bytes_per_row();
    let data = cg_image.data();
    let raw_bytes = data.bytes();

    anyhow::ensure!(
        raw_bytes.len() >= height * bytes_per_row,
        "Screen capture buffer too small: {} < {}",
        raw_bytes.len(),
        height * bytes_per_row,
    );

    // Convert BGRA to RGB
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row_start = y * bytes_per_row;
        for x in 0..width {
            let px = row_start + x * 4;
            rgb.push(raw_bytes[px + 2]); // R
            rgb.push(raw_bytes[px + 1]); // G
            rgb.push(raw_bytes[px]); // B
        }
    }

    // Simple hash for change detection (sample pixels)
    let hash = compute_frame_hash(&rgb);

    // Downscale if wider than max_width
    let (final_rgb, final_w, final_h) = if width as u32 > max_width {
        let scale = max_width as f64 / width as f64;
        let new_h = (height as f64 * scale) as u32;
        let img = image::RgbImage::from_raw(width as u32, height as u32, rgb)
            .context("Failed to create image buffer")?;
        let resized = image::imageops::resize(
            &img,
            max_width,
            new_h,
            image::imageops::FilterType::Triangle,
        );
        let w = resized.width();
        let h = resized.height();
        (resized.into_raw(), w, h)
    } else {
        (rgb, width as u32, height as u32)
    };

    // Encode to JPEG
    let mut jpeg_buf = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_buf, jpeg_quality);
    encoder
        .encode(&final_rgb, final_w, final_h, image::ExtendedColorType::Rgb8)
        .context("JPEG encoding failed")?;

    let jpeg_base64 = BASE64.encode(&jpeg_buf);

    let logical_w = display_bounds.size.width as u32;
    let logical_h = display_bounds.size.height as u32;

    Ok(CapturedFrame {
        jpeg_base64,
        hash,
        width: final_w,
        height: final_h,
        scale_factor,
        logical_width: logical_w,
        logical_height: logical_h,
    })
}

/// Compute an FNV-1a hash by sampling 8192 pixels across the frame.
fn compute_frame_hash(rgb: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let step = (rgb.len() / 8192).max(1);
    for i in (0..rgb.len()).step_by(step) {
        hash ^= rgb[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    hash
}

/// Check if a captured frame looks censored (blank/uniform content).
/// macOS returns a valid image even without Screen Recording permission,
/// but window contents are blanked out — only wallpaper and window chrome remain.
/// This samples pixels and checks color variance: a real screen has high variance,
/// a censored one is mostly uniform.
pub fn frame_looks_censored(rgb: &[u8], width: usize, height: usize) -> bool {
    if rgb.len() < width * height * 3 || width == 0 || height == 0 {
        return true;
    }

    // Sample ~256 pixels spread across the frame
    let total_pixels = width * height;
    let step = (total_pixels / 256).max(1);
    let mut unique_colors: std::collections::HashSet<(u8, u8, u8)> =
        std::collections::HashSet::new();

    for i in (0..total_pixels).step_by(step) {
        let offset = i * 3;
        if offset + 2 < rgb.len() {
            unique_colors.insert((rgb[offset], rgb[offset + 1], rgb[offset + 2]));
        }
        // Real screens have many unique colors; bail early if clearly not censored
        if unique_colors.len() > 32 {
            return false;
        }
    }

    // A real screen with apps open typically has 100+ unique sampled colors.
    // Censored captures (wallpaper-only) may have few colors, but a solid-color
    // wallpaper could also have <32. Use a very conservative threshold.
    unique_colors.len() <= 8
}

/// Generate a SoM-annotated version of a captured frame.
/// Returns the annotated JPEG (base64) and the mark positions.
/// Called on-demand when Gemini requests visual element targeting.
pub fn annotate_with_som(jpeg_bytes: &[u8]) -> Option<(String, Vec<crate::som::SomMark>)> {
    let img = image::load_from_memory(jpeg_bytes).ok()?;
    let (annotated, marks) = crate::som::annotate_frame(&img);

    // Encode annotated image back to JPEG
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, ONDEMAND_JPEG_QUALITY);
    annotated.write_with_encoder(encoder).ok()?;

    Some((BASE64.encode(&buf), marks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_detects_small_changes() {
        let size = 1920 * 1080 * 3;
        let mut frame1 = vec![128u8; size];
        let frame2 = frame1.clone();
        // Change a pixel at a sampled position (step-aligned)
        let step = (size / 8192).max(1);
        frame1[step * 4096] = 129; // middle of the sampled range
        let h1 = compute_frame_hash(&frame1);
        let h2 = compute_frame_hash(&frame2);
        assert_ne!(h1, h2, "Hash should detect single-pixel change");
    }

    #[test]
    fn trigger_sets_flag() {
        let trigger = CaptureTrigger::new();
        trigger.trigger();
        assert!(
            trigger.flag.load(Ordering::Relaxed),
            "trigger() should set the flag"
        );
    }

    #[test]
    fn check_and_clear_returns_true_and_clears() {
        let trigger = CaptureTrigger::new();
        trigger.trigger();
        assert!(
            trigger.check_and_clear(),
            "check_and_clear() should return true after trigger()"
        );
        assert!(
            !trigger.flag.load(Ordering::Relaxed),
            "flag should be cleared after check_and_clear()"
        );
    }

    #[test]
    fn check_and_clear_returns_false_when_not_triggered() {
        let trigger = CaptureTrigger::new();
        assert!(
            !trigger.check_and_clear(),
            "check_and_clear() should return false when not triggered"
        );
    }

    #[test]
    fn multiple_triggers_before_check_produce_one_true() {
        let trigger = CaptureTrigger::new();
        trigger.trigger();
        trigger.trigger();
        trigger.trigger();
        assert!(
            trigger.check_and_clear(),
            "first check_and_clear() should return true"
        );
        assert!(
            !trigger.check_and_clear(),
            "second check_and_clear() should return false"
        );
    }

    #[test]
    fn default_creates_untriggered_instance() {
        let trigger = CaptureTrigger::default();
        assert!(
            !trigger.check_and_clear(),
            "default CaptureTrigger should not be triggered"
        );
    }

    #[test]
    fn clone_shares_flag() {
        let trigger = CaptureTrigger::new();
        let cloned = trigger.clone();
        trigger.trigger();
        assert!(
            cloned.check_and_clear(),
            "cloned trigger should see the original's trigger"
        );
    }

    #[test]
    fn trigger_and_wait_sets_flag_and_provides_receiver() {
        let trigger = CaptureTrigger::new();
        let mut rx = trigger.trigger_and_wait();
        assert!(
            trigger.flag.load(Ordering::Relaxed),
            "trigger_and_wait() should set the flag"
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn trigger_and_wait_receiver_completes_on_send() {
        let trigger = CaptureTrigger::new();
        let mut rx = trigger.trigger_and_wait();
        if let Some(tx) = trigger.take_waiter() {
            let _ = tx.send(());
        }
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn take_waiter_returns_none_when_no_pending() {
        let trigger = CaptureTrigger::new();
        trigger.trigger(); // regular trigger, no waiter
        assert!(trigger.take_waiter().is_none());
    }

    #[test]
    fn high_res_constants_are_larger_than_streaming() {
        const {
            assert!(
                ONDEMAND_MAX_WIDTH > MAX_WIDTH,
                "On-demand should be higher res than streaming"
            );
        }
        const {
            assert!(
                ONDEMAND_JPEG_QUALITY > JPEG_QUALITY,
                "On-demand should be higher quality than streaming"
            );
        }
        const {
            assert!(
                ONDEMAND_MAX_WIDTH <= 2560,
                "On-demand should not exceed 2560px"
            );
        }
    }

    #[test]
    fn capture_screen_high_res_exists() {
        // Verify the function signature compiles — actual capture requires
        // Screen Recording permission and will fail in CI.
        let _fn_ptr: fn() -> Result<CapturedFrame> = capture_screen_high_res;
    }
}

/// Handle for triggering immediate captures (post-action).
#[derive(Clone)]
pub struct CaptureTrigger {
    flag: Arc<AtomicBool>,
    waiter: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

impl Default for CaptureTrigger {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureTrigger {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            waiter: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Signal that an immediate capture should happen.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::Release);
    }

    /// Signal that an immediate capture should happen and return a receiver
    /// that completes when the capture loop has delivered the frame.
    pub fn trigger_and_wait(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Ok(mut guard) = self.waiter.lock() {
            if guard.is_some() {
                tracing::warn!(
                    "trigger_and_wait: overwriting pending waiter (previous caller will get RecvError)"
                );
            }
            *guard = Some(tx);
        }
        self.flag.store(true, Ordering::Release);
        rx
    }

    /// Check and clear the trigger flag.
    pub fn check_and_clear(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }

    /// Take the pending waiter sender, if any. Called by the capture loop
    /// after delivering the frame.
    pub fn take_waiter(&self) -> Option<tokio::sync::oneshot::Sender<()>> {
        self.waiter.lock().ok()?.take()
    }
}
