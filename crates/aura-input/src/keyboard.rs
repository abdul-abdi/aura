use anyhow::Result;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))
}

/// Type a string by posting key events for each character.
/// Uses CGEventKeyboardSetUnicodeString for reliable Unicode support.
pub fn type_text(text: &str) -> Result<()> {
    let source = event_source()?;

    for ch in text.chars() {
        let buf = [ch as u16];
        let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
            .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
        down.set_string_from_utf16_unchecked(&buf);
        down.post(CGEventTapLocation::HID);

        let up_source = event_source()?;
        let up = CGEvent::new_keyboard_event(up_source, 0, false)
            .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
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
            _ => tracing::warn!("Unknown modifier: {m}"),
        }
    }

    let down = CGEvent::new_keyboard_event(source.clone(), key, true)
        .map_err(|_| anyhow::anyhow!("Failed to create key down event"))?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);

    let up_source = event_source()?;
    let up = CGEvent::new_keyboard_event(up_source, key, false)
        .map_err(|_| anyhow::anyhow!("Failed to create key up event"))?;
    up.set_flags(flags);
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
        _ => None,
    }
}
