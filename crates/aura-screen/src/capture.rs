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
}

/// Capture the main display as a JPEG-encoded base64 string.
pub fn capture_screen() -> Result<CapturedFrame> {
    let display = CGDisplay::main();
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

    Ok(CapturedFrame { jpeg_base64, hash })
}

/// Compute a simple hash by sampling pixels across the frame.
fn compute_frame_hash(rgb: &[u8]) -> u64 {
    let mut hash: u64 = 0;
    let step = (rgb.len() / 256).max(1);
    for i in (0..rgb.len()).step_by(step) {
        hash = hash.wrapping_mul(31).wrapping_add(rgb[i] as u64);
    }
    hash
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
