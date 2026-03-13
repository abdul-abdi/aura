//! Safe-to-pipeline action pair detection.
//!
//! Determines when consecutive state-changing tool calls can skip
//! intermediate verification, reducing multi-step action latency.

/// Maximum consecutive actions that can be pipelined without verification.
/// Safety cap to prevent error cascades.
pub(crate) const MAX_CHAIN_LENGTH: usize = 3;

/// Micro-settle delay (ms) between pipelined actions.
pub(crate) const MICRO_SETTLE_MS: u64 = 30;

/// Returns true if `next_tool` can safely execute immediately after `prev_tool`
/// without waiting for screen verification of prev_tool's effect.
pub(crate) fn is_safe_continuation(prev_tool: &str, next_tool: &str) -> bool {
    matches!(
        (prev_tool, next_tool),
        // Type then press Enter/Tab/Escape — natural text entry sequence
        ("type_text", "press_key")
        // Key combo sequences (Cmd+C, Cmd+V, etc.)
        | ("press_key", "press_key")
        // Click into field then type — field is already focused after click
        | ("click", "type_text")
        | ("click_element", "type_text")
        // Activate app then interact with it
        | ("activate_app", "click")
        | ("activate_app", "click_element")
        | ("activate_app", "click_menu_item")
    )
}

/// Returns true if `chain_length` has reached the safety cap.
pub(crate) fn chain_at_limit(chain_length: usize) -> bool {
    chain_length >= MAX_CHAIN_LENGTH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_then_enter_is_safe() {
        assert!(is_safe_continuation("type_text", "press_key"));
    }

    #[test]
    fn key_combo_is_safe() {
        assert!(is_safe_continuation("press_key", "press_key"));
    }

    #[test]
    fn click_then_type_is_safe() {
        assert!(is_safe_continuation("click", "type_text"));
        assert!(is_safe_continuation("click_element", "type_text"));
    }

    #[test]
    fn activate_then_interact_is_safe() {
        assert!(is_safe_continuation("activate_app", "click"));
        assert!(is_safe_continuation("activate_app", "click_element"));
        assert!(is_safe_continuation("activate_app", "click_menu_item"));
    }

    #[test]
    fn unrelated_actions_not_safe() {
        assert!(!is_safe_continuation("click", "click"));
        assert!(!is_safe_continuation("scroll", "type_text"));
        assert!(!is_safe_continuation("run_applescript", "click"));
        assert!(!is_safe_continuation("type_text", "click"));
    }

    #[test]
    fn chain_limit() {
        assert!(!chain_at_limit(0));
        assert!(!chain_at_limit(1));
        assert!(!chain_at_limit(2));
        assert!(chain_at_limit(3));
        assert!(chain_at_limit(10));
    }
}
