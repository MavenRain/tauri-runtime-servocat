//! v1.5 demo: install a `WebviewIpcHandler` on a webview (the
//! Tauri-side hook that a real app's `tauri::Builder::invoke_handler`
//! plugs into) and fire it from a worker thread via
//! [`ServocatWebviewDispatch::send_ipc`].  The handler prints the
//! request and then sends a result string back through the webview's
//! `eval_script` channel, the way Tauri's command system delivers a
//! response to JS.
//!
//! Run with `cargo run --bin demo_tauri_ipc_handler`.  Close the
//! window to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>tauri-runtime-servocat v1.5</h1><p>ipc_handler bridge</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.5 ipc demo")
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
                                    pending.ipc_handler =
                                        Some(Box::new(|webview, request| {
                                            let uri = request.uri().to_string();
                                            let body = request.body().clone();
                                            println!(
                                                "[v1.5 demo] ipc_handler fired: uri={uri:?} body={body:?}",
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
                                                fire_ipc(&dispatcher);
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

fn fire_ipc(dispatcher: &tauri_runtime_servocat::ServocatWebviewDispatch<()>) {
    let request = http::Request::builder()
        .uri("ipc://greet")
        .body(r#"{"name":"World"}"#.to_owned())
        .ok();
    let _ = request.map(|req| {
        let result = dispatcher.send_ipc(req);
        println!("[v1.5 demo] send_ipc(ipc://greet) -> {result:?}");
    });
}
