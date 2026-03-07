use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};
use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

use crate::renderer::{OverlayRenderer, OverlayState};

// ---------------------------------------------------------------------------
// Messages from async tasks to the overlay event loop
// ---------------------------------------------------------------------------

/// Messages sent from async tasks to the overlay event loop.
#[derive(Debug, Clone)]
pub enum OverlayMessage {
    Show,
    Hide,
    SetState(OverlayState),
    Shutdown,
}

// ---------------------------------------------------------------------------
// Overlay window
// ---------------------------------------------------------------------------

pub struct OverlayWindow {
    window: Option<Arc<Window>>,
    context: Option<Context<Arc<Window>>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,
    renderer: Option<OverlayRenderer>,
    state: OverlayState,
    visible: bool,
    last_frame: Instant,
}

impl OverlayWindow {
    pub fn new() -> Self {
        Self {
            window: None,
            context: None,
            surface: None,
            renderer: None,
            state: OverlayState::Idle { breath_phase: 0.0 },
            visible: false,
            last_frame: Instant::now(),
        }
    }

    pub fn show(&mut self) {
        if let Some(ref window) = self.window {
            window.set_visible(true);
            self.visible = true;
            window.request_redraw();
        } else {
            tracing::warn!("Cannot show overlay: window not yet created");
        }
    }

    pub fn hide(&mut self) {
        if let Some(ref window) = self.window {
            window.set_visible(false);
            self.visible = false;
        } else {
            tracing::warn!("Cannot hide overlay: window not yet created");
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

    fn render_frame(&mut self) {
        let Some(ref window) = self.window else { return };
        let Some(ref mut surface) = self.surface else { return };
        let Some(ref mut renderer) = self.renderer else { return };

        let size = window.inner_size();
        let width = size.width;
        let height = size.height;
        if width == 0 || height == 0 {
            return;
        }

        // Resize softbuffer
        let Some(nz_width) = NonZeroU32::new(width) else { return };
        let Some(nz_height) = NonZeroU32::new(height) else { return };
        if surface.resize(nz_width, nz_height).is_err() {
            tracing::warn!("Failed to resize softbuffer surface");
            return;
        }

        // Create Skia raster surface backed by a pixel buffer
        let info = ImageInfo::new(
            (width as i32, height as i32),
            ColorType::RGBA8888,
            AlphaType::Premul,
            None,
        );
        let row_bytes = width as usize * 4;
        let mut pixel_data = vec![0u8; row_bytes * height as usize];

        let mut skia_surface = surfaces::wrap_pixels(
            &info,
            &mut pixel_data,
            Some(row_bytes),
            None,
        );

        let Some(ref mut skia_surface) = skia_surface else {
            tracing::warn!("Failed to create Skia raster surface");
            return;
        };

        // Calculate delta time
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Clear canvas and render
        let canvas = skia_surface.canvas();
        canvas.clear(skia_safe::Color::TRANSPARENT);

        renderer.resize(width as f32, height as f32);
        renderer.render(canvas, &self.state, dt);

        // Blit RGBA pixels to softbuffer XRGB format
        let Ok(mut buffer) = surface.buffer_mut() else {
            tracing::warn!("Failed to get softbuffer buffer");
            return;
        };

        for (i, pixel) in buffer.iter_mut().enumerate() {
            let offset = i * 4;
            if offset + 3 < pixel_data.len() {
                let r = pixel_data[offset] as u32;
                let g = pixel_data[offset + 1] as u32;
                let b = pixel_data[offset + 2] as u32;
                *pixel = (r << 16) | (g << 8) | b;
            }
        }

        if buffer.present().is_err() {
            tracing::warn!("Failed to present softbuffer");
        }

        // Request next frame if visible (animation loop)
        if self.visible {
            window.request_redraw();
        }
    }
}

// ---------------------------------------------------------------------------
// ApplicationHandler — drives the winit event loop
// ---------------------------------------------------------------------------

impl ApplicationHandler<OverlayMessage> for OverlayWindow {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("Aura")
            .with_transparent(true)
            .with_decorations(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);

        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);

                // Create softbuffer context and surface
                match Context::new(window.clone()) {
                    Ok(context) => {
                        match Surface::new(&context, window.clone()) {
                            Ok(sb_surface) => {
                                self.surface = Some(sb_surface);
                            }
                            Err(e) => {
                                tracing::error!("Failed to create softbuffer surface: {e}");
                            }
                        }
                        self.context = Some(context);
                    }
                    Err(e) => {
                        tracing::error!("Failed to create softbuffer context: {e}");
                    }
                }

                let size = window.inner_size();
                self.renderer = Some(OverlayRenderer::new(
                    size.width as f32,
                    size.height as f32,
                ));

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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: OverlayMessage) {
        match event {
            OverlayMessage::Show => self.show(),
            OverlayMessage::Hide => self.hide(),
            OverlayMessage::SetState(state) => {
                self.state = state;
                if self.visible {
                    if let Some(ref window) = self.window {
                        window.request_redraw();
                    }
                }
            }
            OverlayMessage::Shutdown => {
                event_loop.exit();
            }
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
                self.render_frame();
            }
            WindowEvent::Resized(size) => {
                if let Some(ref mut renderer) = self.renderer {
                    renderer.resize(size.width as f32, size.height as f32);
                }
            }
            _ => {}
        }
    }
}

/// Create the overlay event loop configured to accept [`OverlayMessage`]s.
pub fn create_event_loop() -> Result<EventLoop<OverlayMessage>> {
    let event_loop = EventLoop::with_user_event().build()?;
    Ok(event_loop)
}
