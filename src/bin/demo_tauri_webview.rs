//! v1.2 demo: drive a full `tauri_runtime::Runtime` flow on the main
//! thread.  Opens a window, attaches a webview to it via
//! `Runtime::create_webview` with a `data:text/html,...` URL, then
//! from a worker thread navigates to a second page and runs a JS
//! script that mutates the DOM (which back-propagates into the
//! displayed frame).
//!
//! Run with `cargo run --bin demo_tauri_webview`.  Close the window
//! to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE_ONE: &str = "data:text/html,<html><body><h1>tauri-runtime-servocat v1.2</h1><p id='live'>initial page (PAGE_ONE)</p></body></html>";
const PAGE_TWO: &str = "data:text/html,<html><body><h1>navigated</h1><p id='live'>after WebviewDispatch::navigate()</p></body></html>";
const SCRIPT: &str = "document.getElementById('live').setAttribute('class', 'highlighted')";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.2 webview demo")
            .inner_size(720.0, 480.0)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(window_attrs, "main").map(
            |window_pending| {
                let _ = runtime
                    .create_window::<fn(tauri_runtime::window::RawWindow)>(window_pending, None)
                    .map(|detached_window| {
                        let _ = url::Url::parse(PAGE_ONE).ok().map(|page_url| {
                            let attrs = WebviewAttributes::new(
                                tauri_utils::config::WebviewUrl::External(page_url),
                            );
                            let _ = PendingWebview::<(), ServocatRuntime<()>>::new(attrs, "main")
                                .map(|mut pending| {
                                    pending.url = PAGE_ONE.to_owned();
                                    let _ = runtime
                                        .create_webview(detached_window.id, pending)
                                        .map(|detached_webview| {
                                            let dispatcher = detached_webview.dispatcher.clone();
                                            let _ = std::thread::spawn(move || {
                                                std::thread::sleep(Duration::from_millis(1500));
                                                let _ = url::Url::parse(PAGE_TWO)
                                                    .map(|url| dispatcher.navigate(url));
                                                std::thread::sleep(Duration::from_millis(1500));
                                                let _ = dispatcher.eval_script(SCRIPT);
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
