//! v1.5.1 demo: drive the `WebviewIpcHandler` from JS running in
//! boa-cat, via the new `__TAURI__.post_ipc_message(payload)` shim.
//! Confirms the missing JS-side half of the IPC bridge: a Tauri app
//! whose webview-side code calls `__TAURI__.post_ipc_message(...)`
//! reaches the same `invoke_handler` the host registered.
//!
//! Run with `cargo run --bin demo_tauri_ipc_js`.  Close the window
//! to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>tauri-runtime-servocat v1.5.1</h1><p>JS-side ipc_handler bridge</p></body></html>";

// JS that the host runs via eval_script.  Calls our v1.5.1 shim,
// which fires the host's ipc_handler.
const JS_TRIGGER: &str =
    "__TAURI__.post_ipc_message('{\\\"cmd\\\":\\\"greet\\\",\\\"name\\\":\\\"JS-World\\\"}')";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.5.1 JS-IPC demo")
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
                                    pending.ipc_handler = Some(Box::new(|webview, request| {
                                        let uri = request.uri().to_string();
                                        let body = request.body().clone();
                                        println!(
                                            "[v1.5.1 demo] ipc_handler fired: uri={uri:?} body={body:?}",
                                        );
                                        let reply = format!(
                                            "console.log('host responded to {uri}: ' + JSON.stringify({body:?}))",
                                        );
                                        let _ = webview.dispatcher.eval_script(reply);
                                    }));
                                    let _ = runtime
                                        .create_webview(detached_window.id, pending)
                                        .map(|detached_webview| {
                                            let dispatcher =
                                                detached_webview.dispatcher.clone();
                                            let _ = std::thread::spawn(move || {
                                                std::thread::sleep(Duration::from_millis(800));
                                                println!(
                                                    "[v1.5.1 demo] firing eval_script({JS_TRIGGER:?})",
                                                );
                                                let _ = dispatcher.eval_script(JS_TRIGGER);
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
