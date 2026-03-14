//! Vision Oracle: uses Gemini 3 Flash to extract precise click coordinates
//! from full-resolution screenshots when the Accessibility API cannot find
//! the target element.

use anyhow::{Context, Result};
use serde_json::Value;

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
}
