//! Demo binary: renders a small HTML page and shows it in a window.
//!
//! Run with `cargo run --bin demo`.  Close the window to exit.

use tauri_runtime_servocat::{Error, Viewport, render, run_window};

const HTML: &str = "<html><body>\
<h1>tauri-runtime-servocat</h1>\
<p>cosmic-text glyph raster, presented through a softbuffer window.</p>\
<p>close the window to exit.</p>\
</body></html>";

const CSS: &str = "
    body { background-color: white; padding: 24px; }
    h1 { color: navy; height: 48px; }
    p { color: black; height: 28px; }
";

fn main() -> Result<(), Error> {
    let viewport = Viewport::new(640, 480);
    let frame = render(HTML, CSS, viewport)?;
    run_window(frame, viewport)
}
