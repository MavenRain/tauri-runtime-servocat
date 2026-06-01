//! v1.1 demo: drive the `tauri_runtime::Runtime` trait skeleton on
//! the main thread.  Opens a window via `Runtime::create_window`,
//! sets its title via `WindowDispatch::set_title`, runs the event
//! loop, and exits when the window is closed.
//!
//! Run with `cargo run --bin demo_tauri_runtime`.

use std::time::Duration;

use tauri_runtime::window::{PendingWindow, WindowBuilder};
use tauri_runtime::{Runtime, RuntimeInitArgs, WindowDispatch};
use tauri_runtime_servocat::{ServocatRuntime, ServocatWindowBuilder};

fn main() {
    let _ = ServocatRuntime::<()>::new(RuntimeInitArgs::default()).map(|runtime| {
        let attrs = ServocatWindowBuilder::new()
            .title("tauri-runtime-servocat v1.1")
            .inner_size(640.0, 480.0)
            .resizable(true)
            .visible(true);
        let _ = PendingWindow::<(), ServocatRuntime<()>>::new(attrs, "main").map(|pending| {
            let _ = runtime
                .create_window::<fn(tauri_runtime::window::RawWindow)>(pending, None)
                .map(|detached| {
                    let dispatcher = detached.dispatcher.clone();
                    let _ = std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        let _ = dispatcher.set_title("tauri-runtime-servocat v1.1 (renamed)");
                    });
                    runtime.run(|_event| {});
                });
        });
    });
}
