use anyhow::Result;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

fn modifier_flags(modifiers: &[&str]) -> CGEventFlags {
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
    flags
}

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| {
        let errno = std::io::Error::last_os_error();
        anyhow::anyhow!("Failed to create CGEventSource (HIDSystemState): os error {errno}")
    })
}

pub fn move_mouse(x: f64, y: f64) -> Result<()> {
    anyhow::ensure!(
        x.is_finite() && y.is_finite(),
        "Invalid coordinates: ({x}, {y})"
    );
    let source = event_source()?;
    let point = CGPoint::new(x, y);
    let event =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|_| anyhow::anyhow!("Failed to create mouse move event"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn click(x: f64, y: f64, button: &str, click_count: u32, modifiers: &[&str]) -> Result<()> {
    anyhow::ensure!(
        x.is_finite() && y.is_finite(),
        "Invalid coordinates: ({x}, {y})"
    );
    let source = event_source()?;
    let point = CGPoint::new(x, y);

    let (down_type, up_type, cg_button) = match button {
        "right" => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        _ => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
    };

    let down = CGEvent::new_mouse_event(source.clone(), down_type, point, cg_button)
        .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;
    down.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count as i64,
    );

    let up = CGEvent::new_mouse_event(source.clone(), up_type, point, cg_button)
        .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;
    up.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count as i64,
    );

    let flags = modifier_flags(modifiers);
    if !flags.is_empty() {
        down.set_flags(flags);
        up.set_flags(flags);
    }

    // Post both only after both are created.
    // 15ms delay between down/up — macOS window server can drop events posted
    // back-to-back with zero gap, especially on Sonoma 14+.
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(15));
    up.post(CGEventTapLocation::HID);

    Ok(())
}

pub fn scroll(dx: i32, dy: i32) -> Result<()> {
    let source = event_source()?;
    let event = CGEvent::new_scroll_event(
        source,
        ScrollEventUnit::PIXEL,
        2,   // wheel_count
        -dy, // wheel1 (vertical) — negate: Gemini sends +dy=down, CG expects +wheel1=up
        dx,  // wheel2 (horizontal)
        0,   // wheel3
    )
    .map_err(|_| anyhow::anyhow!("Failed to create scroll event"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn drag(from_x: f64, from_y: f64, to_x: f64, to_y: f64, modifiers: &[&str]) -> Result<()> {
    anyhow::ensure!(
        from_x.is_finite() && from_y.is_finite() && to_x.is_finite() && to_y.is_finite(),
        "Invalid drag coordinates"
    );
    let source = event_source()?;
    let from = CGPoint::new(from_x, from_y);
    let to = CGPoint::new(to_x, to_y);
    let flags = modifier_flags(modifiers);

    // Mouse down at source
    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        from,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create drag down event"))?;
    if !flags.is_empty() {
        down.set_flags(flags);
    }
    down.post(CGEventTapLocation::HID);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Interpolate intermediate points every 20px
    let dx = to_x - from_x;
    let dy = to_y - from_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = ((distance / 20.0).ceil() as usize).max(1);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + dx * t;
        let iy = from_y + dy * t;
        let point = CGPoint::new(ix, iy);
        let drag_ev = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDragged,
            point,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("Failed to create drag move event"))?;
        if !flags.is_empty() {
            drag_ev.set_flags(flags);
        }
        drag_ev.post(CGEventTapLocation::HID);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Mouse up at destination
    let up = CGEvent::new_mouse_event(source, CGEventType::LeftMouseUp, to, CGMouseButton::Left)
        .map_err(|_| anyhow::anyhow!("Failed to create drag up event"))?;
    if !flags.is_empty() {
        up.set_flags(flags);
    }
    up.post(CGEventTapLocation::HID);

    Ok(())
}

pub fn move_mouse_pid(x: f64, y: f64, pid: i32) -> Result<()> {
    anyhow::ensure!(pid > 0, "Invalid PID: {pid}");
    anyhow::ensure!(
        x.is_finite() && y.is_finite(),
        "Invalid coordinates: ({x}, {y})"
    );
    let source = event_source()?;
    let point = CGPoint::new(x, y);
    let event =
        CGEvent::new_mouse_event(source, CGEventType::MouseMoved, point, CGMouseButton::Left)
            .map_err(|_| anyhow::anyhow!("Failed to create mouse move event"))?;
    event.post_to_pid(pid);
    Ok(())
}

pub fn click_pid(
    x: f64,
    y: f64,
    button: &str,
    click_count: u32,
    modifiers: &[&str],
    pid: i32,
) -> Result<()> {
    anyhow::ensure!(pid > 0, "Invalid PID: {pid}");
    anyhow::ensure!(
        x.is_finite() && y.is_finite(),
        "Invalid coordinates: ({x}, {y})"
    );
    let source = event_source()?;
    let point = CGPoint::new(x, y);

    let (down_type, up_type, cg_button) = match button {
        "right" => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        _ => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
    };

    let down = CGEvent::new_mouse_event(source.clone(), down_type, point, cg_button)
        .map_err(|_| anyhow::anyhow!("Failed to create mouse down event"))?;
    down.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count as i64,
    );

    let up = CGEvent::new_mouse_event(source.clone(), up_type, point, cg_button)
        .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;
    up.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count as i64,
    );

    let flags = modifier_flags(modifiers);
    if !flags.is_empty() {
        down.set_flags(flags);
        up.set_flags(flags);
    }

    down.post_to_pid(pid);
    std::thread::sleep(std::time::Duration::from_millis(15));
    up.post_to_pid(pid);

    Ok(())
}

