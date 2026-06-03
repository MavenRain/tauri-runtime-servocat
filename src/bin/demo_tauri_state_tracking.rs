//! v1.7 demo: drive the previously-no-op `WindowDispatch` state
//! getters (`is_enabled`, `is_maximizable`, `is_minimizable`,
//! `is_closable`, `is_always_on_top`) plus `WebviewDispatch::bounds`
//! / `position` / `size`, round-tripping through `WindowFlags` and
//! webview-bounds bookkeeping in the `AppHandler`.
//!
//! Run with `cargo run --bin demo_tauri_state_tracking`.  Close the
//! window to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::dpi::{LogicalPosition, LogicalSize, Position, Size};
use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch, WindowDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>v1.7</h1><p>state tracking</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.7 state-tracking demo")
            .inner_size(640.0, 360.0)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(window_attrs, "main").map(
            |window_pending| {
                let _ = runtime
                    .create_window::<fn(tauri_runtime::window::RawWindow)>(window_pending, None)
                    .map(|detached_window| {
                        let window_dispatcher = detached_window.dispatcher.clone();
                        let _ = url::Url::parse(PAGE).ok().map(|page_url| {
                            let attrs = WebviewAttributes::new(
                                tauri_utils::config::WebviewUrl::External(page_url),
                            );
                            let _ = PendingWebview::<(), ServocatRuntime<()>>::new(attrs, "main")
                                .map(|mut pending| {
                                    pending.url = PAGE.to_owned();
                                    let _ = runtime
                                        .create_webview(detached_window.id, pending)
                                        .map(|detached_webview| {
                                            let webview_dispatcher =
                                                detached_webview.dispatcher.clone();
                                            let _ = std::thread::spawn(move || {
                                                std::thread::sleep(Duration::from_millis(800));
                                                exercise_window(&window_dispatcher);
                                                std::thread::sleep(Duration::from_millis(200));
                                                exercise_webview(&webview_dispatcher);
                                            });
                                            runtime.run(|_event| {});
                                        });
                                });
                        });
                    });
            },
        );
    });
}

fn exercise_window<D: WindowDispatch<()>>(dispatcher: &D) {
    println!("[v1.7 demo] window state getters (defaults):");
    println!(
        "[v1.7 demo]   is_enabled       = {:?}",
        dispatcher.is_enabled()
    );
    println!(
        "[v1.7 demo]   is_maximizable   = {:?}",
        dispatcher.is_maximizable()
    );
    println!(
        "[v1.7 demo]   is_minimizable   = {:?}",
        dispatcher.is_minimizable()
    );
    println!(
        "[v1.7 demo]   is_closable      = {:?}",
        dispatcher.is_closable()
    );
    println!(
        "[v1.7 demo]   is_always_on_top = {:?}",
        dispatcher.is_always_on_top()
    );
    let _ = dispatcher.set_enabled(false);
    let _ = dispatcher.set_maximizable(false);
    let _ = dispatcher.set_minimizable(false);
    let _ = dispatcher.set_closable(false);
    let _ = dispatcher.set_always_on_top(true);
    println!("[v1.7 demo] window state getters (after flipping all):");
    println!(
        "[v1.7 demo]   is_enabled       = {:?}",
        dispatcher.is_enabled()
    );
    println!(
        "[v1.7 demo]   is_maximizable   = {:?}",
        dispatcher.is_maximizable()
    );
    println!(
        "[v1.7 demo]   is_minimizable   = {:?}",
        dispatcher.is_minimizable()
    );
    println!(
        "[v1.7 demo]   is_closable      = {:?}",
        dispatcher.is_closable()
    );
    println!(
        "[v1.7 demo]   is_always_on_top = {:?}",
        dispatcher.is_always_on_top()
    );
}

fn exercise_webview<D: WebviewDispatch<()>>(dispatcher: &D) {
    println!("[v1.7 demo] webview bounds (defaults):");
    println!("[v1.7 demo]   position = {:?}", dispatcher.position());
    println!("[v1.7 demo]   size     = {:?}", dispatcher.size());
    let _ = dispatcher.set_position(Position::Logical(LogicalPosition::new(40.0, 60.0)));
    let _ = dispatcher.set_size(Size::Logical(LogicalSize::new(320.0, 240.0)));
    println!("[v1.7 demo] webview bounds (after set):");
    println!("[v1.7 demo]   position = {:?}", dispatcher.position());
    println!("[v1.7 demo]   size     = {:?}", dispatcher.size());
}
