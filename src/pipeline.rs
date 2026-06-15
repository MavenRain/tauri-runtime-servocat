//! Pipeline driver: HTML + CSS + viewport -> [`Frame`].

use boa_cat::Value;
use boa_cat::heap::Heap;
use layout_cat::Viewport;

use crate::error::Error;
use crate::frame::Frame;
use crate::images;

/// Parse `html` and `css`, build the DOM, run cascade + block layout,
/// emit the paint-cat display list, and wrap everything in a
/// [`Frame`].  The script-side fields are empty (`Value::Undefined`
/// and an empty heap).
///
/// # Errors
///
/// Propagates parser and layout errors from the cat-stack.
pub fn render(html_source: &str, css_source: &str, viewport: Viewport) -> Result<Frame, Error> {
    let html_doc = html_cat::parse(html_source)?;
    let dom = dom_cat::Document::from_html_doc(&html_doc);
    let stylesheet = css_cat::parse(css_source)?;
    let layout_tree = layout_cat::layout(&dom, &stylesheet, viewport);
    let display_list = paint_cat::build(&layout_tree, &dom);
    Ok(Frame::new(
        dom,
        layout_tree,
        display_list,
        Value::Undefined,
        Heap::new(),
    ))
}

/// v3.32.0: like [`render`] but also walks the DOM for `<img>`
/// elements with `data:image/png;base64,...` src attributes,
/// decodes each, and attaches the resulting `NodeId ->
/// DecodedImage` map to the frame.  The rasterizer
/// ([`crate::raster::render_to_pixels_with`]) stamps each decoded
/// image at the matching layout rect.
///
/// File-path / file:// / http: URLs are silently skipped for now
/// -- a follow-up will add filesystem + net-cat fetch paths once
/// the async coordination story is settled.
///
/// # Errors
///
/// Propagates parser and layout errors from [`render`].  Image
/// decode failures are non-fatal (the offending `<img>` simply
/// renders as the background, the rest of the frame is unaffected).
pub fn render_with_inline_assets(
    html_source: &str,
    css_source: &str,
    viewport: Viewport,
) -> Result<Frame, Error> {
    let frame = render(html_source, css_source, viewport)?;
    let decoded_images = images::collect_inline_images(frame.document());
    Ok(frame.with_decoded_images(decoded_images))
}
