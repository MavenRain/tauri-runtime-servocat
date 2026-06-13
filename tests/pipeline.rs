//! Integration tests covering the headless pipeline + script driver.

#![allow(clippy::float_cmp)]

use boa_cat::Value;
use paint_cat::PaintCommand;
use tauri_runtime_servocat::{
    Error, HostCommands, Viewport, render, run_script, run_script_with_cookies,
    run_script_with_state,
};

fn fail(_msg: &'static str) -> Error {
    Error::Engine(boa_cat::Error::Unsupported { feature: "test" })
}

#[test]
fn render_produces_display_list() -> Result<(), Error> {
    let frame = render(
        "<html><body><p>hi</p></body></html>",
        "p { background-color: red; }",
        Viewport::new(800, 600),
    )?;
    (!frame.display_list().is_empty())
        .then_some(())
        .ok_or_else(|| fail("expected non-empty display list"))
}

#[test]
fn render_background_emits_fill_rect() -> Result<(), Error> {
    let frame = render(
        "<html><body><div></div></body></html>",
        "div { background-color: red; height: 100px; }",
        Viewport::new(800, 600),
    )?;
    frame
        .display_list()
        .commands()
        .iter()
        .any(|c| matches!(c, PaintCommand::FillRect { .. }))
        .then_some(())
        .ok_or_else(|| fail("expected FillRect"))
}

#[test]
fn render_empty_doc_empty_or_minimal_list() -> Result<(), Error> {
    let frame = render("", "", Viewport::new(800, 600))?;
    let _ = frame;
    Ok(())
}

#[test]
fn render_viewport_width_propagates_to_body() -> Result<(), Error> {
    let frame = render(
        "<html><body><p>x</p></body></html>",
        "",
        Viewport::new(1024, 768),
    )?;
    let body = frame
        .layout_tree()
        .root_box()
        .ok_or_else(|| fail("no root"))?;
    (body.rect().width() == 1024.0)
        .then_some(())
        .ok_or_else(|| fail("body should fill viewport width"))
}

#[test]
fn run_script_returns_dom_value() -> Result<(), Error> {
    let frame = run_script(
        "<html><body><p id='g'>hi</p></body></html>",
        "",
        "document.getElementById('g').textContent",
        Viewport::new(800, 600),
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "hi")
        .then_some(())
        .ok_or_else(|| fail("expected 'hi' from script"))
}

#[test]
fn run_script_with_math() -> Result<(), Error> {
    let frame = run_script(
        "<html><body></body></html>",
        "",
        "Math.floor(3.7) + Math.abs(-5)",
        Viewport::new(800, 600),
    )?;
    matches!(frame.script_value(), Value::Number(n) if (n - 8.0).abs() < 1e-9)
        .then_some(())
        .ok_or_else(|| fail("expected 8"))
}

#[test]
fn run_script_with_attribute_mutation() -> Result<(), Error> {
    let frame = run_script(
        "<html><body><p id='p'>x</p></body></html>",
        "",
        "const el = document.getElementById('p'); el.setAttribute('data-x', '42'); el.getAttribute('data-x')",
        Viewport::new(800, 600),
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "42")
        .then_some(())
        .ok_or_else(|| fail("expected '42'"))
}

#[test]
fn run_script_query_selector_class() -> Result<(), Error> {
    let frame = run_script(
        "<html><body><p class='a'>1</p><p class='b'>2</p></body></html>",
        "",
        "document.querySelector('.b').textContent",
        Viewport::new(800, 600),
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "2")
        .then_some(())
        .ok_or_else(|| fail("expected '2'"))
}

#[test]
fn run_script_layout_still_produced() -> Result<(), Error> {
    let frame = run_script(
        "<html><body><p>x</p></body></html>",
        "p { height: 50px; }",
        "1",
        Viewport::new(800, 600),
    )?;
    (!frame.display_list().is_empty())
        .then_some(())
        .ok_or_else(|| fail("expected display list alongside script"))
}

#[test]
fn document_cookie_seeded_value_visible_to_js() -> Result<(), Error> {
    let (frame, _writes) = run_script_with_cookies(
        "<html></html>",
        "",
        "document.cookie",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "session=abc",
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "session=abc")
        .then_some(())
        .ok_or_else(|| fail("expected the seeded cookie string from JS"))
}

#[test]
fn document_cookie_js_write_reaches_write_log() -> Result<(), Error> {
    // v3.11: writes surface as a per-write log (one entry per
    // `document.cookie = "..."` statement, attributes intact)
    // instead of one post-eval string.
    let (_frame, writes) = run_script_with_cookies(
        "<html></html>",
        "",
        "document.cookie = 'theme=dark'",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
    )?;
    (writes == vec!["theme=dark".to_owned()])
        .then_some(())
        .ok_or_else(|| fail("write log should carry the JS-written value"))
}

#[test]
fn document_cookie_multiple_attribute_writes_log_in_order() -> Result<(), Error> {
    let (_frame, writes) = run_script_with_cookies(
        "<html></html>",
        "",
        "document.cookie = 'a=1; Path=/'; document.cookie = 'b=2; Max-Age=600'",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
    )?;
    (writes == vec!["a=1; Path=/".to_owned(), "b=2; Max-Age=600".to_owned()])
        .then_some(())
        .ok_or_else(|| fail("write log must keep order + attributes per entry"))
}

