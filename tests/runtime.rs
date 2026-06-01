//! v1.0 smoke tests: prove the [`tauri_runtime::Runtime`] trait
//! family is implemented by the Servocat types.  These tests don't
//! exercise behaviour (the skeleton mostly returns errors); they
//! ensure the trait bounds resolve so a Tauri app can be parameterized
//! over `ServocatRuntime`.

#![allow(clippy::unnecessary_wraps)]

use tauri_runtime::window::WindowBuilder;
use tauri_runtime::{
    EventLoopProxy, Runtime, RuntimeHandle, RuntimeInitArgs, WebviewDispatch, WindowDispatch,
};
use tauri_runtime_servocat::{
    Error, ServocatEventLoopProxy, ServocatHandle, ServocatRuntime, ServocatWebviewDispatch,
    ServocatWindowBuilder, ServocatWindowDispatch,
};

fn fail(_msg: &'static str) -> Error {
    Error::Engine(boa_cat::Error::Unsupported { feature: "test" })
}

fn assert_runtime<R: Runtime<()>>() {}
fn assert_handle<H: RuntimeHandle<()>>() {}
fn assert_window_dispatch<W: WindowDispatch<()>>() {}
fn assert_webview_dispatch<W: WebviewDispatch<()>>() {}
fn assert_event_loop_proxy<P: EventLoopProxy<()>>() {}
fn assert_window_builder<B: WindowBuilder>() {}

#[test]
fn servocat_runtime_implements_runtime() -> Result<(), Error> {
    assert_runtime::<ServocatRuntime<()>>();
    Ok(())
}

#[test]
fn servocat_handle_implements_runtime_handle() -> Result<(), Error> {
    assert_handle::<ServocatHandle<()>>();
    Ok(())
}

#[test]
fn servocat_window_dispatch_implements_window_dispatch() -> Result<(), Error> {
    assert_window_dispatch::<ServocatWindowDispatch<()>>();
    Ok(())
}

#[test]
fn servocat_webview_dispatch_implements_webview_dispatch() -> Result<(), Error> {
    assert_webview_dispatch::<ServocatWebviewDispatch<()>>();
    Ok(())
}

#[test]
fn servocat_event_loop_proxy_implements_event_loop_proxy() -> Result<(), Error> {
    assert_event_loop_proxy::<ServocatEventLoopProxy<()>>();
    Ok(())
}

#[test]
fn servocat_window_builder_implements_window_builder() -> Result<(), Error> {
    assert_window_builder::<ServocatWindowBuilder>();
    Ok(())
}

#[test]
fn runtime_constructor_returns_ok() -> Result<(), Error> {
    ServocatRuntime::<()>::new(RuntimeInitArgs::default())
        .map(|_runtime| ())
        .map_err(|_e| fail("Runtime::new should return Ok in the skeleton"))
}

#[test]
fn runtime_handle_round_trip() -> Result<(), Error> {
    let runtime = ServocatRuntime::<()>::new(RuntimeInitArgs::default())
        .map_err(|_e| fail("Runtime::new failed"))?;
    let _handle = runtime.handle();
    let _proxy = runtime.create_proxy();
    Ok(())
}

#[test]
fn window_builder_threads_theme() -> Result<(), Error> {
    let builder = ServocatWindowBuilder::new()
        .title("test")
        .resizable(true)
        .theme(Some(tauri_utils::Theme::Dark));
    (builder.get_theme() == Some(tauri_utils::Theme::Dark))
        .then_some(())
        .ok_or_else(|| fail("WindowBuilder::theme should round-trip via get_theme"))
}
