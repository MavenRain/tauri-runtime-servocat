//! v1.10 demo: create two windows, attach a webview to the first,
//! then `reparent` it onto the second.  The same dispatcher is used
//! for all calls -- `set_title` on each window confirms two windows
//! exist, and the reparented webview ends up presenting in the second
//! window because the `webview_to_window` mapping in the handler
//! follows the move.
//!
//! Run with `cargo run --bin demo_tauri_reparent`.  Close both
//! windows to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch, WindowDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>v1.10</h1><p>reparent demo</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let primary_attrs = ServocatWindowBuilder::new()
            .title("v1.10 demo (primary window)")
            .inner_size(560.0, 320.0)
            .position(100.0, 100.0)
            .visible(true);
        let secondary_attrs = ServocatWindowBuilder::new()
            .title("v1.10 demo (secondary window)")
            .inner_size(560.0, 320.0)
            .position(720.0, 100.0)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(primary_attrs, "primary").map(
            |primary_pending| {
                let _ = PendingWindow::<(), ServocatRuntime<()>>::new(secondary_attrs, "secondary")
                    .map(|secondary_pending| {
                        let _ = runtime
                            .create_window::<fn(tauri_runtime::window::RawWindow)>(
                                primary_pending,
                                None,
                            )
                            .and_then(|primary| {
                                runtime
                                    .create_window::<fn(tauri_runtime::window::RawWindow)>(
                                        secondary_pending,
                                        None,
                                    )
                                    .map(|secondary| (primary, secondary))
                            })
                            .map(|(primary_window, secondary_window)| {
                                let _ = url::Url::parse(PAGE).ok().map(|page_url| {
                                    let attrs = WebviewAttributes::new(
                                        tauri_utils::config::WebviewUrl::External(page_url),
                                    );
                                    let _ = PendingWebview::<(), ServocatRuntime<()>>::new(
                                        attrs, "main",
                                    )
                                    .map(|mut pending| {
                                        pending.url = PAGE.to_owned();
                                        let _ = runtime
                                            .create_webview(primary_window.id, pending)
                                            .map(|detached_webview| {
                                                let webview_dispatcher =
                                                    detached_webview.dispatcher.clone();
                                                let secondary_id = secondary_window.id;
                                                let _ = std::thread::spawn(move || {
                                                    std::thread::sleep(Duration::from_millis(
                                                        1500,
                                                    ));
                                                    println!(
                                                        "[v1.10 demo] reparenting webview to secondary window: {:?}",
                                                        webview_dispatcher.reparent(secondary_id),
                                                    );
                                                });
                                                runtime.run(|_event| {});
                                            });
                                    });
                                });
                                // Use the window dispatchers to set titles so the demo
                                // shows both windows are independently controllable.
                                let _ = primary_window
                                    .dispatcher
                                    .set_title("primary (webview reparents away from here)");
                                let _ = secondary_window
                                    .dispatcher
                                    .set_title("secondary (webview lands here)");
                            });
                    });
            },
        );
    });
}
