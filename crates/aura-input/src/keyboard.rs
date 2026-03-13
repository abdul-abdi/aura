use anyhow::Result;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| {
        let errno = std::io::Error::last_os_error();
        anyhow::anyhow!("Failed to create CGEventSource (HIDSystemState): os error {errno}")
    })
}

/// Type a string by posting key events for each character.
/// Uses CGEventKeyboardSetUnicodeString for reliable Unicode support.
pub fn type_text(text: &str) -> Result<()> {
    let source = event_source()?;

    for ch in text.chars() {
        let mut buf = [0u16; 2];
        let encoded = ch.encode_utf16(&mut buf);

        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
        down.set_string_from_utf16_unchecked(encoded);

        let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;

        // Post both only after both are created
        down.post(CGEventTapLocation::HID);
        up.post(CGEventTapLocation::HID);

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    Ok(())
}

/// Press a key with optional modifiers.
/// `key`: virtual keycode (e.g. 36 = Return, 53 = Escape, 49 = Space)
/// `modifiers`: list of "cmd", "shift", "alt", "ctrl"
pub fn press_key(key: CGKeyCode, modifiers: &[&str]) -> Result<()> {
    let source = event_source()?;
    let mut flags = CGEventFlags::empty();

    for m in modifiers {
        match *m {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown modifier '{m}'. Valid modifiers: cmd, shift, alt, option, ctrl, control"
                ))
            }
        }
    }

    let down = CGEvent::new_keyboard_event(source.clone(), key, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
    down.set_flags(flags);

    let up = CGEvent::new_keyboard_event(source.clone(), key, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
    up.set_flags(CGEventFlags::CGEventFlagNull);

    // Post both only after both are created; 12ms gap mirrors mouse click delay
    // to prevent the macOS window server from dropping key events.
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(12));
    up.post(CGEventTapLocation::HID);

    Ok(())
}

