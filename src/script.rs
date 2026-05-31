//! Script driver: HTML + CSS + JS + viewport -> [`Frame`] with the JS
//! return value populated.

use boa_cat::env::Env;
use boa_cat::evaluate_program_with;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap;
use ecma_lex_cat::lex;
use ecma_parse_cat::parse_script;
use layout_cat::Viewport;

use crate::error::Error;
use crate::frame::Frame;

/// Default step budget used by [`run_script`].
pub const DEFAULT_FUEL: u64 = 1_000_000;

/// Like [`crate::pipeline::render`] but with an inline JS step that
/// runs against the parsed document via boa-cat + ecma-runtime-cat +
/// web-api-cat.
///
/// The JS value of the script's final expression lands in
/// [`Frame::script_value`].  Script mutations to the JS-side DOM are
/// visible in [`Frame::script_heap`] but are NOT yet back-propagated
/// into the layout pass (v0.5+).
///
/// # Errors
///
/// Propagates parser, lexer, and JS engine errors.
pub fn run_script(
    html_source: &str,
    css_source: &str,
    js_source: &str,
    viewport: Viewport,
) -> Result<Frame, Error> {
    let html_doc = html_cat::parse(html_source)?;
    let dom = dom_cat::Document::from_html_doc(&html_doc);
    let stylesheet = css_cat::parse(css_source)?;
    let layout_tree = layout_cat::layout(&dom, &stylesheet, viewport);
    let display_list = paint_cat::build(&layout_tree, &dom);
    let (env, heap) = ecma_runtime_cat::install(Env::empty(), Heap::new());
    let (env, heap) = web_api_cat::install(env, heap, &html_doc);
    let tokens = lex(js_source).map_err(boa_cat::Error::from)?;
    let program = parse_script(&tokens).map_err(boa_cat::Error::from)?;
    let (value, heap) = evaluate_program_with(&program, env, heap, Fuel::new(DEFAULT_FUEL))?;
    Ok(Frame::new(dom, layout_tree, display_list, value, heap))
}