pub fn scroll_pid(dx: i32, dy: i32, pid: i32) -> Result<()> {
    anyhow::ensure!(pid > 0, "Invalid PID: {pid}");
    let source = event_source()?;
    let event = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 2, -dy, dx, 0)
        .map_err(|_| anyhow::anyhow!("Failed to create scroll event"))?;
    event.post_to_pid(pid);
    Ok(())
}

pub fn drag_pid(
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    modifiers: &[&str],
    pid: i32,
) -> Result<()> {
    anyhow::ensure!(pid > 0, "Invalid PID: {pid}");
    anyhow::ensure!(
        from_x.is_finite() && from_y.is_finite() && to_x.is_finite() && to_y.is_finite(),
        "Invalid drag coordinates"
    );
    let source = event_source()?;
    let from = CGPoint::new(from_x, from_y);
    let to = CGPoint::new(to_x, to_y);
    let flags = modifier_flags(modifiers);

    // Mouse down at source
    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        from,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create drag down event"))?;
    if !flags.is_empty() {
        down.set_flags(flags);
    }
    down.post_to_pid(pid);
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Interpolate intermediate points every 20px
    let dx = to_x - from_x;
    let dy = to_y - from_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let steps = ((distance / 20.0).ceil() as usize).max(1);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let ix = from_x + dx * t;
        let iy = from_y + dy * t;
        let point = CGPoint::new(ix, iy);
        let drag_ev = CGEvent::new_mouse_event(
            source.clone(),
            CGEventType::LeftMouseDragged,
            point,
            CGMouseButton::Left,
        )
        .map_err(|_| anyhow::anyhow!("Failed to create drag move event"))?;
        if !flags.is_empty() {
            drag_ev.set_flags(flags);
        }
        drag_ev.post_to_pid(pid);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Mouse up at destination
    let up = CGEvent::new_mouse_event(source, CGEventType::LeftMouseUp, to, CGMouseButton::Left)
        .map_err(|_| anyhow::anyhow!("Failed to create drag up event"))?;
    if !flags.is_empty() {
        up.set_flags(flags);
    }
    up.post_to_pid(pid);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_pid_rejects_zero_pid() {
        let result = click_pid(100.0, 100.0, "left", 1, &[], 0);
        assert!(result.is_err());
    }

    #[test]
    fn move_mouse_pid_rejects_zero_pid() {
        let result = move_mouse_pid(100.0, 100.0, 0);
        assert!(result.is_err());
    }

    #[test]
    fn scroll_pid_rejects_zero_pid() {
        let result = scroll_pid(0, 100, 0);
        assert!(result.is_err());
    }

    #[test]
    fn drag_pid_rejects_zero_pid() {
        let result = drag_pid(0.0, 0.0, 100.0, 100.0, &[], 0);
        assert!(result.is_err());
    }
}
