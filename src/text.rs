//! Text shaping + glyph rasterization via cosmic-text + swash.
//!
//! v0.3 limitations:
//!
//! - Mask content only (no color glyphs / emoji raster in v0.3).
//! - Default `FontSystem` config; loads system fonts on construction
//!   which can be slow on first use.  Tests amortize across cases by
//!   sharing a single [`TextRenderer`] per test.
//! - Single-line layout per `FillText` command; the box's content
//!   rect's width is the wrap width.
//! - No bidi reordering / shaping beyond what cosmic-text's
//!   `Shaping::Advanced` provides by default.

use cosmic_text::{
    Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent, SwashImage,
};
use tiny_skia::Pixmap;

use layout_cat::{Color, Rect};

/// A cached text renderer: holds a `FontSystem` and a `SwashCache`.
/// Building one is expensive (loads system fonts); reuse it across
/// calls.
pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl std::fmt::Debug for TextRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TextRenderer").finish()
    }
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    /// Build a renderer using the default cosmic-text configuration
    /// (system fonts via fontdb + fontconfig where available).
    #[must_use]
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Render `text` into `pixmap` within `rect` using `color` and
    /// `font_size`.
    pub fn render_text(
        &mut self,
        pixmap: &mut Pixmap,
        rect: Rect,
        text: &str,
        color: Color,
        font_size: f64,
    ) {
        if !text.is_empty() && font_size > 0.0 {
            self.shape_and_paint(pixmap, rect, text, color, font_size);
        }
    }

    fn shape_and_paint(
        &mut self,
        pixmap: &mut Pixmap,
        rect: Rect,
        text: &str,
        color: Color,
        font_size: f64,
    ) {
        let metrics = Metrics::new(f32_from_f64(font_size), f32_from_f64(font_size * 1.2));
        // FFI carve-out: cosmic-text's `Buffer::new` and
        // `shape_until_scroll` take `&mut FontSystem`.
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let attrs = Attrs::new();
        buffer.set_size(
            Some(f32_from_f64(rect.width())),
            Some(f32_from_f64(rect.height())),
        );
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, true);
        let unit_color = UnitColor::from_color(color);
        let pixmap_width = pixmap.width();
        let pixmap_height = pixmap.height();
        let bytes_per_row = usize::try_from(pixmap_width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .unwrap_or(0);
        let origin_x = f32_from_f64(rect.origin().x());
        let origin_y = f32_from_f64(rect.origin().y());
        // FFI carve-out: passing a mutable wrapper through nested
        // `for_each` closures keeps `blit_mask`'s signature under the
        // clippy arg-count limit without using interior mutability.
        let mut view = PixmapView {
            data: pixmap.data_mut(),
            width: pixmap_width,
            height: pixmap_height,
            bytes_per_row,
        };
        buffer.layout_runs().for_each(|run| {
            run.glyphs.iter().for_each(|glyph| {
                let physical = glyph.physical((origin_x, origin_y + run.line_y), 1.0);
                let image_opt = self
                    .swash_cache
                    .get_image(&mut self.font_system, physical.cache_key);
                let _ = image_opt.as_ref().map(|image| {
                    blit_mask(&mut view, image, (physical.x, physical.y), unit_color);
                });
            });
        });
    }
}

#[derive(Clone, Copy)]
struct UnitColor {
    red: f64,
    green: f64,
    blue: f64,
    alpha: f64,
}

impl UnitColor {
    fn from_color(color: Color) -> Self {
        Self {
            red: clamp_unit(color.red()),
            green: clamp_unit(color.green()),
            blue: clamp_unit(color.blue()),
            alpha: clamp_unit(color.alpha()),
        }
    }
}

struct PixmapView<'a> {
    data: &'a mut [u8],
    width: u32,
    height: u32,
    bytes_per_row: usize,
}

