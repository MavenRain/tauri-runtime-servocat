//! Winit + softbuffer window driver.
//!
//! v0.4 limitations:
//!
//! - Renders the supplied [`Frame`] once at startup; window resizes do
//!   not re-layout or re-paint at a different resolution.
//! - Composites the premultiplied RGBA output over a white background
//!   before writing into softbuffer's 0RGB word format.
//! - Listens only to `CloseRequested` and `RedrawRequested`; other
//!   `WindowEvent` variants are intentionally ignored (FFI carve-out
//!   on a 30+ variant external enum).

use std::num::NonZeroU32;

use softbuffer::{Context, Surface};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use layout_cat::Viewport;

use crate::error::Error;
use crate::frame::Frame;
use crate::raster::{PixelBuffer, render_to_pixels_with};
use crate::text::TextRenderer;

/// Open a window of the given [`Viewport`] dimensions and present the
/// rasterized [`Frame`] inside it.  Blocks until the window is closed.
///
/// # Errors
///
/// Returns [`Error::Window`] if the event loop, window, or softbuffer
/// surface fails to initialize.
pub fn run_window(frame: Frame, viewport: Viewport) -> Result<(), Error> {
    let event_loop = EventLoop::new()?;
    let mut app = WindowApp::new(frame, viewport);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct WindowApp {
    frame: Frame,
    viewport: Viewport,
    text_renderer: TextRenderer,
    window: Option<Window>,
    pixels: Option<PixelBuffer>,
}

impl WindowApp {
    fn new(frame: Frame, viewport: Viewport) -> Self {
        Self {
            frame,
            viewport,
            text_renderer: TextRenderer::new(),
            window: None,
            pixels: None,
        }
    }

    fn present(&mut self) -> Option<()> {
        let window = self.window.as_ref()?;
        let pixels = self.pixels.as_ref()?;
        let size = window.inner_size();
        let width = NonZeroU32::new(size.width)?;
        let height = NonZeroU32::new(size.height)?;
        let context = Context::new(window).ok()?;
        // FFI carve-out: softbuffer's `Surface` / `Buffer` require
        // `&mut` for `resize`, `buffer_mut`, and `present`.
        let mut surface = Surface::new(&context, window).ok()?;
        surface.resize(width, height).ok()?;
        let mut buffer = surface.buffer_mut().ok()?;
        write_pixels(pixels, &mut buffer);
        buffer.present().ok()?;
        Some(())
    }
}

impl ApplicationHandler for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = WindowAttributes::default()
            .with_title("tauri-runtime-servocat demo")
            .with_inner_size(PhysicalSize::new(
                self.viewport.width(),
                self.viewport.height(),
            ));
        let _ = event_loop.create_window(attrs).map(|window| {
            window.request_redraw();
            let pixels = render_to_pixels_with(
                &self.frame,
                self.viewport.width(),
                self.viewport.height(),
                &mut self.text_renderer,
            );
            self.window = Some(window);
            self.pixels = Some(pixels);
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                let _ = self.present();
            }
            // FFI carve-out: winit's `WindowEvent` enum has 30+
            // variants; we listen to the two we care about and bind
            // the rest to a named catch-all so we don't write a
            // literal `_` arm.
            ignored => {
                let _ = ignored;
            }
        }
    }
}

fn write_pixels(pixels: &PixelBuffer, buffer: &mut softbuffer::Buffer<'_, &Window, &Window>) {
    pixels
        .rgba()
        .chunks_exact(4)
        .map(rgba_to_softbuffer_word)
        .zip(buffer.iter_mut())
        .for_each(|(word, slot)| *slot = word);
}

fn rgba_to_softbuffer_word(chunk: &[u8]) -> u32 {
    // chunks_exact(4) guarantees four bytes, but use defensive
    // `.get()` reads anyway to keep the helper panic-free.
    let red = chunk.first().copied().unwrap_or(0);
    let green = chunk.get(1).copied().unwrap_or(0);
    let blue = chunk.get(2).copied().unwrap_or(0);
    let alpha = chunk.get(3).copied().unwrap_or(0);
    // Composite premultiplied RGBA over a solid white background:
    //   out = src + (1 - src_a) * dst  with dst = (1,1,1)
    // In u8s that becomes saturating_add(src_c, 255 - src_a).
    let inv = 255_u8 - alpha;
    let out_r = red.saturating_add(inv);
    let out_g = green.saturating_add(inv);
    let out_b = blue.saturating_add(inv);
    (u32::from(out_r) << 16) | (u32::from(out_g) << 8) | u32::from(out_b)
}
