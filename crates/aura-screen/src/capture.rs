use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use core_graphics::display::CGDisplay;
use image::codecs::jpeg::JpegEncoder;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Maximum width to send to Gemini (downscale retina captures).
const MAX_WIDTH: u32 = 1920;
/// JPEG quality (0-100). 60 balances readability vs bandwidth.
const JPEG_QUALITY: u8 = 60;

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

/// Capture the active display (display under mouse cursor) as a JPEG-encoded base64 string.
pub fn capture_screen() -> Result<CapturedFrame> {
    let display_id = active_display_id().unwrap_or(CGDisplay::main().id);
    let display = CGDisplay::new(display_id);
    let cg_image = CGDisplay::image(&display)
        .context("Failed to capture screen — is Screen Recording permission granted?")?;

    let width = cg_image.width();
    let height = cg_image.height();
    let bytes_per_row = cg_image.bytes_per_row();
    let data = cg_image.data();
    let raw_bytes = data.bytes();

    // Convert BGRA to RGB
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row_start = y * bytes_per_row;
        for x in 0..width {
            let px = row_start + x * 4;
            rgb.push(raw_bytes[px + 2]); // R
            rgb.push(raw_bytes[px + 1]); // G
            rgb.push(raw_bytes[px]);     // B
        }
    }

    // Simple hash for change detection (sample pixels)
    let hash = compute_frame_hash(&rgb);

    // Downscale if wider than MAX_WIDTH
    let (final_rgb, final_w, final_h) = if width as u32 > MAX_WIDTH {
        let scale = MAX_WIDTH as f64 / width as f64;
        let new_h = (height as f64 * scale) as u32;
        let img = image::RgbImage::from_raw(width as u32, height as u32, rgb)
            .context("Failed to create image buffer")?;
        let resized = image::imageops::resize(
            &img,
            MAX_WIDTH,
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
    let mut encoder = JpegEncoder::new_with_quality(&mut jpeg_buf, JPEG_QUALITY);
    encoder
        .encode(&final_rgb, final_w, final_h, image::ExtendedColorType::Rgb8)
        .context("JPEG encoding failed")?;

    let jpeg_base64 = BASE64.encode(&jpeg_buf);

    Ok(CapturedFrame { jpeg_base64, hash, width: final_w, height: final_h })
}

/// Compute an FNV-1a hash by sampling 2048 pixels across the frame.
fn compute_frame_hash(rgb: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let step = (rgb.len() / 2048).max(1);
    for i in (0..rgb.len()).step_by(step) {
        hash ^= rgb[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_detects_small_changes() {
        let mut frame1 = vec![128u8; 1920 * 1080 * 3];
        let frame2 = frame1.clone();
        frame1[1920 * 540 * 3 + 960 * 3] = 129;
        let h1 = compute_frame_hash(&frame1);
        let h2 = compute_frame_hash(&frame2);
        assert_ne!(h1, h2, "Hash should detect single-pixel change");
    }
}

/// Handle for triggering immediate captures (post-action).
#[derive(Clone)]
pub struct CaptureTrigger {
    flag: Arc<AtomicBool>,
}

impl CaptureTrigger {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Signal that an immediate capture should happen.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Check and clear the trigger flag.
    pub fn check_and_clear(&self) -> bool {
        self.flag.swap(false, Ordering::Relaxed)
    }
}
