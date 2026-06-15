//! Tiny-skia rasterizer.  Walks a [`Frame`]'s display list and paints
//! the `FillRect`, `StrokeRect`, and `FillText` commands into an RGBA
//! pixel buffer.  `FillText` is delegated to a [`TextRenderer`] which
//! does cosmic-text shaping + swash glyph rasterization.
//!
//! v0.3 limitations:
//!
//! - No clipping, transforms, or stacking contexts.
//! - No anti-aliased path edges beyond what tiny-skia gives by default.

use layout_cat::LayoutBox;
use paint_cat::PaintCommand;
use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

use crate::frame::Frame;
use crate::images::DecodedImage;
use crate::text::TextRenderer;

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

/// Rasterize `frame` into an RGBA pixel buffer of the given size,
/// building a fresh [`TextRenderer`] for this call.  Convenient but
/// pays the font-load cost on every call.  Use
/// [`render_to_pixels_with`] to amortize.
#[must_use]
pub fn render_to_pixels(frame: &Frame, width: u32, height: u32) -> PixelBuffer {
    let mut text_renderer = TextRenderer::new();
    render_to_pixels_with(frame, width, height, &mut text_renderer)
}

/// Rasterize `frame` into an RGBA pixel buffer of the given size using
/// a caller-supplied [`TextRenderer`].  Reuse the renderer across
/// calls to amortize the font-loading cost.
///
/// v3.32.0: after rasterizing the display list, also stamps every
/// decoded image in `frame.decoded_images()` at its matching
/// `<img>` layout-box rect.  Frames built via [`crate::render`]
/// have an empty decoded-image map; frames built via
/// [`crate::render_with_inline_assets`] carry the decoded data:
/// PNGs.
#[must_use]
pub fn render_to_pixels_with(
    frame: &Frame,
    width: u32,
    height: u32,
    text_renderer: &mut TextRenderer,
) -> PixelBuffer {
    let bytes = Pixmap::new(width, height).map_or_else(
        || empty_bytes(width, height),
        |pixmap| {
            let pixmap =
                paint_commands_into(pixmap, frame.display_list().commands(), text_renderer);
            let pixmap = stamp_decoded_images(pixmap, frame);
            pixmap.take()
        },
    );
    PixelBuffer {
        width,
        height,
        bytes,
    }
}

fn paint_commands_into(
    pixmap_in: Pixmap,
    commands: &[PaintCommand],
    text_renderer: &mut TextRenderer,
) -> Pixmap {
    let mut pixmap = pixmap_in;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);
    #[allow(clippy::needless_for_each)]
    commands.iter().for_each(|cmd| {
        apply_command(&mut pixmap, cmd, text_renderer);
    });
    pixmap
}

fn stamp_decoded_images(pixmap_in: Pixmap, frame: &Frame) -> Pixmap {
    let mut pixmap = pixmap_in;
    let decoded = frame.decoded_images();
    if decoded.is_empty() {
        return pixmap;
    }
    if let Some(root) = frame.layout_tree().root_box() {
        walk_layout_boxes(root, &mut |layout_box| {
            if let Some(decoded_image) = decoded.get(&layout_box.dom_node()) {
                stamp_one_image(&mut pixmap, &layout_box.rect(), decoded_image);
            }
        });
    }
    pixmap
}

fn walk_layout_boxes<F: FnMut(&LayoutBox)>(layout_box: &LayoutBox, callback: &mut F) {
    callback(layout_box);
    #[allow(clippy::needless_for_each)]
    layout_box.children().iter().for_each(|child| {
        walk_layout_boxes(child, callback);
    });
}

fn stamp_one_image(pixmap: &mut Pixmap, target: &layout_cat::Rect, decoded: &DecodedImage) {
    let Some(source_pixmap) = pixmap_from_decoded(decoded) else {
        return;
    };
    let Some(target_rect) = rect_to_skia(target) else {
        return;
    };
    let scale_x = target_rect.width() / u32_to_f32_lossy(source_pixmap.width().max(1));
    let scale_y = target_rect.height() / u32_to_f32_lossy(source_pixmap.height().max(1));
    let transform =
        Transform::from_scale(scale_x, scale_y).post_translate(target_rect.x(), target_rect.y());
    let paint = PixmapPaint {
        opacity: 1.0,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, source_pixmap.as_ref(), &paint, transform, None);
}

fn pixmap_from_decoded(decoded: &DecodedImage) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(decoded.width(), decoded.height())?;
    let dest = pixmap.data_mut();
    // tiny-skia uses premultiplied alpha; the decoded buffer is
    // straight alpha.  Premultiply on copy.
    dest.chunks_exact_mut(4)
        .zip(decoded.rgba().chunks_exact(4))
        .for_each(|(out, src)| {
            let alpha = u32::from(src[3]);
            let premul = |c: u8| u8::try_from(u32::from(c) * alpha / 255).unwrap_or(0);
            out[0] = premul(src[0]);
            out[1] = premul(src[1]);
            out[2] = premul(src[2]);
            out[3] = src[3];
        });
    Some(pixmap)
}

/// Rasterize a slice of [`PaintCommand`]s into an RGBA pixel buffer.
/// Useful when the caller needs to mutate or scale the display list
/// before rasterization (see the v3 zoom path in
/// `crate::tauri_impl`).
#[must_use]
pub fn render_commands_to_pixels_with(
    commands: &[PaintCommand],
    width: u32,
    height: u32,
    text_renderer: &mut TextRenderer,
) -> PixelBuffer {
    let bytes = rasterize_to_bytes(commands, width, height, text_renderer);
    PixelBuffer {
        width,
        height,
        bytes,
    }
}

fn rasterize_to_bytes(
    commands: &[PaintCommand],
    width: u32,
    height: u32,
    text_renderer: &mut TextRenderer,
) -> Vec<u8> {
    Pixmap::new(width, height).map_or_else(
        || empty_bytes(width, height),
        |pixmap| paint_commands_into(pixmap, commands, text_renderer).take(),
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

fn apply_command(pixmap: &mut Pixmap, command: &PaintCommand, text_renderer: &mut TextRenderer) {
    match command {
        PaintCommand::FillRect { rect, color } => fill_rect(pixmap, rect, color),
        PaintCommand::StrokeRect { rect, color, width } => stroke_rect(pixmap, rect, color, *width),
        PaintCommand::FillText {
            rect,
            text,
            color,
            font_size,
        } => text_renderer.render_text(pixmap, *rect, text, *color, *font_size),
    }
}

fn fill_rect(pixmap: &mut Pixmap, rect: &layout_cat::Rect, color: &layout_cat::Color) {
    if let Some(skia_rect) = rect_to_skia(rect) {
        let paint = build_paint(color);
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

fn u32_to_f32_lossy(value: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let n = value as f32;
    n
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