fn blit_mask(view: &mut PixmapView<'_>, image: &SwashImage, origin: (i32, i32), color: UnitColor) {
    let _ = matches!(image.content, SwashContent::Mask)
        .then_some(image.placement)
        .filter(|placement| placement.width > 0 && placement.height > 0)
        .map(|placement| {
            let (origin_x, origin_y) = origin;
            let left = origin_x + placement.left;
            let top = origin_y - placement.top;
            (0..placement.height).for_each(|row| {
                (0..placement.width).for_each(|col| {
                    let _ = plot_pixel(view, image, (left, top), (row, col), color);
                });
            });
        });
}

fn plot_pixel(
    view: &mut PixmapView<'_>,
    image: &SwashImage,
    origin: (i32, i32),
    pixel: (u32, u32),
    color: UnitColor,
) -> Option<()> {
    let (left, top) = origin;
    let (row, col) = pixel;
    let glyph_width = image.placement.width;
    let dst_x = u32::try_from(left.checked_add(i32_from_u32(col))?).ok()?;
    let dst_y = u32::try_from(top.checked_add(i32_from_u32(row))?).ok()?;
    (dst_x < view.width).then_some(())?;
    (dst_y < view.height).then_some(())?;
    let row_stride = usize::try_from(glyph_width).ok()?;
    let row_index = usize::try_from(row).ok()?;
    let col_index = usize::try_from(col).ok()?;
    let src_index = row_index.checked_mul(row_stride)?.checked_add(col_index)?;
    let mask = image.data.get(src_index).copied().map(unit_from_byte)?;
    let src_alpha = color.alpha * mask;
    (src_alpha > 0.0).then_some(())?;
    let dst_y_usize = usize::try_from(dst_y).ok()?;
    let dst_x_usize = usize::try_from(dst_x).ok()?;
    let dst_offset = dst_y_usize
        .checked_mul(view.bytes_per_row)?
        .checked_add(dst_x_usize.checked_mul(4)?)?;
    blend_premul_over(view.data, dst_offset, color, src_alpha)
}

fn blend_premul_over(
    pixmap_data: &mut [u8],
    offset: usize,
    color: UnitColor,
    src_alpha: f64,
) -> Option<()> {
    let end = offset.checked_add(4)?;
    let (dst_red, dst_green, dst_blue, dst_alpha) = {
        let dst_bytes = pixmap_data.get(offset..end)?;
        let red = dst_bytes.first().copied().map(unit_from_byte)?;
        let green = dst_bytes.get(1).copied().map(unit_from_byte)?;
        let blue = dst_bytes.get(2).copied().map(unit_from_byte)?;
        let alpha = dst_bytes.get(3).copied().map(unit_from_byte)?;
        (red, green, blue, alpha)
    };
    let inv = 1.0 - src_alpha;
    let blended = [
        u8_from_unit(color.red * src_alpha + dst_red * inv),
        u8_from_unit(color.green * src_alpha + dst_green * inv),
        u8_from_unit(color.blue * src_alpha + dst_blue * inv),
        u8_from_unit(src_alpha + dst_alpha * inv),
    ];
    let slot = pixmap_data.get_mut(offset..end)?;
    slot.iter_mut()
        .zip(blended.iter())
        .for_each(|(out, value)| *out = *value);
    Some(())
}

fn unit_from_byte(byte: u8) -> f64 {
    f64::from(byte) / 255.0
}

fn clamp_unit(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn u8_from_unit(value: f64) -> u8 {
    let scaled = (value.clamp(0.0, 1.0) * 255.0).round();
    // Lossy but bounded by the clamp + round above.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = scaled as u8;
    byte
}

fn i32_from_u32(value: u32) -> i32 {
    // Glyph widths/heights fit easily in i32 in any practical case;
    // clamp at i32::MAX defensively.
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn f32_from_f64(value: f64) -> f32 {
    if value.is_finite() {
        // f64 -> f32 narrowing for the cosmic-text API boundary.
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = value as f32;
        narrowed
    } else if value > 0.0 {
        f32::INFINITY
    } else if value < 0.0 {
        f32::NEG_INFINITY
    } else {
        0.0
    }
}
