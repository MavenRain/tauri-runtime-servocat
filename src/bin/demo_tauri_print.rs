//! v1.9 demo: drive the previously-no-op `WebviewDispatch::print`
//! (rasterizes the current frame to a PNG in the OS temp directory)
//! and `with_webview` (now succeeds; runs the supplied closure on the
//! main thread with a placeholder `Box<dyn Any>` so callers can use it
//! for host-side scheduling).
//!
//! Run with `cargo run --bin demo_tauri_print`.  Close the window to
//! exit.  After exit the PNG should be at
//! `$TMPDIR/tauri-runtime-servocat-print-main.png`.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>tauri-runtime-servocat v1.9</h1><p>print() rasterizes this frame to a PNG.</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.9 print demo")
            .inner_size(640.0, 360.0)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(window_attrs, "main").map(
            |window_pending| {
                let _ = runtime
                    .create_window::<fn(tauri_runtime::window::RawWindow)>(window_pending, None)
                    .map(|detached_window| {
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
                                            let dispatcher = detached_webview.dispatcher.clone();
                                            let _ = std::thread::spawn(move || {
                                                std::thread::sleep(Duration::from_millis(1200));
                                                println!(
                                                    "[v1.9 demo] with_webview(...) -> {:?}",
                                                    dispatcher.with_webview(|_handle| {
                                                        println!(
                                                            "[v1.9 demo] with_webview closure ran on main thread",
                                                        );
                                                    }),
                                                );
                                                std::thread::sleep(Duration::from_millis(200));
                                                println!(
                                                    "[v1.9 demo] print() -> {:?}",
                                                    dispatcher.print(),
                                                );
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
