//! Pipeline driver: HTML + CSS + viewport -> [`Frame`].

use boa_cat::Value;
use boa_cat::heap::Heap;
use layout_cat::Viewport;

use crate::error::Error;
use crate::frame::Frame;

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
