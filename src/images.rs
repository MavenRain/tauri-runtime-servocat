//! v3.32.0 image-asset pipeline: decode inline `data:` PNG image
//! URLs from `<img>` element attributes during `render()` so the
//! rasterizer can stamp the decoded pixels at the image's layout
//! position.
//!
//! Currently supports:
//!
//! - `data:image/png;base64,...` URLs decoded inline (no I/O).
//!
//! Deferred:
//!
//! - File-path / file:// URLs (need a base-dir argument on the
//!   render entry point).
//! - JPEG (would add `jpeg-decoder`).
//! - HTTP fetch via net-cat (needs async coordination with the
//!   render loop).

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use dom_cat::{Document, NodeId};

/// A successfully decoded image: 8-bit RGBA pixel buffer with
/// non-premultiplied alpha (rasterizer premultiplies on stamp).
#[derive(Debug, Clone)]
pub struct DecodedImage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl DecodedImage {
    /// Construct from decoded RGBA pixels.  Panics in debug if the
    /// byte length does not match `width * height * 4`.
    #[must_use]
    pub fn new(rgba: Vec<u8>, width: u32, height: u32) -> Self {
        debug_assert_eq!(
            rgba.len(),
            usize::try_from(width)
                .ok()
                .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
                .and_then(|p| p.checked_mul(4))
                .unwrap_or(0)
        );
        Self {
            rgba,
            width,
            height,
        }
    }

    /// Raw RGBA bytes (8 bits per channel, non-premultiplied).
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Decoded image width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Decoded image height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Walk `dom` for `<img>` elements with `data:image/png;base64,...`
/// src attributes and return a `NodeId -> DecodedImage` map.  Other
/// schemes (file paths, http URLs) are silently skipped for now.
#[must_use]
pub fn collect_inline_images(dom: &Document) -> BTreeMap<NodeId, DecodedImage> {
    dom_cat::query_selector_all(dom, "img")
        .unwrap_or_default()
        .iter()
        .filter_map(|id| {
            let src = dom
                .get(*id)
                .and_then(dom_cat::Node::as_element)
                .and_then(|element| element.attribute("src"))?;
            decode_data_url(src).map(|decoded| (*id, decoded))
        })
        .collect()
}

/// Decode a `data:image/png;base64,...` URL into RGBA pixels.
/// Returns `None` for unsupported schemes, missing base64, decode
/// errors, or anything else that cannot resolve to a PNG.
#[must_use]
pub fn decode_data_url(src: &str) -> Option<DecodedImage> {
    let rest = src.strip_prefix("data:image/png;base64,")?;
    let bytes = STANDARD.decode(rest.trim()).ok()?;
    decode_png_bytes(&bytes)
}

/// Decode raw PNG bytes into 8-bit RGBA.  Handles grayscale,
/// indexed, RGB-without-alpha, and 16-bit depth via the
/// `Transformations` flags so the output is always RGBA8.
#[must_use]
pub fn decode_png_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());
    let expected = usize::try_from(info.width)
        .ok()
        .and_then(|w| {
            usize::try_from(info.height)
                .ok()
                .and_then(|h| w.checked_mul(h))
        })
        .and_then(|p| p.checked_mul(4))?;
    (buffer.len() == expected).then(|| DecodedImage::new(buffer, info.width, info.height))
}
