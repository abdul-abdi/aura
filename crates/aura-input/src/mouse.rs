use anyhow::Result;
use core_graphics::event::{
    CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow::anyhow!("Failed to create CGEventSource"))
}

pub fn move_mouse(x: f64, y: f64) -> Result<()> {
    let source = event_source()?;
    let point = CGPoint::new(x, y);
    let event = CGEvent::new_mouse_event(
        source,
        CGEventType::MouseMoved,
        point,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create mouse move event"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn click(x: f64, y: f64, button: &str, click_count: u32) -> Result<()> {
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
    down.post(CGEventTapLocation::HID);

    let up_source = event_source()?;
    let up = CGEvent::new_mouse_event(up_source, up_type, point, cg_button)
        .map_err(|_| anyhow::anyhow!("Failed to create mouse up event"))?;
    up.set_integer_value_field(
        core_graphics::event::EventField::MOUSE_EVENT_CLICK_STATE,
        click_count as i64,
    );
    up.post(CGEventTapLocation::HID);

    Ok(())
}

pub fn scroll(dx: i32, dy: i32) -> Result<()> {
    let source = event_source()?;
    let event = CGEvent::new_scroll_event(
        source,
        ScrollEventUnit::PIXEL,
        2, // wheel_count
        dy,     // wheel1 (vertical)
        dx,     // wheel2 (horizontal)
        0,      // wheel3
    )
    .map_err(|_| anyhow::anyhow!("Failed to create scroll event"))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn drag(from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> Result<()> {
    let source = event_source()?;
    let from = CGPoint::new(from_x, from_y);
    let to = CGPoint::new(to_x, to_y);

    // Mouse down at start
    let down = CGEvent::new_mouse_event(
        source.clone(),
        CGEventType::LeftMouseDown,
        from,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create drag down event"))?;
    down.post(CGEventTapLocation::HID);

    std::thread::sleep(std::time::Duration::from_millis(50));

    // Move to destination
    let drag_source = event_source()?;
    let drag = CGEvent::new_mouse_event(
        drag_source,
        CGEventType::LeftMouseDragged,
        to,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create drag move event"))?;
    drag.post(CGEventTapLocation::HID);

    std::thread::sleep(std::time::Duration::from_millis(50));

    // Mouse up at destination
    let up_source = event_source()?;
    let up = CGEvent::new_mouse_event(
        up_source,
        CGEventType::LeftMouseUp,
        to,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow::anyhow!("Failed to create drag up event"))?;
    up.post(CGEventTapLocation::HID);

    Ok(())
}
