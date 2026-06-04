//! v2.0 demo: set the webview's `background_color` to navy and watch
//! the softbuffer compositor actually paint the transparent areas in
//! that colour instead of always defaulting to white.
//!
//! Run with `cargo run --bin demo_tauri_bg_color`.  Close the window
//! to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1 style='color:white'>tauri-runtime-servocat v2.0</h1><p style='color:white'>background color applied in softbuffer compositor</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v2.0 bg-color demo")
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
                                            let dispatcher =
                                                detached_webview.dispatcher.clone();
                                            let _ = std::thread::spawn(move || {
                                                std::thread::sleep(Duration::from_millis(1500));
                                                println!(
                                                    "[v2.0 demo] set_background_color(navy) -> {:?}",
                                                    dispatcher.set_background_color(Some(
                                                        tauri_utils::config::Color(
                                                            0, 0, 128, 255,
                                                        ),
                                                    )),
                                                );
                                                // Force a re-paint so the background change is
                                                // visible (reload re-builds the frame and
                                                // requests a redraw).
                                                let _ = dispatcher.reload();
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
