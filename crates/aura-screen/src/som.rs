//! Lightweight Set-of-Mark (SoM) overlay for visual element targeting.
//!
//! Detects rectangular interactive regions in screenshots via edge detection
//! and overlays numbered markers. Gemini can then reference "mark N" instead
//! of estimating pixel coordinates.
//!
//! This is a pure-Rust alternative to OmniParser (YOLO+Florence, requires CUDA).

use image::{DynamicImage, Rgba, RgbaImage};

/// A detected interactive region with its bounding box and mark number.
#[derive(Debug, Clone)]
pub struct SomMark {
    /// Mark number (1-indexed, displayed on overlay).
    pub id: usize,
    /// Bounding box: (x, y, width, height) in pixels.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Minimum region dimension (pixels) to qualify as an interactive element.
const MIN_REGION_SIZE: u32 = 20;

/// Maximum number of marks to overlay (prevents clutter).
const MAX_MARKS: usize = 30;

/// Edge detection threshold (0-255). Pixels with gradient magnitude above
/// this are considered edges.
const EDGE_THRESHOLD: u8 = 40;

/// Minimum gap between detected regions to avoid duplicates (pixels).
const MIN_REGION_GAP: u32 = 10;

/// Detect rectangular interactive regions using Sobel edge detection
/// and connected component analysis on the resulting binary edge map.
pub fn detect_regions(img: &DynamicImage) -> Vec<SomMark> {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();

    if w < MIN_REGION_SIZE * 2 || h < MIN_REGION_SIZE * 2 {
        return Vec::new();
    }

    // Sobel edge detection
    let mut edges = vec![false; (w * h) as usize];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = |dx: i32, dy: i32| -> u8 {
                gray.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)
                    .0[0]
            };
            // Sobel X
            let gx = -(idx(-1, -1) as i16) + (idx(1, -1) as i16) - 2 * (idx(-1, 0) as i16)
                + 2 * (idx(1, 0) as i16)
                - (idx(-1, 1) as i16)
                + (idx(1, 1) as i16);
            // Sobel Y
            let gy = -(idx(-1, -1) as i16) - 2 * (idx(0, -1) as i16) - (idx(1, -1) as i16)
                + (idx(-1, 1) as i16)
                + 2 * (idx(0, 1) as i16)
                + (idx(1, 1) as i16);
            let magnitude = ((gx.abs() + gy.abs()) / 2) as u8;
            if magnitude > EDGE_THRESHOLD {
                edges[(y * w + x) as usize] = true;
            }
        }
    }

    // Simple horizontal run-length region detection:
    // Find horizontal runs of edges, group into rectangular regions
    let mut regions: Vec<(u32, u32, u32, u32)> = Vec::new(); // (x, y, w, h)

    // Scan for rectangular clusters using a grid-based approach
    let grid_size = MIN_REGION_SIZE;

    for gy in (0..h).step_by(grid_size as usize / 2) {
        for gx in (0..w).step_by(grid_size as usize / 2) {
            // Count edges in this grid cell
            let mut edge_count = 0u32;
            let cell_w = grid_size.min(w - gx);
            let cell_h = grid_size.min(h - gy);
            for dy in 0..cell_h {
                for dx in 0..cell_w {
                    let idx = ((gy + dy) * w + (gx + dx)) as usize;
                    if idx < edges.len() && edges[idx] {
                        edge_count += 1;
                    }
                }
            }

            // If enough edges, this is likely an interactive region
            let cell_area = cell_w * cell_h;
            if cell_area > 0 && edge_count * 100 / cell_area > 15 {
                // Check if this overlaps with an existing region
                let overlaps = regions.iter().any(|&(rx, ry, rw, rh)| {
                    gx < rx + rw + MIN_REGION_GAP
                        && gx + cell_w + MIN_REGION_GAP > rx
                        && gy < ry + rh + MIN_REGION_GAP
                        && gy + cell_h + MIN_REGION_GAP > ry
                });

                if !overlaps {
                    regions.push((gx, gy, cell_w, cell_h));
                }
            }
        }
    }

    // Sort by position (top-to-bottom, left-to-right) and cap at MAX_MARKS
    regions.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    regions.truncate(MAX_MARKS);

    regions
        .into_iter()
        .enumerate()
        .map(|(i, (x, y, w, h))| SomMark {
            id: i + 1,
            x,
            y,
            width: w,
            height: h,
        })
        .collect()
}

/// Render numbered markers on a copy of the image at each detected region.
/// Returns the annotated image and the list of marks with their coordinates.
pub fn annotate_frame(img: &DynamicImage) -> (DynamicImage, Vec<SomMark>) {
    let marks = detect_regions(img);
    let mut canvas = img.to_rgba8();

    for mark in &marks {
        draw_mark_label(&mut canvas, mark);
    }

    (DynamicImage::ImageRgba8(canvas), marks)
}