/// Type text targeted at a specific process (PID-targeted, not global HID).
pub fn type_text_pid(text: &str, pid: i32) -> Result<()> {
    anyhow::ensure!(pid > 0, "Invalid PID: {pid}");
    let source = event_source()?;

    for ch in text.chars() {
        let mut buf = [0u16; 2];
        let encoded = ch.encode_utf16(&mut buf);

        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
        down.set_string_from_utf16_unchecked(encoded);

        let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;

        down.post_to_pid(pid);
        up.post_to_pid(pid);

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// Press a key with optional modifiers targeted at a specific process.
pub fn press_key_pid(key: CGKeyCode, modifiers: &[&str], pid: i32) -> Result<()> {
    anyhow::ensure!(pid > 0, "Invalid PID: {pid}");
    let source = event_source()?;
    let mut flags = CGEventFlags::empty();

    for m in modifiers {
        match *m {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown modifier '{m}'. Valid modifiers: cmd, shift, alt, option, ctrl, control"
                ))
            }
        }
    }

    let down = CGEvent::new_keyboard_event(source.clone(), key, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
    down.set_flags(flags);

    let up = CGEvent::new_keyboard_event(source.clone(), key, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
    up.set_flags(CGEventFlags::CGEventFlagNull);

    // 12ms gap mirrors mouse click delay to prevent the macOS window server
    // from dropping key events.
    down.post_to_pid(pid);
    std::thread::sleep(std::time::Duration::from_millis(12));
    up.post_to_pid(pid);

    Ok(())
}

/// Press a key down without releasing. Caller must call key_up later.
pub fn key_down(key: CGKeyCode, modifiers: &[&str]) -> Result<()> {
    let source = event_source()?;
    let mut flags = CGEventFlags::empty();
    for m in modifiers {
        match *m {
            "cmd" | "command" => flags |= CGEventFlags::CGEventFlagCommand,
            "shift" => flags |= CGEventFlags::CGEventFlagShift,
            "alt" | "option" => flags |= CGEventFlags::CGEventFlagAlternate,
            "ctrl" | "control" => flags |= CGEventFlags::CGEventFlagControl,
            _ => {}
        }
    }
    let down = CGEvent::new_keyboard_event(source, key, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);
    Ok(())
}

/// Release a previously held key.
pub fn key_up(key: CGKeyCode) -> Result<()> {
    let source = event_source()?;
    let up = CGEvent::new_keyboard_event(source, key, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
    up.set_flags(CGEventFlags::CGEventFlagNull);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

/// Map common key names to macOS virtual keycodes.
pub fn keycode_from_name(name: &str) -> Option<CGKeyCode> {
    match name.to_lowercase().as_str() {
        "return" | "enter" => Some(36),
        "tab" => Some(48),
        "space" => Some(49),
        "delete" | "backspace" => Some(51),
        "escape" | "esc" => Some(53),
        "left" => Some(123),
        "right" => Some(124),
        "down" => Some(125),
        "up" => Some(126),
        "f1" => Some(122),
        "f2" => Some(120),
        "f3" => Some(99),
        "f4" => Some(118),
        "f5" => Some(96),
        "f6" => Some(97),
        "f7" => Some(98),
        "f8" => Some(100),
        "f9" => Some(101),
        "f10" => Some(109),
        "f11" => Some(103),
        "f12" => Some(111),
        "a" => Some(0),
        "b" => Some(11),
        "c" => Some(8),
        "d" => Some(2),
        "e" => Some(14),
        "f" => Some(3),
        "g" => Some(5),
        "h" => Some(4),
        "i" => Some(34),
        "j" => Some(38),
        "k" => Some(40),
        "l" => Some(37),
        "m" => Some(46),
        "n" => Some(45),
        "o" => Some(31),
        "p" => Some(35),
        "q" => Some(12),
        "r" => Some(15),
        "s" => Some(1),
        "t" => Some(17),
        "u" => Some(32),
        "v" => Some(9),
        "w" => Some(13),
        "x" => Some(7),
        "y" => Some(16),
        "z" => Some(6),
        // Number keys (main keyboard row)
        "0" => Some(29),
        "1" => Some(18),
        "2" => Some(19),
        "3" => Some(20),
        "4" => Some(21),
        "5" => Some(23),
        "6" => Some(22),
        "7" => Some(26),
        "8" => Some(28),
        "9" => Some(25),
        // Navigation keys
        "home" => Some(115),
        "end" => Some(119),
        "pageup" | "page_up" => Some(116),
        "pagedown" | "page_down" => Some(121),
        "forwarddelete" | "forward_delete" => Some(117),
        "capslock" | "caps_lock" => Some(57),
        // Punctuation (US keyboard layout)
        "-" | "minus" => Some(27),
        "=" | "equals" => Some(24),
        "[" | "leftbracket" => Some(33),
        "]" | "rightbracket" => Some(30),
        "\\" | "backslash" => Some(42),
        ";" | "semicolon" => Some(41),
        "'" | "quote" => Some(39),
        "," | "comma" => Some(43),
        "." | "period" => Some(47),
        "/" | "slash" => Some(44),
        "`" | "grave" => Some(50),
        // Modifier keys
        "shift" => Some(56),
        "cmd" | "command" => Some(55),
        "alt" | "option" => Some(58),
        "ctrl" | "control" => Some(59),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_text_pid_rejects_zero_pid() {
        let result = type_text_pid("hello", 0);
        assert!(result.is_err());
    }

    #[test]
    fn press_key_pid_rejects_zero_pid() {
        let result = press_key_pid(36, &[], 0);
        assert!(result.is_err());
    }

    #[test]
    fn press_key_unknown_modifier_returns_error() {
        let result = press_key(36, &["super"]);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("super"), "error should name the bad modifier");
        assert!(msg.contains("cmd"), "error should list valid modifiers");
    }

    #[test]
    fn press_key_pid_unknown_modifier_returns_error() {
        // Use a valid PID (1 = launchd) so we don't fail on the PID guard first.
        let result = press_key_pid(36, &["super"], 1);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("super"), "error should name the bad modifier");
        assert!(msg.contains("cmd"), "error should list valid modifiers");
    }

    // --- Special keys ---

    #[test]
    fn keycode_return() {
        assert_eq!(keycode_from_name("return"), Some(36));
        assert_eq!(keycode_from_name("enter"), Some(36));
        assert_eq!(keycode_from_name("Return"), Some(36));
        assert_eq!(keycode_from_name("ENTER"), Some(36));
    }

    #[test]
    fn keycode_tab() {
        assert_eq!(keycode_from_name("tab"), Some(48));
        assert_eq!(keycode_from_name("Tab"), Some(48));
    }

    #[test]
    fn keycode_space() {
        assert_eq!(keycode_from_name("space"), Some(49));
        assert_eq!(keycode_from_name("SPACE"), Some(49));
    }

    #[test]
    fn keycode_delete() {
        assert_eq!(keycode_from_name("delete"), Some(51));
        assert_eq!(keycode_from_name("backspace"), Some(51));
        assert_eq!(keycode_from_name("Delete"), Some(51));
    }

    #[test]
    fn keycode_escape() {
        assert_eq!(keycode_from_name("escape"), Some(53));
        assert_eq!(keycode_from_name("esc"), Some(53));
        assert_eq!(keycode_from_name("Escape"), Some(53));
    }

    // --- Arrow keys ---

    #[test]
    fn keycode_arrows() {
        assert_eq!(keycode_from_name("left"), Some(123));
        assert_eq!(keycode_from_name("right"), Some(124));
        assert_eq!(keycode_from_name("down"), Some(125));
        assert_eq!(keycode_from_name("up"), Some(126));
        assert_eq!(keycode_from_name("Left"), Some(123));
        assert_eq!(keycode_from_name("UP"), Some(126));
    }

    // --- Function keys ---

    #[test]
    fn keycode_function_keys() {
        assert_eq!(keycode_from_name("f1"), Some(122));
        assert_eq!(keycode_from_name("f2"), Some(120));
        assert_eq!(keycode_from_name("f3"), Some(99));
        assert_eq!(keycode_from_name("f4"), Some(118));
        assert_eq!(keycode_from_name("f5"), Some(96));
        assert_eq!(keycode_from_name("f6"), Some(97));
        assert_eq!(keycode_from_name("f7"), Some(98));
        assert_eq!(keycode_from_name("f8"), Some(100));
        assert_eq!(keycode_from_name("f9"), Some(101));
        assert_eq!(keycode_from_name("f10"), Some(109));
        assert_eq!(keycode_from_name("f11"), Some(103));
        assert_eq!(keycode_from_name("f12"), Some(111));
        // Case insensitive
        assert_eq!(keycode_from_name("F1"), Some(122));
        assert_eq!(keycode_from_name("F12"), Some(111));
    }

    // --- Letters ---

    #[test]
    fn keycode_all_letters() {
        let expected: &[(&str, CGKeyCode)] = &[
            ("a", 0),
            ("b", 11),
            ("c", 8),
            ("d", 2),
            ("e", 14),
            ("f", 3),
            ("g", 5),
            ("h", 4),
            ("i", 34),
            ("j", 38),
            ("k", 40),
            ("l", 37),
            ("m", 46),
            ("n", 45),
            ("o", 31),
            ("p", 35),
            ("q", 12),
            ("r", 15),
            ("s", 1),
            ("t", 17),
            ("u", 32),
            ("v", 9),
            ("w", 13),
            ("x", 7),
            ("y", 16),
            ("z", 6),
        ];
        for &(name, code) in expected {
            assert_eq!(
                keycode_from_name(name),
                Some(code),
                "keycode for '{name}' should be {code}"
            );
        }
    }

    #[test]
    fn keycode_uppercase_letters() {
        // Should be case-insensitive
        assert_eq!(keycode_from_name("A"), Some(0));
        assert_eq!(keycode_from_name("Z"), Some(6));
        assert_eq!(keycode_from_name("M"), Some(46));
    }

    // --- Unknown keys ---

    // --- Number keys ---

    #[test]
    fn keycode_number_keys() {
        let expected: &[(&str, CGKeyCode)] = &[
            ("0", 29),
            ("1", 18),
            ("2", 19),
            ("3", 20),
            ("4", 21),
            ("5", 23),
            ("6", 22),
            ("7", 26),
            ("8", 28),
            ("9", 25),
        ];
        for &(name, code) in expected {
            assert_eq!(
                keycode_from_name(name),
                Some(code),
                "keycode for '{name}' should be {code}"
            );
        }
    }

    // --- Navigation keys ---

    #[test]
    fn keycode_navigation_keys() {
        assert_eq!(keycode_from_name("home"), Some(115));
        assert_eq!(keycode_from_name("Home"), Some(115));
        assert_eq!(keycode_from_name("end"), Some(119));
        assert_eq!(keycode_from_name("End"), Some(119));
        assert_eq!(keycode_from_name("pageup"), Some(116));
        assert_eq!(keycode_from_name("page_up"), Some(116));
        assert_eq!(keycode_from_name("PageUp"), Some(116));
        assert_eq!(keycode_from_name("pagedown"), Some(121));
        assert_eq!(keycode_from_name("page_down"), Some(121));
        assert_eq!(keycode_from_name("PageDown"), Some(121));
        assert_eq!(keycode_from_name("forwarddelete"), Some(117));
        assert_eq!(keycode_from_name("forward_delete"), Some(117));
        assert_eq!(keycode_from_name("ForwardDelete"), Some(117));
        assert_eq!(keycode_from_name("capslock"), Some(57));
        assert_eq!(keycode_from_name("caps_lock"), Some(57));
        assert_eq!(keycode_from_name("CapsLock"), Some(57));
    }

    // --- Punctuation keys ---

    #[test]
    fn keycode_punctuation_keys() {
        let expected: &[(&str, CGKeyCode)] = &[
            ("-", 27),
            ("minus", 27),
            ("=", 24),
            ("equals", 24),
            ("[", 33),
            ("leftbracket", 33),
            ("]", 30),
            ("rightbracket", 30),
            ("\\", 42),
            ("backslash", 42),
            (";", 41),
            ("semicolon", 41),
            ("'", 39),
            ("quote", 39),
            (",", 43),
            ("comma", 43),
            (".", 47),
            ("period", 47),
            ("/", 44),
            ("slash", 44),
            ("`", 50),
            ("grave", 50),
        ];
        for &(name, code) in expected {
            assert_eq!(
                keycode_from_name(name),
                Some(code),
                "keycode for '{name}' should be {code}"
            );
        }
    }

    // --- Unknown keys ---

    #[test]
    fn keycode_unknown_returns_none() {
        assert_eq!(keycode_from_name("unknown"), None);
        assert_eq!(keycode_from_name(""), None);
        assert_eq!(keycode_from_name("f13"), None);
    }

    // --- Modifier keys ---

    #[test]
    fn keycode_modifier_keys() {
        assert_eq!(keycode_from_name("shift"), Some(56));
        assert_eq!(keycode_from_name("Shift"), Some(56));
        assert_eq!(keycode_from_name("cmd"), Some(55));
        assert_eq!(keycode_from_name("command"), Some(55));
        assert_eq!(keycode_from_name("alt"), Some(58));
        assert_eq!(keycode_from_name("option"), Some(58));
        assert_eq!(keycode_from_name("ctrl"), Some(59));
        assert_eq!(keycode_from_name("control"), Some(59));
    }

    // --- key_down / key_up ---

    #[test]
    fn key_down_creates_event_without_panic() {
        let result = key_down(56, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn key_up_creates_event_without_panic() {
        let result = key_up(56);
        assert!(result.is_ok());
    }
}
