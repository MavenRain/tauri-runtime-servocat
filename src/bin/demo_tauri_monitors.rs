//! v1.4 demo: enumerate monitors from a worker thread via
//! `RuntimeHandle::available_monitors`, dispatch a closure back to
//! the main thread via `run_on_main_thread`, and load a page from a
//! percent-encoded `data:text/html,...` URL.
//!
//! Run with `cargo run --bin demo_tauri_monitors`.  Close the window
//! to exit.

#![allow(clippy::assigning_clones)]

use std::time::Duration;

use tauri_runtime::webview::{PendingWebview, WebviewAttributes};
use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeHandle, RuntimeInitArgs, WindowDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

// Percent-encoded HTML body (`<` -> `%3C`, `>` -> `%3E`, space -> `%20`)
// to exercise the v1.4 percent-decoder.
const PAGE: &str = "data:text/html,%3Chtml%3E%3Cbody%3E%3Ch1%3Etauri-runtime-servocat%20v1.4%3C%2Fh1%3E%3Cp%3Emonitor%20enumeration%20%2B%20run_on_main_thread%3C%2Fp%3E%3C%2Fbody%3E%3C%2Fhtml%3E";

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let window_attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.4 monitors demo")
            .inner_size(640.0, 360.0)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(window_attrs, "main").map(
            |window_pending| {
                let _ = runtime
                    .create_window::<fn(tauri_runtime::window::RawWindow)>(window_pending, None)
                    .map(|detached_window| {
                        let window_dispatcher = detached_window.dispatcher.clone();
                        let handle = runtime.handle();
                        let _ = url::Url::parse(PAGE).ok().map(|page_url| {
                            let attrs = WebviewAttributes::new(
                                tauri_utils::config::WebviewUrl::External(page_url),
                            );
                            let _ = PendingWebview::<(), ServocatRuntime<()>>::new(attrs, "main")
                                .map(|mut pending| {
                                    pending.url = PAGE.to_owned();
                                    let _ = runtime.create_webview(detached_window.id, pending);
                                    let _ = std::thread::spawn(move || {
                                        std::thread::sleep(Duration::from_millis(800));
                                        report_monitors(&handle);
                                        std::thread::sleep(Duration::from_millis(200));
                                        run_on_main_demo(&window_dispatcher);
                                    });
                                    runtime.run(|_event| {});
                                });
                        });
                    });
            },
        );
    });
}

fn report_monitors<H: RuntimeHandle<()>>(handle: &H) {
    let primary = handle.primary_monitor();
    let monitors = handle.available_monitors();
    println!("[v1.4 demo] monitor enumeration from worker thread:");
    println!(
        "[v1.4 demo]   primary       = {}",
        primary
            .as_ref()
            .map_or("<none>".to_owned(), describe_monitor),
    );
    println!("[v1.4 demo]   total count   = {}", monitors.len());
    monitors.iter().enumerate().for_each(|(index, monitor)| {
        println!(
            "[v1.4 demo]   monitor[{index}] = {}",
            describe_monitor(monitor)
        );
    });
}

fn describe_monitor(monitor: &tauri_runtime::monitor::Monitor) -> String {
    format!(
        "{} ({}x{} @ ({}, {}) scale={})",
        monitor.name.as_deref().unwrap_or("<unnamed>"),
        monitor.size.width,
        monitor.size.height,
        monitor.position.x,
        monitor.position.y,
        monitor.scale_factor,
    )
}

fn run_on_main_demo<D: WindowDispatch<()>>(dispatcher: &D) {
    let _ = dispatcher.run_on_main_thread(|| {
        println!("[v1.4 demo] run_on_main_thread closure fired on the main thread");
    });
}