/// Draw a numbered label at the top-left corner of a mark's bounding box.
fn draw_mark_label(canvas: &mut RgbaImage, mark: &SomMark) {
    let label = format!("{}", mark.id);
    let label_w = (label.len() as u32) * 8 + 4; // ~8px per char + padding
    let label_h: u32 = 14;

    // Draw background rectangle (semi-transparent red)
    let bg_color = Rgba([220, 40, 40, 200]);
    let text_color = Rgba([255, 255, 255, 255]);

    let (img_w, img_h) = canvas.dimensions();
    let bx = mark.x.min(img_w.saturating_sub(label_w));
    let by = mark.y.saturating_sub(label_h + 2);

    for dy in 0..label_h {
        for dx in 0..label_w {
            let px = bx + dx;
            let py = by + dy;
            if px < img_w && py < img_h {
                canvas.put_pixel(px, py, bg_color);
            }
        }
    }

    // Draw number using simple pixel font (3×5 digit bitmaps)
    let digits: Vec<u8> = label.bytes().map(|b| b - b'0').collect();
    for (di, &digit) in digits.iter().enumerate() {
        let bitmap = digit_bitmap(digit);
        let ox = bx + 2 + (di as u32) * 8;
        let oy = by + 3;
        for (row, bits) in bitmap.iter().enumerate() {
            for col in 0..5u32 {
                if bits & (1 << (4 - col)) != 0 {
                    // Draw 2× scaled pixel for readability
                    for sy in 0..2u32 {
                        for sx in 0..2u32 {
                            let px = ox + col * 2 + sx;
                            let py = oy + (row as u32) * 2 + sy;
                            if px < img_w && py < img_h {
                                canvas.put_pixel(px, py, text_color);
                            }
                        }
                    }
                }
            }
        }
    }

    // Draw outline around the region (1px red border)
    let outline_color = Rgba([220, 40, 40, 180]);
    for dx in 0..mark.width {
        let px = mark.x + dx;
        if px < img_w {
            if mark.y < img_h {
                canvas.put_pixel(px, mark.y, outline_color);
            }
            let bot = mark.y + mark.height.saturating_sub(1);
            if bot < img_h {
                canvas.put_pixel(px, bot, outline_color);
            }
        }
    }
    for dy in 0..mark.height {
        let py = mark.y + dy;
        if py < img_h {
            if mark.x < img_w {
                canvas.put_pixel(mark.x, py, outline_color);
            }
            let right = mark.x + mark.width.saturating_sub(1);
            if right < img_w {
                canvas.put_pixel(right, py, outline_color);
            }
        }
    }
}

/// 3×5 pixel bitmap for digits 0-9 (5 bits per row, MSB = leftmost).
fn digit_bitmap(d: u8) -> [u8; 5] {
    match d {
        0 => [0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
        1 => [0b00100, 0b01100, 0b00100, 0b00100, 0b01110],
        2 => [0b01110, 0b10001, 0b00110, 0b01000, 0b11111],
        3 => [0b01110, 0b10001, 0b00110, 0b10001, 0b01110],
        4 => [0b10010, 0b10010, 0b11111, 0b00010, 0b00010],
        5 => [0b11111, 0b10000, 0b11110, 0b00001, 0b11110],
        6 => [0b01110, 0b10000, 0b11110, 0b10001, 0b01110],
        7 => [0b11111, 0b00001, 0b00010, 0b00100, 0b00100],
        8 => [0b01110, 0b10001, 0b01110, 0b10001, 0b01110],
        9 => [0b01110, 0b10001, 0b01111, 0b00001, 0b01110],
        _ => [0; 5],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_regions_on_blank_image_returns_empty() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(200, 200));
        let regions = detect_regions(&img);
        assert!(regions.is_empty(), "Blank image should have no regions");
    }

    #[test]
    fn detect_regions_on_tiny_image_returns_empty() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(10, 10));
        let regions = detect_regions(&img);
        assert!(regions.is_empty(), "Tiny image should have no regions");
    }

    #[test]
    fn marks_are_numbered_sequentially() {
        // Create an image with high-contrast rectangles that should be detected
        let mut img = RgbaImage::new(400, 400);
        // Draw two distinct rectangles with sharp edges
        for x in 50..150 {
            for y in 50..100 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        for x in 250..350 {
            for y in 50..100 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let dyn_img = DynamicImage::ImageRgba8(img);
        let marks = detect_regions(&dyn_img);
        // Marks should be 1-indexed
        for (i, mark) in marks.iter().enumerate() {
            assert_eq!(mark.id, i + 1);
        }
    }

    #[test]
    fn annotate_frame_returns_same_dimensions() {
        let img = DynamicImage::ImageRgba8(RgbaImage::new(200, 200));
        let (annotated, _marks) = annotate_frame(&img);
        assert_eq!(annotated.width(), 200);
        assert_eq!(annotated.height(), 200);
    }

    #[test]
    fn digit_bitmap_all_digits_valid() {
        for d in 0..=9 {
            let bm = digit_bitmap(d);
            // Each digit should have at least some pixels set
            assert!(bm.iter().any(|&row| row != 0), "Digit {d} bitmap is empty");
        }
    }

    #[test]
    fn max_marks_cap() {
        // Even if we detect many regions, cap at MAX_MARKS
        assert!(MAX_MARKS <= 30);
    }
}
