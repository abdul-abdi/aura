use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

pub struct OverlayWindow {
    window: Option<Window>,
    visible: bool,
}

impl OverlayWindow {
    pub fn new() -> Self {
        Self {
            window: None,
            visible: false,
        }
    }

    pub fn show(&mut self) {
        if let Some(ref window) = self.window {
            window.set_visible(true);
            self.visible = true;
        }
    }

    pub fn hide(&mut self) {
        if let Some(ref window) = self.window {
            window.set_visible(false);
            self.visible = false;
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn request_redraw(&self) {
        if let Some(ref window) = self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for OverlayWindow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("Aura")
            .with_transparent(true)
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);

        match event_loop.create_window(attrs) {
            Ok(window) => {
                #[cfg(target_os = "macos")]
                {
                    // Platform-specific click-through setup
                    // Will use objc messaging in full implementation
                }
                self.window = Some(window);
            }
            Err(e) => tracing::error!("Failed to create overlay window: {e}"),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.hide();
            }
            WindowEvent::RedrawRequested => {
                // Renderer dispatches draw calls here — see Task 8b
            }
            _ => {}
        }
    }
}

pub fn create_event_loop() -> Result<EventLoop<()>> {
    let event_loop = EventLoop::new()?;
    Ok(event_loop)
}
