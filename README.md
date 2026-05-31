# tauri-runtime-servocat

Servo-replacement runtime for Tauri: wires the full `comp-cat-rs` cat-stack ([`html-cat`](https://crates.io/crates/html-cat), [`css-cat`](https://crates.io/crates/css-cat), [`dom-cat`](https://crates.io/crates/dom-cat), [`layout-cat`](https://crates.io/crates/layout-cat), [`paint-cat`](https://crates.io/crates/paint-cat), [`net-cat`](https://crates.io/crates/net-cat), [`boa-cat`](https://crates.io/crates/boa-cat), [`ecma-runtime-cat`](https://crates.io/crates/ecma-runtime-cat), [`web-api-cat`](https://crates.io/crates/web-api-cat)) into a single rendering + scripting pipeline.

## Example

```rust
use tauri_runtime_servocat::{render, run_script, Error, Viewport};

fn main() -> Result<(), Error> {
    let frame = render(
        "<html><body><p>hello</p></body></html>",
        "p { background-color: red; padding: 8px; }",
        Viewport::new(800, 600),
    )?;
    assert!(!frame.display_list().is_empty());

    let scripted = run_script(
        "<html><body><p id='g'>hi</p></body></html>",
        "",
        "document.getElementById('g').textContent",
        Viewport::new(800, 600),
    )?;
    assert_eq!(format!("{}", scripted.script_value()), "\"hi\"");
    Ok(())
}
```

## v0.1.0 scope

- `render(html, css, viewport) -> Frame` -- HTML → DOM → layout → display list.
- `run_script(html, css, js, viewport) -> Frame` -- same plus an inline JS step.
- `Frame` carries the dom-cat `Document`, layout tree, paint-cat `DisplayList`, and (when scripted) the JS return value.

## Roadmap (committed for subsequent versions)

- **v0.2.0** -- `tiny-skia` rasterizer (rectangles only, no text); `render_to_pixels(frame, buffer)`.
- **v0.3.0** -- `cosmic-text` for text shaping + glyph raster.
- **v0.4.0** -- `winit` + `softbuffer` window + a runnable demo binary.
- **v0.5.0** -- IPC bridge (`window.__TAURI_INVOKE__`); DOM-mutation back-propagation into layout.
- **v1.0.0** -- `tauri_runtime::Runtime` trait impl with the working pieces wired in; remaining methods stubbed and filled in over patch releases.

## License

MIT OR Apache-2.0
