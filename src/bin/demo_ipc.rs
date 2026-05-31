//! v0.5 demo binary: registers a host command, calls it from JS,
//! mutates the DOM via `setAttribute`, back-propagates the mutation
//! into layout, and shows the updated render in a window.
//!
//! Run with `cargo run --bin demo_ipc`.  Close the window to exit.

use boa_cat::Value;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap;
use boa_cat::outcome::{EvalResult, Outcome};
use tauri_runtime_servocat::{Error, HostCommands, Viewport, run_script_with_backprop, run_window};

const HTML: &str = "<html><body>\
<h1 id='title' class='dim'>tauri-runtime-servocat v0.5</h1>\
<p class='dim'>The host registered a `now` command.  The script called it and used the result to set the title's class to `live`, which the back-prop pass picked up.</p>\
</body></html>";

const CSS: &str = "
    body { background-color: white; padding: 24px; }
    h1.dim { color: gray; height: 48px; }
    h1.live { color: navy; background-color: yellow; height: 48px; }
    p.dim { color: gray; height: 28px; }
    p.live { color: black; height: 28px; }
";

const JS: &str = "
    const stamp = __TAURI__.invoke('now');
    document.getElementById('title').setAttribute('class', 'live');
    document.getElementById('title').setAttribute('data-stamp', stamp);
";

#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn now(_args: Vec<Value>, _this: Value, heap: Heap, fuel: Fuel) -> EvalResult {
    // Pure host command (no captured state): returns a fixed marker
    // string.  A real host would consult its clock here.
    Ok((
        Outcome::Normal(Value::String("2026-05-31T12:00:00Z".to_owned())),
        heap,
        fuel,
    ))
}

fn main() -> Result<(), Error> {
    let viewport = Viewport::new(640, 480);
    let commands = HostCommands::new().with("now", now);
    let frame = run_script_with_backprop(HTML, CSS, JS, viewport, &commands)?;
    run_window(frame, viewport)
}
