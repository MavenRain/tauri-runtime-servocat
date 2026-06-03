//! v1.6 demo: exercise the previously-no-op `WindowDispatch` methods
//! that now route through winit -- `set_cursor_icon`, `set_min_size`
//! / `set_max_size`, `set_always_on_top`, `set_theme`, `center`,
//! `request_user_attention`.
//!
//! Run with `cargo run --bin demo_tauri_window_methods`.  Close the
//! window to exit.

use std::time::Duration;

use tauri_runtime::dpi::{LogicalSize, Size};
use tauri_runtime::window::{CursorIcon, PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, UserAttentionType, WindowDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.6 windowdispatch demo")
            .inner_size(640.0, 360.0)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(window_attrs, "main").map(
            |window_pending| {
                let _ = runtime
                    .create_window::<fn(tauri_runtime::window::RawWindow)>(window_pending, None)
                    .map(|detached_window| {
                        let dispatcher = detached_window.dispatcher.clone();
                        let _ = std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(400));
                            exercise(&dispatcher);
                        });
                        runtime.run(|_event| {});
                    });
            },
        );
    });
}

fn exercise<D: WindowDispatch<()>>(dispatcher: &D) {
    println!(
        "[v1.6 demo] set_min_size((400, 300)) -> {:?}",
        dispatcher.set_min_size(Some(Size::Logical(LogicalSize::new(400.0, 300.0))))
    );
    println!(
        "[v1.6 demo] set_max_size((1200, 900)) -> {:?}",
        dispatcher.set_max_size(Some(Size::Logical(LogicalSize::new(1200.0, 900.0))))
    );
    println!(
        "[v1.6 demo] set_cursor_icon(Crosshair) -> {:?}",
        dispatcher.set_cursor_icon(CursorIcon::Crosshair)
    );
    println!(
        "[v1.6 demo] set_always_on_top(true) -> {:?}",
        dispatcher.set_always_on_top(true)
    );
    println!("[v1.6 demo] center() -> {:?}", dispatcher.center());
    println!(
        "[v1.6 demo] set_theme(Dark) -> {:?}",
        dispatcher.set_theme(Some(tauri_utils::Theme::Dark))
    );
    println!(
        "[v1.6 demo] request_user_attention(Informational) -> {:?}",
        dispatcher.request_user_attention(Some(UserAttentionType::Informational))
    );
    println!(
        "[v1.6 demo] set_cursor_visible(false) -> {:?}",
        dispatcher.set_cursor_visible(false)
    );
    println!(
        "[v1.6 demo] set_cursor_grab(true) -> {:?}",
        dispatcher.set_cursor_grab(true)
    );
    println!(
        "[v1.6 demo] set_ignore_cursor_events(false) -> {:?}",
        dispatcher.set_ignore_cursor_events(false)
    );
}
