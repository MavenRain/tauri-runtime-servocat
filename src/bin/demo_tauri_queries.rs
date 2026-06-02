//! v1.3 demo: round-trip `WindowDispatch` state queries from a worker
//! thread, and observe the JS return value of an `eval_script_with_callback`
//! call land back in the supplied callback.
//!
//! Run with `cargo run --bin demo_tauri_queries`.  Close the window
//! to exit.

#![allow(clippy::assigning_clones)]

use std::sync::mpsc;
use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch, WindowDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>tauri-runtime-servocat v1.3</h1><p>state queries + eval callback</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.3 query demo")
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
                                                report_state(&window_dispatcher);
                                                std::thread::sleep(Duration::from_millis(400));
                                                report_eval(&webview_dispatcher);
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

fn report_state<D: WindowDispatch<()>>(dispatcher: &D) {
    let title = dispatcher
        .title()
        .unwrap_or_else(|_e| String::from("<error>"));
    let inner = dispatcher.inner_size().unwrap_or_default();
    let scale = dispatcher.scale_factor().unwrap_or(0.0);
    let focused = dispatcher.is_focused().unwrap_or(false);
    let visible = dispatcher.is_visible().unwrap_or(false);
    let maximized = dispatcher.is_maximized().unwrap_or(false);
    let fullscreen = dispatcher.is_fullscreen().unwrap_or(false);
    println!("[v1.3 demo] window state from worker thread:");
    println!("[v1.3 demo]   title       = {title:?}");
    println!(
        "[v1.3 demo]   inner_size  = {}x{}",
        inner.width, inner.height
    );
    println!("[v1.3 demo]   scale       = {scale}");
    println!("[v1.3 demo]   focused     = {focused}");
    println!("[v1.3 demo]   visible     = {visible}");
    println!("[v1.3 demo]   maximized   = {maximized}");
    println!("[v1.3 demo]   fullscreen  = {fullscreen}");
}

fn report_eval<D: WebviewDispatch<()>>(dispatcher: &D) {
    let (tx, rx) = mpsc::channel::<String>();
    let _ = dispatcher.eval_script_with_callback("1 + 41", move |result| {
        let _ = tx.send(result);
    });
    let value = rx
        .recv_timeout(Duration::from_millis(1500))
        .unwrap_or_else(|_e| String::from("<no reply>"));
    println!("[v1.3 demo] eval_script_with_callback('1 + 41') -> {value}");
}
