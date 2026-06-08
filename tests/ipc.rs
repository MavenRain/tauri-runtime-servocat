//! v0.5 integration tests: host command invocation + DOM back-prop.

use boa_cat::Value;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap;
use boa_cat::outcome::{EvalResult, Outcome};
use paint_cat::PaintCommand;
use tauri_runtime_servocat::{
    Error, HostCommands, Viewport, run_script_with_backprop, run_script_with_commands,
};

fn fail(_msg: &'static str) -> Error {
    Error::Engine(boa_cat::Error::Unsupported { feature: "test" })
}

#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn echo_upper(args: Vec<Value>, _this: Value, heap: Heap, fuel: Fuel) -> EvalResult {
    let text = args
        .first()
        .and_then(|first| match first {
            Value::String(s) => Some(s.clone()),
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::Number(_)
            | Value::Object(_)
            | Value::Function(_)
            | Value::Native(_) => None,
        })
        .unwrap_or_default();
    let upper = text.to_uppercase();
    Ok((Outcome::Normal(Value::String(upper)), heap, fuel))
}

#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn add(args: Vec<Value>, _this: Value, heap: Heap, fuel: Fuel) -> EvalResult {
    let total: f64 = args
        .iter()
        .filter_map(|value| match value {
            Value::Number(n) => Some(*n),
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::String(_)
            | Value::Object(_)
            | Value::Function(_)
            | Value::Native(_) => None,
        })
        .sum();
    Ok((Outcome::Normal(Value::Number(total)), heap, fuel))
}

#[test]
fn host_command_invoked_via_dispatcher() -> Result<(), Error> {
    let commands = HostCommands::new().with("shout", echo_upper);
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "__TAURI__.invoke('shout', 'hello')",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "HELLO")
        .then_some(())
        .ok_or_else(|| fail("expected 'HELLO' from __TAURI__.invoke"))
}

#[test]
fn host_command_invoked_as_method() -> Result<(), Error> {
    let commands = HostCommands::new().with("shout", echo_upper);
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "__TAURI__.shout('hi there')",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "HI THERE")
        .then_some(())
        .ok_or_else(|| fail("expected 'HI THERE' from __TAURI__.shout"))
}

#[test]
fn host_command_with_multiple_args() -> Result<(), Error> {
    let commands = HostCommands::new().with("add", add);
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "__TAURI__.invoke('add', 1, 2, 3, 4)",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::Number(n) if (*n - 10.0).abs() < 1e-9)
        .then_some(())
        .ok_or_else(|| fail("expected 10 from add(1,2,3,4)"))
}

#[test]
fn unknown_command_returns_null() -> Result<(), Error> {
    let commands = HostCommands::new();
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "__TAURI__.invoke('nope', 'x')",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::Null)
        .then_some(())
        .ok_or_else(|| fail("expected null for unknown command"))
}

#[test]
fn backprop_picks_up_set_attribute_class_change() -> Result<(), Error> {
    let frame = run_script_with_backprop(
        "<html><body><p id='p'>hello</p></body></html>",
        "p { background-color: red; height: 30px; } p.live { background-color: green; }",
        "document.getElementById('p').setAttribute('class', 'live')",
        Viewport::new(200, 100),
        &HostCommands::new(),
    )?;
    let has_green = frame.display_list().commands().iter().any(|cmd| match cmd {
        PaintCommand::FillRect { color, .. } => {
            (color.green() - 0.5).abs() < 0.5
                && color.red() < 0.1
                && color.blue() < 0.1
                && color.green() > 0.4
        }
        PaintCommand::StrokeRect { .. } | PaintCommand::FillText { .. } => false,
    });
    has_green
        .then_some(())
        .ok_or_else(|| fail("expected a green FillRect after class mutation back-prop"))
}

#[test]
fn backprop_falls_back_when_no_mutation() -> Result<(), Error> {
    let frame_no_script = run_script_with_backprop(
        "<html><body><p>hi</p></body></html>",
        "p { background-color: red; height: 20px; }",
        "1",
        Viewport::new(100, 50),
        &HostCommands::new(),
    )?;
    (!frame_no_script.display_list().is_empty())
        .then_some(())
        .ok_or_else(|| fail("expected non-empty display list with no-op script"))
}

#[test]
fn host_command_invoked_via_global_invoke() -> Result<(), Error> {
    // v3.14: top-level `invoke(cmd, ...args)` dispatches through the
    // thread-local registry, no `this` required.
    let commands = HostCommands::new().with("shout", echo_upper);
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "invoke('shout', 'hi')",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::String(s) if s == "HI")
        .then_some(())
        .ok_or_else(|| fail("expected 'HI' from global invoke"))
}

#[test]
fn global_invoke_survives_being_rebound_to_a_variable() -> Result<(), Error> {
    // The whole reason for v3.14: `const i = invoke; i(...)` should
    // still dispatch even though `this` is no longer __TAURI__.
    let commands = HostCommands::new().with("add", add);
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "const i = invoke; i('add', 2, 3, 5)",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::Number(n) if (*n - 10.0).abs() < 1e-9)
        .then_some(())
        .ok_or_else(|| fail("rebound invoke should still hit the registry"))
}

#[test]
fn global_invoke_unknown_command_returns_null() -> Result<(), Error> {
    let commands = HostCommands::new();
    let frame = run_script_with_commands(
        "<html><body></body></html>",
        "",
        "invoke('missing', 1)",
        Viewport::new(100, 100),
        &commands,
    )?;
    matches!(frame.script_value(), Value::Null)
        .then_some(())
        .ok_or_else(|| fail("unknown command via global invoke should yield null"))
}
