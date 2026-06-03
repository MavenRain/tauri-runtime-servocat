//! v1.8 demo: exercise the previously-no-op `WebviewDispatch`
//! state-tracking methods -- `set_zoom`, `set_background_color`,
//! `set_auto_resize`, the cookie jar (`set_cookie` / `delete_cookie` /
//! `cookies` / `cookies_for_url`), and `clear_all_browsing_data`.
//!
//! Run with `cargo run --bin demo_tauri_webview_state`.  Close the
//! window to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use cookie::Cookie;
use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WebviewDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

const PAGE: &str = "data:text/html,<html><body><h1>v1.8</h1><p>webview state</p></body></html>";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.8 webview-state demo")
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
                                                std::thread::sleep(Duration::from_millis(800));
                                                exercise(&dispatcher);
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

fn exercise<D: WebviewDispatch<()>>(dispatcher: &D) {
    println!(
        "[v1.8 demo] set_zoom(1.25) -> {:?}",
        dispatcher.set_zoom(1.25)
    );
    println!(
        "[v1.8 demo] set_auto_resize(true) -> {:?}",
        dispatcher.set_auto_resize(true)
    );
    println!(
        "[v1.8 demo] set_background_color(navy) -> {:?}",
        dispatcher.set_background_color(Some(tauri_utils::config::Color(0, 0, 128, 255))),
    );

    let session = Cookie::build(("session", "abc123")).build();
    let preference = Cookie::build(("theme", "dark")).build();
    println!(
        "[v1.8 demo] set_cookie(session=abc123) -> {:?}",
        dispatcher.set_cookie(session.clone()),
    );
    println!(
        "[v1.8 demo] set_cookie(theme=dark)     -> {:?}",
        dispatcher.set_cookie(preference.clone()),
    );
    println!("[v1.8 demo] cookies() -> {:?}", dispatcher.cookies());

    let _ = dispatcher.delete_cookie(session);
    println!(
        "[v1.8 demo] cookies() after delete(session) -> {:?}",
        dispatcher.cookies(),
    );

    let _ = dispatcher.clear_all_browsing_data();
    println!(
        "[v1.8 demo] cookies() after clear_all_browsing_data -> {:?}",
        dispatcher.cookies(),
    );
}
