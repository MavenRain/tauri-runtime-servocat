//! Script driver: HTML + CSS + JS + viewport -> [`Frame`] with the JS
//! return value populated.  v0.5 adds two variants:
//!
//! - [`run_script_with_commands`] installs a [`HostCommands`] registry
//!   as `__TAURI__` so scripts can call into the host.
//! - [`run_script_with_backprop`] additionally walks the post-script
//!   JS-side DOM via [`web_api_cat::extract_document`] and re-runs
//!   layout + paint so scripted DOM mutations are reflected in the
//!   returned [`Frame`]'s display list.

use boa_cat::Value;
use boa_cat::env::{Binding, Env};
use boa_cat::evaluate_program_with;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap;
use ecma_lex_cat::lex;
use ecma_parse_cat::parse_script;
use layout_cat::Viewport;
use web_api_cat::extract_document;

use crate::error::Error;
use crate::frame::Frame;
use crate::ipc::HostCommands;

/// Default step budget used by the script-driver helpers.
pub const DEFAULT_FUEL: u64 = 1_000_000;

/// Like [`crate::pipeline::render`] but with an inline JS step that
/// runs against the parsed document via boa-cat + ecma-runtime-cat +
/// web-api-cat.
///
/// The JS value of the script's final expression lands in
/// [`Frame::script_value`].  Script mutations to the JS-side DOM are
/// visible in [`Frame::script_heap`] but are NOT back-propagated into
/// the layout pass.  Use [`run_script_with_backprop`] for that.
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
    run_script_with_commands(
        html_source,
        css_source,
        js_source,
        viewport,
        &HostCommands::new(),
    )
}

/// Variant of [`run_script`] that installs a [`HostCommands`] registry
/// as the JS global `__TAURI__`.  Scripts can then call host commands
/// either as `__TAURI__.cmd(...args)` (direct method) or via
/// `__TAURI__.invoke("cmd", ...args)` (dispatcher).
///
/// As with [`run_script`], DOM mutations are NOT back-propagated.
///
/// # Errors
///
/// Propagates parser, lexer, and JS engine errors.
pub fn run_script_with_commands(
    html_source: &str,
    css_source: &str,
    js_source: &str,
    viewport: Viewport,
    commands: &HostCommands,
) -> Result<Frame, Error> {
    let prepared = prepare(html_source, css_source, viewport, commands)?;
    let (value, heap) = evaluate(js_source, prepared.env, prepared.heap)?;
    Ok(Frame::new(
        prepared.dom,
        prepared.layout_tree,
        prepared.display_list,
        value,
        heap,
    ))
}

/// Variant of [`run_script_with_commands`] that walks the post-script
/// JS-side DOM via [`web_api_cat::extract_document`] and re-runs
/// layout + paint, so the returned [`Frame`]'s display list reflects
/// scripted mutations (`setAttribute`, etc.).
///
/// Falls back to the pre-script layout if `extract_document` returns
/// `None` (e.g. the JS state isn't a recognisable document tree any
/// more).  The script value and heap on the returned `Frame` are
/// always the post-script ones.
///
/// # Errors
///
/// Propagates parser, lexer, and JS engine errors.
pub fn run_script_with_backprop(
    html_source: &str,
    css_source: &str,
    js_source: &str,
    viewport: Viewport,
    commands: &HostCommands,
) -> Result<Frame, Error> {
    run_script_with_cookies(html_source, css_source, js_source, viewport, commands, "")
        .map(|(frame, _post_eval)| frame)
}

/// Variant of [`run_script_with_backprop`] that also synchronizes a
/// cookie string in and out of `document.cookie`.  `cookie_string`
/// is pushed into `document.cookie` via
/// [`web_api_cat::set_document_cookie`] before the JS step, and the
/// post-script value of `document.cookie` is returned alongside the
/// frame so the caller can parse + merge any writes JS made.
///
/// The caller is responsible for the JS-visibility filter on
/// `cookie_string` (e.g. dropping `HttpOnly` entries) and for parsing
/// the post-eval string back into cookie objects.  This function
/// only shuttles strings.
///
/// # Errors
///
/// Propagates parser, lexer, and JS engine errors.
pub fn run_script_with_cookies(
    html_source: &str,
    css_source: &str,
    js_source: &str,
    viewport: Viewport,
    commands: &HostCommands,
    cookie_string: &str,
) -> Result<(Frame, Option<String>), Error> {
    let prepared = prepare(html_source, css_source, viewport, commands)?;
    let document_value = prepared.document_value.clone();
    let stylesheet = prepared.stylesheet.clone();
    let heap = web_api_cat::set_document_cookie(&document_value, prepared.heap, cookie_string);
    let (value, heap) = evaluate(js_source, prepared.env, heap)?;
    let post_eval = web_api_cat::get_document_cookie(&document_value, &heap);
    let (dom, layout_tree, display_list) = extract_document(&document_value, &heap)
        .map(|extracted_dom| {
            let new_layout = layout_cat::layout(&extracted_dom, &stylesheet, viewport);
            let new_display = paint_cat::build(&new_layout, &extracted_dom);
            (extracted_dom, new_layout, new_display)
        })
        .unwrap_or((prepared.dom, prepared.layout_tree, prepared.display_list));
    Ok((
        Frame::new(dom, layout_tree, display_list, value, heap),
        post_eval,
    ))
}

struct Prepared {
    env: Env,
    heap: Heap,
    document_value: Value,
    dom: dom_cat::Document,
    stylesheet: css_cat::Stylesheet,
    layout_tree: layout_cat::LayoutTree,
    display_list: paint_cat::DisplayList,
}

fn prepare(
    html_source: &str,
    css_source: &str,
    viewport: Viewport,
    commands: &HostCommands,
) -> Result<Prepared, Error> {
    let html_doc = html_cat::parse(html_source)?;
    let dom = dom_cat::Document::from_html_doc(&html_doc);
    let stylesheet = css_cat::parse(css_source)?;
    let layout_tree = layout_cat::layout(&dom, &stylesheet, viewport);
    let display_list = paint_cat::build(&layout_tree, &dom);
    let (env, heap) = ecma_runtime_cat::install(Env::empty(), Heap::new());
    let (env, heap) = web_api_cat::install(env, heap, &html_doc);
    let (env, heap) = commands.install(env, heap);
    let document_value =
        lookup_document(&env, &heap).ok_or(Error::Engine(boa_cat::Error::Unsupported {
            feature: "document binding missing after web_api_cat::install",
        }))?;
    Ok(Prepared {
        env,
        heap,
        document_value,
        dom,
        stylesheet,
        layout_tree,
        display_list,
    })
}

fn evaluate(js_source: &str, env: Env, heap: Heap) -> Result<(Value, Heap), Error> {
    let tokens = lex(js_source).map_err(boa_cat::Error::from)?;
    let program = parse_script(&tokens).map_err(boa_cat::Error::from)?;
    let (value, heap) = evaluate_program_with(&program, env, heap, Fuel::new(DEFAULT_FUEL))?;
    Ok((value, heap))
}

fn lookup_document(env: &Env, heap: &Heap) -> Option<Value> {
    env.lookup("document").and_then(|binding| match binding {
        Binding::Cell(cell_id) => heap.cell(*cell_id).map(|cell| cell.value().clone()),
        Binding::Direct(value) => Some(value.clone()),
    })
}
