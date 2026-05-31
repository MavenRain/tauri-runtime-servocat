//! Tiny-skia rasterizer.  Walks a [`Frame`]'s display list and paints
//! the `FillRect` and `StrokeRect` commands into an RGBA pixel buffer.
//!
//! v0.2 limitations:
//!
//! - `FillText` is recorded as a placeholder rectangle (background
//!   highlight only); proper text shaping waits on v0.3 cosmic-text.
//! - No clipping, transforms, or stacking contexts.
//! - No anti-aliased path edges beyond what tiny-skia gives by default.

use paint_cat::PaintCommand;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::frame::Frame;

/// A rasterized pixel buffer.
#[derive(Debug, Clone)]
pub struct PixelBuffer {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl PixelBuffer {
    /// Buffer width in CSS pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Buffer height in CSS pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw RGBA bytes (8 bits per channel, premultiplied alpha per
    /// tiny-skia's convention).  Length is `width * height * 4`.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.bytes
    }

    /// Sample the byte at `(x, y, channel)`; `None` if out of bounds.
    /// Channel is 0=R, 1=G, 2=B, 3=A.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32, channel: usize) -> Option<u8> {
        if x >= self.width || y >= self.height || channel >= 4 {
            None
        } else {
            let row_bytes = usize::try_from(self.width)
                .ok()
                .and_then(|w| w.checked_mul(4))?;
            let row_index = usize::try_from(y).ok()?.checked_mul(row_bytes)?;
            let col_index = usize::try_from(x).ok().and_then(|c| c.checked_mul(4))?;
            self.bytes.get(row_index + col_index + channel).copied()
        }
    }
}

/// Rasterize `frame` into an RGBA pixel buffer of the given size.
/// The viewport is treated as starting at `(0, 0)`; pixels outside the
/// requested dimensions are clipped by tiny-skia.
#[must_use]
pub fn render_to_pixels(frame: &Frame, width: u32, height: u32) -> PixelBuffer {
    let bytes = rasterize_to_bytes(frame, width, height);
    PixelBuffer {
        width,
        height,
        bytes,
    }
}

fn rasterize_to_bytes(frame: &Frame, width: u32, height: u32) -> Vec<u8> {
    Pixmap::new(width, height).map_or_else(
        || empty_bytes(width, height),
        |pixmap| paint_commands(pixmap, frame),
    )
}

fn empty_bytes(width: u32, height: u32) -> Vec<u8> {
    let total = usize::try_from(width)
        .ok()
        .and_then(|w| {
            usize::try_from(height)
                .ok()
                .and_then(|h| w.checked_mul(h).and_then(|p| p.checked_mul(4)))
        })
        .unwrap_or(0);
    vec![0; total]
}

fn paint_commands(pixmap_in: Pixmap, frame: &Frame) -> Vec<u8> {
    // External `&mut self` carve-out for tiny-skia's `fill_path` /
    // `stroke_path`.  Pixmap construction returns an owned value; we
    // shadow the binding as `mut` only to satisfy the tiny-skia API.
    let mut pixmap = pixmap_in;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);
    frame.display_list().commands().iter().for_each(|cmd| {
        apply_command(&mut pixmap, cmd);
    });
    pixmap.take()
}

fn apply_command(pixmap: &mut Pixmap, command: &PaintCommand) {
    match command {
        PaintCommand::FillRect { rect, color } => fill_rect(pixmap, rect, color),
        PaintCommand::StrokeRect { rect, color, width } => stroke_rect(pixmap, rect, color, *width),
        PaintCommand::FillText { rect, color, .. } => placeholder_text(pixmap, rect, color),
    }
}

fn fill_rect(pixmap: &mut Pixmap, rect: &layout_cat::Rect, color: &layout_cat::Color) {
    if let Some(skia_rect) = rect_to_skia(rect) {
        let paint = build_paint(color);
        if let Some(path) = PathBuilder::from_rect(skia_rect).into() {
            let _ = path;
        }
        let path = PathBuilder::from_rect(skia_rect);
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke_rect(
    pixmap: &mut Pixmap,
    rect: &layout_cat::Rect,
    color: &layout_cat::Color,
    width: f64,
) {
    if let Some(skia_rect) = rect_to_skia(rect) {
        let paint = build_paint(color);
        let path = PathBuilder::from_rect(skia_rect);
        let stroke = Stroke {
            width: f32_from_f64(width).max(0.0),
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn placeholder_text(pixmap: &mut Pixmap, rect: &layout_cat::Rect, color: &layout_cat::Color) {
    // Until cosmic-text lands in v0.3, FillText is rendered as a faint
    // highlight at 30% of the text color's alpha so callers can see
    // that text would appear in that region.
    let dimmed = layout_cat::Color::rgba(
        color.red(),
        color.green(),
        color.blue(),
        color.alpha() * 0.3,
    );
    fill_rect(pixmap, rect, &dimmed);
}

fn rect_to_skia(rect: &layout_cat::Rect) -> Option<Rect> {
    let x = f32_from_f64(rect.origin().x());
    let y = f32_from_f64(rect.origin().y());
    let w = f32_from_f64(rect.width()).max(0.0);
    let h = f32_from_f64(rect.height()).max(0.0);
    if w == 0.0 || h == 0.0 {
        None
    } else {
        Rect::from_xywh(x, y, w, h)
    }
}

fn build_paint(color: &layout_cat::Color) -> Paint<'static> {
    let r = f32_from_f64(color.red()).clamp(0.0, 1.0);
    let g = f32_from_f64(color.green()).clamp(0.0, 1.0);
    let b = f32_from_f64(color.blue()).clamp(0.0, 1.0);
    let a = f32_from_f64(color.alpha()).clamp(0.0, 1.0);
    let skia_color =
        tiny_skia::Color::from_rgba(r, g, b, a).unwrap_or(tiny_skia::Color::TRANSPARENT);
    let mut paint = Paint::default();
    paint.set_color(skia_color);
    paint.anti_alias = true;
    paint
}

fn f32_from_f64(value: f64) -> f32 {
    // tiny-skia's API is f32; we accept the precision loss and saturate
    // at the f32 extremes.
    if value.is_finite() {
        #[allow(clippy::cast_possible_truncation)]
        let n = value as f32;
        n
    } else if value > 0.0 {
        f32::INFINITY
    } else if value < 0.0 {
        f32::NEG_INFINITY
    } else {
        0.0
    }
}