#[test]
fn local_storage_seeded_value_visible_to_js() -> Result<(), Error> {
    let local_seed = vec![("theme".to_owned(), "dark".to_owned())];
    let outcome = run_script_with_state(
        "<html></html>",
        "",
        "localStorage.getItem('theme')",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
        &local_seed,
        &[],
    )?;
    matches!(outcome.frame().script_value(), Value::String(s) if s == "dark")
        .then_some(())
        .ok_or_else(|| fail("expected seeded localStorage entry to be readable from JS"))
}

#[test]
fn local_storage_js_setitem_lands_in_host_snapshot() -> Result<(), Error> {
    let outcome = run_script_with_state(
        "<html></html>",
        "",
        "localStorage.setItem('alpha', 'one'); localStorage.setItem('beta', 'two');",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
        &[],
        &[],
    )?;
    (outcome.local_storage()
        == [
            ("alpha".to_owned(), "one".to_owned()),
            ("beta".to_owned(), "two".to_owned()),
        ])
    .then_some(())
    .ok_or_else(|| fail("expected setItem writes to surface in the post-eval snapshot"))
}

#[test]
fn local_storage_js_remove_item_reflects_in_snapshot() -> Result<(), Error> {
    let local_seed = vec![
        ("keep".to_owned(), "yes".to_owned()),
        ("drop".to_owned(), "x".to_owned()),
    ];
    let outcome = run_script_with_state(
        "<html></html>",
        "",
        "localStorage.removeItem('drop');",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
        &local_seed,
        &[],
    )?;
    (outcome.local_storage() == [("keep".to_owned(), "yes".to_owned())])
        .then_some(())
        .ok_or_else(|| fail("expected removeItem to drop the entry from the snapshot"))
}

#[test]
fn session_storage_is_independent_of_local() -> Result<(), Error> {
    let local_seed = vec![("k".to_owned(), "from-local".to_owned())];
    let session_seed = vec![("k".to_owned(), "from-session".to_owned())];
    let outcome = run_script_with_state(
        "<html></html>",
        "",
        "localStorage.getItem('k') + ',' + sessionStorage.getItem('k')",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
        &local_seed,
        &session_seed,
    )?;
    matches!(outcome.frame().script_value(), Value::String(s) if s == "from-local,from-session")
        .then_some(())
        .ok_or_else(|| fail("expected localStorage and sessionStorage to be independent"))
}

#[test]
fn session_storage_js_writes_land_in_host_snapshot() -> Result<(), Error> {
    let outcome = run_script_with_state(
        "<html></html>",
        "",
        "sessionStorage.setItem('once', 'now');",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "",
        &[],
        &[],
    )?;
    (outcome.session_storage() == [("once".to_owned(), "now".to_owned())])
        .then_some(())
        .ok_or_else(|| {
            fail("expected sessionStorage writes to surface separately from localStorage")
        })
}

#[test]
fn cookie_and_storage_seeds_compose_without_interference() -> Result<(), Error> {
    let local_seed = vec![("k".to_owned(), "v".to_owned())];
    let outcome = run_script_with_state(
        "<html></html>",
        "",
        "document.cookie = 'session=abc';
        localStorage.setItem('k2', 'v2');
        document.cookie + '|' + localStorage.getItem('k')",
        Viewport::new(400, 300),
        &HostCommands::new(),
        "user=anon",
        &local_seed,
        &[],
    )?;
    matches!(outcome.frame().script_value(), Value::String(s) if s == "user=anon; session=abc|v")
        .then_some(())
        .ok_or_else(|| fail("expected cookie seed + local seed both visible to JS"))?;
    (outcome.cookie_writes() == ["session=abc".to_owned()])
        .then_some(())
        .ok_or_else(|| fail("expected cookie write log to surface"))?;
    (outcome.local_storage()
        == [
            ("k".to_owned(), "v".to_owned()),
            ("k2".to_owned(), "v2".to_owned()),
        ])
    .then_some(())
    .ok_or_else(|| fail("expected local snapshot to merge seed + JS write"))
}

#[test]
fn js_accessor_literal_dispatches_through_engine() -> Result<(), Error> {
    // v3.12 picks up ecma-parse-cat 0.2 + boa-cat 0.3.1 so
    // user-written `{ get x() {} }` / `{ set x(v) {} }` literal
    // syntax reaches the engine's accessor-dispatch path that
    // web-api-cat's `document.cookie` already uses internally.
    // The script defines an inline accessor, writes through the
    // setter, reads back through the getter, and proves the
    // result reflects the setter's transformation -- end-to-end
    // proof that parser -> engine accessor wiring is live.
    let frame = run_script(
        "<html></html>",
        "",
        "let backing = 0;
        const o = {
            get x() { return backing; },
            set x(v) { backing = v * 3; }
        };
        o.x = 7;
        o.x",
        Viewport::new(400, 300),
    )?;
    matches!(frame.script_value(), Value::Number(n) if (n - 21.0).abs() < 1e-9)
        .then_some(())
        .ok_or_else(|| fail("inline accessor setter+getter should round-trip"))
}

#[test]
fn js_shorthand_method_dispatches_through_engine() -> Result<(), Error> {
    // Shorthand methods land as Method-kind AST members; the
    // engine handles them identically to data properties holding
    // a function value.  Calling `o.add(2, 3)` proves the bound
    // function value is reachable through normal property access.
    let frame = run_script(
        "<html></html>",
        "",
        "const o = { add(a, b) { return a + b; } }; o.add(2, 3)",
        Viewport::new(400, 300),
    )?;
    matches!(frame.script_value(), Value::Number(n) if (n - 5.0).abs() < 1e-9)
        .then_some(())
        .ok_or_else(|| fail("shorthand method should be callable"))
}
