//! v2.3 implementation of the `tauri_runtime::Runtime` trait surface.
//!
//! Backed by a real winit event loop with per-window softbuffer
//! presentation:
//!
//! - [`ServocatRuntime::new`] creates a winit [`EventLoop`] with our
//!   internal [`RuntimeEvent<T>`] payload.
//! - [`Runtime::create_window`] allocates a [`WindowId`] and sends a
//!   `RuntimeEvent::CreateWindow` through the proxy; the actual winit
//!   window is constructed inside the event-loop handler when the
//!   event is delivered.
//! - [`Runtime::create_webview`] sends a `RuntimeEvent::CreateWebview`
//!   for the window; the handler parses the URL (currently
//!   `data:text/html,...`), runs the cat-stack pipeline, and stores
//!   the resulting [`Frame`] plus a reusable [`TextRenderer`] in a
//!   per-window [`WebviewState`].
//! - [`WebviewDispatch::navigate`] sends `Navigate`, replacing the
//!   stored HTML and rebuilding the frame.
//!   [`WebviewDispatch::reload`] re-runs the pipeline against the
//!   cached HTML/CSS.  [`WebviewDispatch::eval_script`] runs the
//!   script via `run_script_with_backprop`, back-propagates DOM
//!   mutations into layout, and stores the new frame.
//! - [`Runtime::run`] consumes the event loop and drives an
//!   [`ApplicationHandler`] that creates queued windows, paints the
//!   webview frame on `RedrawRequested` via softbuffer, routes winit
//!   `WindowEvent`s to `tauri_runtime::WindowEvent`s, and forwards
//!   user-defined events.
//! - [`WindowDispatch::set_title`] / [`set_size`] / [`set_position`] /
//!   [`show`] / [`hide`] / [`close`] / [`set_focus`] / [`maximize`] /
//!   [`unmaximize`] / [`minimize`] / [`unminimize`] / [`set_resizable`] /
//!   [`set_decorations`] / [`set_fullscreen`] send their commands
//!   through the proxy.  Most [`WindowDispatch`] / [`WebviewDispatch`]
//!   getters still return zero values; per-window state query
//!   round-trips arrive in 1.3+.
//!
//! [`Runtime`]: tauri_runtime::Runtime
//! [`Runtime::create_window`]: tauri_runtime::Runtime::create_window
//! [`Runtime::run`]: tauri_runtime::Runtime::run
//! [`WindowDispatch`]: tauri_runtime::WindowDispatch
//! [`WindowDispatch::set_title`]: tauri_runtime::WindowDispatch::set_title
//! [`set_size`]: tauri_runtime::WindowDispatch::set_size
//! [`set_position`]: tauri_runtime::WindowDispatch::set_position
//! [`show`]: tauri_runtime::WindowDispatch::show
//! [`hide`]: tauri_runtime::WindowDispatch::hide
//! [`close`]: tauri_runtime::WindowDispatch::close
//! [`set_focus`]: tauri_runtime::WindowDispatch::set_focus
//! [`maximize`]: tauri_runtime::WindowDispatch::maximize
//! [`unmaximize`]: tauri_runtime::WindowDispatch::unmaximize
//! [`minimize`]: tauri_runtime::WindowDispatch::minimize
//! [`unminimize`]: tauri_runtime::WindowDispatch::unminimize
//! [`set_resizable`]: tauri_runtime::WindowDispatch::set_resizable
//! [`set_decorations`]: tauri_runtime::WindowDispatch::set_decorations
//! [`set_fullscreen`]: tauri_runtime::WindowDispatch::set_fullscreen
//! [`EventLoop`]: winit::event_loop::EventLoop
//! [`ApplicationHandler`]: winit::application::ApplicationHandler

#![allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    clippy::too_many_lines,
    unexpected_cfgs
)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use boa_cat::Value;
use boa_cat::fuel::Fuel;
use boa_cat::heap::Heap as BoaHeap;
use boa_cat::outcome::{EvalResult, Outcome};

use cookie::Cookie;
use raw_window_handle::{DisplayHandle, HandleError, WindowHandle};
use softbuffer::{Context, Surface};
use tauri_runtime::dpi::{PhysicalPosition, PhysicalRect, PhysicalSize, Position, Rect, Size};
use tauri_runtime::monitor::Monitor;
use tauri_runtime::webview::{DetachedWebview, PendingWebview, WebviewIpcHandler};
use tauri_runtime::window::{
    CursorIcon, DetachedWindow, PendingWindow, RawWindow, WebviewEvent, WindowBuilder,
    WindowBuilderBase, WindowEvent as TauriWindowEvent, WindowId, WindowSizeConstraints,
};
use tauri_runtime::{
    DeviceEventFilter, Error, EventLoopProxy as TauriEventLoopProxy, Icon, ProgressBarState,
    Result, RunEvent, Runtime, RuntimeHandle, RuntimeInitArgs, UserEvent, WebviewDispatch,
    WebviewEventId, WindowDispatch, WindowEventId,
};
use tauri_utils::config::{Color, WindowConfig};
use tauri_utils::{Theme, TitleBarStyle};
use url::Url;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalPosition as WinitPhysicalPosition};
use winit::event::WindowEvent as WinitWindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId as WinitWindowId};

use crate::frame::Frame;
use crate::ipc::HostCommands;
use crate::raster::{PixelBuffer, render_to_pixels_with};
use crate::script::run_script_with_backprop;
use crate::text::TextRenderer;
use layout_cat::Viewport;

static NEXT_WINDOW_ID: AtomicU32 = AtomicU32::new(1);
static NEXT_WEBVIEW_ID: AtomicU32 = AtomicU32::new(1);

fn allocate_window_id() -> WindowId {
    WindowId::from(NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst))
}

fn allocate_webview_id() -> WebviewId {
    WebviewId(NEXT_WEBVIEW_ID.fetch_add(1, Ordering::SeqCst))
}

fn raw_window_id(id: WindowId) -> u32 {
    // `WindowId` is a `pub struct WindowId(u32)` newtype with `From<u32>`
    // but no direct accessor.  Round-trip through `Debug` to recover
    // the raw value -- ugly but stable for v1.1.
    let debug = format!("{id:?}");
    debug
        .trim_start_matches("WindowId(")
        .trim_end_matches(')')
        .parse::<u32>()
        .unwrap_or(0)
}

/// Stable identifier for a webview owned by a [`ServocatRuntime`].
/// Distinct from [`WindowId`] so that
/// [`WebviewDispatch::reparent`] can move a webview between windows
/// without invalidating an existing dispatcher.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct WebviewId(u32);

impl WebviewId {
    fn raw(self) -> u32 {
        self.0
    }
}

/// Cross-thread message sent through the winit `EventLoopProxy` to
/// command the runtime from another thread (a [`ServocatHandle`] /
/// [`ServocatWindowDispatch`] / [`ServocatWebviewDispatch`]).
///
/// Not `Clone` (carries `mpsc::Sender` reply handles and a
/// `Box<dyn Fn>` callback for `EvalScriptWithCallback`).  `Debug` is
/// hand-rolled to print just the variant name.
pub enum RuntimeEvent<T: UserEvent> {
    /// A user-defined event surfaced through `Runtime::run`'s callback.
    User(T),
    /// Asks the runtime to create a window with the given attributes
    /// under the given id and label.
    CreateWindow {
        id: WindowId,
        label: String,
        attributes: ServocatWindowBuilder,
    },
    /// Sets the title of an existing window.
    SetTitle { id: WindowId, title: String },
    /// Resizes an existing window.
    SetSize { id: WindowId, size: Size },
    /// Repositions an existing window.
    SetPosition { id: WindowId, position: Position },
    /// Shows an existing window.
    Show { id: WindowId },
    /// Hides an existing window.
    Hide { id: WindowId },
    /// Focuses an existing window.
    Focus { id: WindowId },
    /// Closes an existing window.
    Close { id: WindowId },
    /// Destroys an existing window.
    Destroy { id: WindowId },
    /// Maximizes an existing window.
    Maximize { id: WindowId },
    /// Unmaximizes an existing window.
    Unmaximize { id: WindowId },
    /// Minimizes an existing window.
    Minimize { id: WindowId },
    /// Unminimizes an existing window.
    Unminimize { id: WindowId },
    /// Updates the resizable flag of an existing window.
    SetResizable { id: WindowId, resizable: bool },
    /// Updates the decorations flag of an existing window.
    SetDecorations { id: WindowId, decorations: bool },
    /// Updates the fullscreen flag of an existing window.
    SetFullscreen { id: WindowId, fullscreen: bool },
    /// Attaches a webview to an existing window with the given URL.
    /// The runtime will parse the URL (currently `data:text/html,...`),
    /// build the cat-stack pipeline, and present the resulting frame
    /// into the window via softbuffer.  The optional `ipc_handler` is
    /// stored alongside the webview so [`ServocatWebviewDispatch::send_ipc`]
    /// (and the JS-side `__TAURI__.post_ipc_message` shim) can fire
    /// the host's `invoke_handler`.
    CreateWebview {
        window_id: WindowId,
        webview_id: WebviewId,
        label: String,
        url: String,
        ipc_handler: Option<WebviewIpcHandler<T, ServocatRuntime<T>>>,
    },
    /// Re-attaches an existing webview to a different window.
    /// Updates the `webview_to_window` / `window_to_webview` maps;
    /// the dispatcher continues to address this webview.
    ReparentWebview {
        webview_id: WebviewId,
        new_window_id: WindowId,
    },
    /// Synthesizes a webview->host IPC request, invoking the
    /// `ipc_handler` registered when the webview was created.
    IpcRequest {
        webview_id: WebviewId,
        request: http::Request<String>,
    },
    /// Navigates the webview to a new URL.  The runtime re-builds the
    /// pipeline against the new content and requests a redraw on the
    /// webview's parent window.
    Navigate { webview_id: WebviewId, url: String },
    /// Reloads the webview (re-builds the pipeline against the cached
    /// HTML/CSS and requests a redraw).
    Reload { webview_id: WebviewId },
    /// Runs a JS expression against the webview via
    /// `run_script_with_backprop`, back-propagates DOM mutations into
    /// layout, and requests a redraw.
    EvalScript {
        webview_id: WebviewId,
        script: String,
    },
    /// Like [`Self::EvalScript`] but also delivers a string-formatted
    /// representation of the script's return value back through the
    /// supplied callback after the eval completes.
    ///
    /// FFI carve-out: `Box<dyn Fn(String)>` is unavoidable here.
    /// `winit::EventLoop<RuntimeEvent<T>>` is parameterized on a
    /// concrete enum, so a closure with arbitrary captures (which is
    /// what the `WebviewDispatch::eval_script_with_callback` trait
    /// method accepts) must be type-erased to fit in a variant.
    EvalScriptWithCallback {
        webview_id: WebviewId,
        script: String,
        callback: Box<dyn Fn(String) + Send + 'static>,
    },
    /// Asks the handler for the inner (client area) size of a window.
    QueryInnerSize {
        id: WindowId,
        reply: mpsc::Sender<PhysicalSize<u32>>,
    },
    /// Asks the handler for the outer (full window) size of a window.
    QueryOuterSize {
        id: WindowId,
        reply: mpsc::Sender<PhysicalSize<u32>>,
    },
    /// Asks the handler for the inner position of a window.
    QueryInnerPosition {
        id: WindowId,
        reply: mpsc::Sender<PhysicalPosition<i32>>,
    },
    /// Asks the handler for the outer position of a window.
    QueryOuterPosition {
        id: WindowId,
        reply: mpsc::Sender<PhysicalPosition<i32>>,
    },
    /// Asks the handler for the scale factor of a window.
    QueryScaleFactor {
        id: WindowId,
        reply: mpsc::Sender<f64>,
    },
    /// Asks the handler for the title of a window.
    QueryTitle {
        id: WindowId,
        reply: mpsc::Sender<String>,
    },
    /// Asks the handler whether a window is currently visible.
    QueryIsVisible {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window currently has focus.
    QueryIsFocused {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window is currently maximized.
    QueryIsMaximized {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window is currently minimized.
    QueryIsMinimized {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window is currently in fullscreen.
    QueryIsFullscreen {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window is currently decorated.
    QueryIsDecorated {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window is currently resizable.
    QueryIsResizable {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler for the theme of a window.
    QueryTheme {
        id: WindowId,
        reply: mpsc::Sender<Theme>,
    },
    /// Asks the handler for the primary monitor of the system.
    QueryPrimaryMonitor {
        reply: mpsc::Sender<Option<Monitor>>,
    },
    /// Asks the handler for all monitors known to the system.
    QueryAvailableMonitors { reply: mpsc::Sender<Vec<Monitor>> },
    /// Asks the handler for the monitor that contains the given
    /// point, in absolute coordinates.
    QueryMonitorFromPoint {
        x: f64,
        y: f64,
        reply: mpsc::Sender<Option<Monitor>>,
    },
    /// Asks the handler for the monitor a particular window currently
    /// resides on.
    QueryCurrentMonitor {
        id: WindowId,
        reply: mpsc::Sender<Option<Monitor>>,
    },
    /// Run an arbitrary closure on the main thread (where the event
    /// loop is dispatched).
    ///
    /// FFI carve-out: `Box<dyn FnOnce()>` is unavoidable.  The trait
    /// methods `RuntimeHandle::run_on_main_thread` /
    /// `WindowDispatch::run_on_main_thread` /
    /// `WebviewDispatch::run_on_main_thread` accept a generic
    /// `F: FnOnce() + Send + 'static`; since
    /// `winit::EventLoop<RuntimeEvent<T>>` is a concrete enum the
    /// closure with arbitrary captures must be type-erased to ship
    /// through the proxy.
    RunOnMainThread {
        thunk: Box<dyn FnOnce() + Send + 'static>,
    },
    /// Sets cursor visibility for an existing window.
    SetCursorVisible { id: WindowId, visible: bool },
    /// Sets cursor grab (lock to window) for an existing window.
    SetCursorGrab { id: WindowId, grab: bool },
    /// Sets the cursor icon for an existing window.
    SetCursorIcon { id: WindowId, icon: CursorIcon },
    /// Moves the cursor within an existing window's coordinate system.
    SetCursorPosition { id: WindowId, position: Position },
    /// Toggles whether an existing window passes cursor events through.
    SetIgnoreCursorEvents { id: WindowId, ignore: bool },
    /// Updates the minimum inner size constraint of an existing window.
    SetMinSize { id: WindowId, size: Option<Size> },
    /// Updates the maximum inner size constraint of an existing window.
    SetMaxSize { id: WindowId, size: Option<Size> },
    /// Updates the always-on-top flag of an existing window.
    SetAlwaysOnTop { id: WindowId, always_on_top: bool },
    /// Updates the theme of an existing window.
    SetWindowTheme { id: WindowId, theme: Option<Theme> },
    /// Starts dragging the OS chrome of an existing window.
    StartDragging { id: WindowId },
    /// Requests user attention to an existing window.
    RequestUserAttention {
        id: WindowId,
        request_type: Option<tauri_runtime::UserAttentionType>,
    },
    /// Centers an existing window on its current monitor.
    CenterWindow { id: WindowId },
    /// Sets the icon of an existing window.
    SetWindowIcon {
        id: WindowId,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Updates the tracked `enabled` flag for a window.
    SetEnabled { id: WindowId, enabled: bool },
    /// Updates the tracked `maximizable` flag for a window.
    SetMaximizable { id: WindowId, maximizable: bool },
    /// Updates the tracked `minimizable` flag for a window.
    SetMinimizable { id: WindowId, minimizable: bool },
    /// Updates the tracked `closable` flag for a window.
    SetClosable { id: WindowId, closable: bool },
    /// Updates the tracked `focusable` flag for a window.
    SetFocusable { id: WindowId, focusable: bool },
    /// Updates the tracked `skip_taskbar` flag for a window.
    SetSkipTaskbar { id: WindowId, skip: bool },
    /// Asks the handler whether a window's `enabled` flag is set.
    QueryIsEnabled {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window's `maximizable` flag is set.
    QueryIsMaximizable {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window's `minimizable` flag is set.
    QueryIsMinimizable {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window's `closable` flag is set.
    QueryIsClosable {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Asks the handler whether a window's `always_on_top` flag is set.
    QueryIsAlwaysOnTop {
        id: WindowId,
        reply: mpsc::Sender<bool>,
    },
    /// Updates the tracked bounds (position + size) of a webview.
    SetWebviewBounds { webview_id: WebviewId, bounds: Rect },
    /// Updates the tracked size of a webview (position unchanged).
    SetWebviewSize { webview_id: WebviewId, size: Size },
    /// Updates the tracked position of a webview (size unchanged).
    SetWebviewPosition {
        webview_id: WebviewId,
        position: Position,
    },
    /// Asks the handler for the tracked bounds of a webview.
    QueryWebviewBounds {
        webview_id: WebviewId,
        reply: mpsc::Sender<Rect>,
    },
    /// Asks the handler for the tracked position of a webview.
    QueryWebviewPosition {
        webview_id: WebviewId,
        reply: mpsc::Sender<PhysicalPosition<i32>>,
    },
    /// Asks the handler for the tracked size of a webview.
    QueryWebviewSize {
        webview_id: WebviewId,
        reply: mpsc::Sender<PhysicalSize<u32>>,
    },
    /// Updates the tracked zoom factor of a webview.
    SetWebviewZoom { webview_id: WebviewId, scale: f64 },
    /// Updates the tracked background color of a webview.
    SetWebviewBackgroundColor {
        webview_id: WebviewId,
        color: Option<Color>,
    },
    /// Updates the tracked auto-resize flag of a webview.
    SetWebviewAutoResize {
        webview_id: WebviewId,
        auto_resize: bool,
    },
    /// Appends a cookie to the webview's in-memory cookie jar
    /// (replacing any existing entry with the same name).
    AddWebviewCookie {
        webview_id: WebviewId,
        cookie: Cookie<'static>,
    },
    /// Removes a cookie from the webview's in-memory cookie jar.
    RemoveWebviewCookie {
        webview_id: WebviewId,
        cookie: Cookie<'static>,
    },
    /// Clears the webview's in-memory cookie jar.
    ClearWebviewBrowsingData { webview_id: WebviewId },
    /// Asks the handler for the full cookie jar of a webview.
    QueryWebviewCookies {
        webview_id: WebviewId,
        reply: mpsc::Sender<Vec<Cookie<'static>>>,
    },
    /// Asks the handler for the cookies in a webview's jar matching
    /// the given URL's host.
    QueryWebviewCookiesForUrl {
        webview_id: WebviewId,
        url: Url,
        reply: mpsc::Sender<Vec<Cookie<'static>>>,
    },
    /// Rasterizes the webview's current frame to a PNG and writes it
    /// to the OS temp directory.  Used by [`WebviewDispatch::print`].
    PrintWebview { webview_id: WebviewId },
    /// Closes the window the given webview is currently attached to.
    /// Used by [`WebviewDispatch::close`]; the handler resolves the
    /// parent via `webview_to_window`.
    CloseWebviewParentWindow { webview_id: WebviewId },
    /// Focuses the window the given webview is currently attached to.
    /// Used by [`WebviewDispatch::set_focus`].
    FocusWebviewParentWindow { webview_id: WebviewId },
    /// Asks the runtime to exit with the given code.
    Exit { code: i32 },
}

impl<T: UserEvent> std::fmt::Debug for RuntimeEvent<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::User(_) => "User",
            Self::CreateWindow { .. } => "CreateWindow",
            Self::SetTitle { .. } => "SetTitle",
            Self::SetSize { .. } => "SetSize",
            Self::SetPosition { .. } => "SetPosition",
            Self::Show { .. } => "Show",
            Self::Hide { .. } => "Hide",
            Self::Focus { .. } => "Focus",
            Self::Close { .. } => "Close",
            Self::Destroy { .. } => "Destroy",
            Self::Maximize { .. } => "Maximize",
            Self::Unmaximize { .. } => "Unmaximize",
            Self::Minimize { .. } => "Minimize",
            Self::Unminimize { .. } => "Unminimize",
            Self::SetResizable { .. } => "SetResizable",
            Self::SetDecorations { .. } => "SetDecorations",
            Self::SetFullscreen { .. } => "SetFullscreen",
            Self::CreateWebview { .. } => "CreateWebview",
            Self::ReparentWebview { .. } => "ReparentWebview",
            Self::IpcRequest { .. } => "IpcRequest",
            Self::Navigate { .. } => "Navigate",
            Self::Reload { .. } => "Reload",
            Self::EvalScript { .. } => "EvalScript",
            Self::EvalScriptWithCallback { .. } => "EvalScriptWithCallback",
            Self::QueryInnerSize { .. } => "QueryInnerSize",
            Self::QueryOuterSize { .. } => "QueryOuterSize",
            Self::QueryInnerPosition { .. } => "QueryInnerPosition",
            Self::QueryOuterPosition { .. } => "QueryOuterPosition",
            Self::QueryScaleFactor { .. } => "QueryScaleFactor",
            Self::QueryTitle { .. } => "QueryTitle",
            Self::QueryIsVisible { .. } => "QueryIsVisible",
            Self::QueryIsFocused { .. } => "QueryIsFocused",
            Self::QueryIsMaximized { .. } => "QueryIsMaximized",
            Self::QueryIsMinimized { .. } => "QueryIsMinimized",
            Self::QueryIsFullscreen { .. } => "QueryIsFullscreen",
            Self::QueryIsDecorated { .. } => "QueryIsDecorated",
            Self::QueryIsResizable { .. } => "QueryIsResizable",
            Self::QueryTheme { .. } => "QueryTheme",
            Self::QueryPrimaryMonitor { .. } => "QueryPrimaryMonitor",
            Self::QueryAvailableMonitors { .. } => "QueryAvailableMonitors",
            Self::QueryMonitorFromPoint { .. } => "QueryMonitorFromPoint",
            Self::QueryCurrentMonitor { .. } => "QueryCurrentMonitor",
            Self::RunOnMainThread { .. } => "RunOnMainThread",
            Self::SetCursorVisible { .. } => "SetCursorVisible",
            Self::SetCursorGrab { .. } => "SetCursorGrab",
            Self::SetCursorIcon { .. } => "SetCursorIcon",
            Self::SetCursorPosition { .. } => "SetCursorPosition",
            Self::SetIgnoreCursorEvents { .. } => "SetIgnoreCursorEvents",
            Self::SetMinSize { .. } => "SetMinSize",
            Self::SetMaxSize { .. } => "SetMaxSize",
            Self::SetAlwaysOnTop { .. } => "SetAlwaysOnTop",
            Self::SetWindowTheme { .. } => "SetWindowTheme",
            Self::StartDragging { .. } => "StartDragging",
            Self::RequestUserAttention { .. } => "RequestUserAttention",
            Self::CenterWindow { .. } => "CenterWindow",
            Self::SetWindowIcon { .. } => "SetWindowIcon",
            Self::SetEnabled { .. } => "SetEnabled",
            Self::SetMaximizable { .. } => "SetMaximizable",
            Self::SetMinimizable { .. } => "SetMinimizable",
            Self::SetClosable { .. } => "SetClosable",
            Self::SetFocusable { .. } => "SetFocusable",
            Self::SetSkipTaskbar { .. } => "SetSkipTaskbar",
            Self::QueryIsEnabled { .. } => "QueryIsEnabled",
            Self::QueryIsMaximizable { .. } => "QueryIsMaximizable",
            Self::QueryIsMinimizable { .. } => "QueryIsMinimizable",
            Self::QueryIsClosable { .. } => "QueryIsClosable",
            Self::QueryIsAlwaysOnTop { .. } => "QueryIsAlwaysOnTop",
            Self::SetWebviewBounds { .. } => "SetWebviewBounds",
            Self::SetWebviewSize { .. } => "SetWebviewSize",
            Self::SetWebviewPosition { .. } => "SetWebviewPosition",
            Self::QueryWebviewBounds { .. } => "QueryWebviewBounds",
            Self::QueryWebviewPosition { .. } => "QueryWebviewPosition",
            Self::QueryWebviewSize { .. } => "QueryWebviewSize",
            Self::SetWebviewZoom { .. } => "SetWebviewZoom",
            Self::SetWebviewBackgroundColor { .. } => "SetWebviewBackgroundColor",
            Self::SetWebviewAutoResize { .. } => "SetWebviewAutoResize",
            Self::AddWebviewCookie { .. } => "AddWebviewCookie",
            Self::RemoveWebviewCookie { .. } => "RemoveWebviewCookie",
            Self::ClearWebviewBrowsingData { .. } => "ClearWebviewBrowsingData",
            Self::QueryWebviewCookies { .. } => "QueryWebviewCookies",
            Self::QueryWebviewCookiesForUrl { .. } => "QueryWebviewCookiesForUrl",
            Self::PrintWebview { .. } => "PrintWebview",
            Self::CloseWebviewParentWindow { .. } => "CloseWebviewParentWindow",
            Self::FocusWebviewParentWindow { .. } => "FocusWebviewParentWindow",
            Self::Exit { .. } => "Exit",
        };
        formatter.write_str(name)
    }
}

/// The Servocat runtime.  Owns the winit event loop until
/// [`Runtime::run`] is called.
pub struct ServocatRuntime<T: UserEvent> {
    event_loop: Option<EventLoop<RuntimeEvent<T>>>,
    proxy: EventLoopProxy<RuntimeEvent<T>>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatRuntime<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServocatRuntime")
            .field("event_loop", &self.event_loop.is_some())
            .finish_non_exhaustive()
    }
}

/// `Send`-able handle to a [`ServocatRuntime`].  Holds the proxy used
/// to enqueue [`RuntimeEvent`]s.
pub struct ServocatHandle<T: UserEvent> {
    proxy: EventLoopProxy<RuntimeEvent<T>>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatHandle<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServocatHandle")
            .finish_non_exhaustive()
    }
}

impl<T: UserEvent> Clone for ServocatHandle<T> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
        }
    }
}

/// `Send`-able dispatcher for [`ServocatRuntime`] windows.  Holds the
/// proxy + the dispatched window's id.
pub struct ServocatWindowDispatch<T: UserEvent> {
    proxy: EventLoopProxy<RuntimeEvent<T>>,
    window_id: WindowId,
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatWindowDispatch<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServocatWindowDispatch")
            .field("window_id", &self.window_id)
            .finish_non_exhaustive()
    }
}

impl<T: UserEvent> Clone for ServocatWindowDispatch<T> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
            window_id: self.window_id,
            _marker: PhantomData,
        }
    }
}

/// `Send`-able dispatcher for [`ServocatRuntime`] webviews.  Holds
/// a stable [`WebviewId`] that survives a `reparent(new_window_id)`
/// call -- the handler maintains a `webview_to_window` map so all
/// events on this dispatcher continue to address the same webview at
/// its current parent window.
pub struct ServocatWebviewDispatch<T: UserEvent> {
    proxy: EventLoopProxy<RuntimeEvent<T>>,
    webview_id: WebviewId,
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatWebviewDispatch<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServocatWebviewDispatch")
            .field("webview_id", &self.webview_id)
            .finish_non_exhaustive()
    }
}

impl<T: UserEvent> Clone for ServocatWebviewDispatch<T> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
            webview_id: self.webview_id,
            _marker: PhantomData,
        }
    }
}

impl<T: UserEvent> ServocatWebviewDispatch<T> {
    /// v1.5 Rust-side IPC bridge: fires the `ipc_handler` that was
    /// supplied to [`PendingWebview::ipc_handler`] when this webview
    /// was created.  The host's handler runs on the event-loop
    /// thread; results can be delivered back to JS via the standard
    /// `WebviewDispatch::eval_script` machinery (which Tauri's
    /// command system already does internally).
    ///
    /// Wiring this to a JS-side `__TAURI_INTERNALS__.postMessage`
    /// shim is left for a future 1.5.x release.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EventLoopClosed`] if the event loop has
    /// already exited.
    pub fn send_ipc(&self, request: http::Request<String>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::IpcRequest {
                webview_id: self.webview_id,
                request,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }
}

/// `Send`-able event-loop proxy.  Wraps winit's
/// [`EventLoopProxy<RuntimeEvent<T>>`] so callers can dispatch
/// user-typed events.
pub struct ServocatEventLoopProxy<T: UserEvent> {
    proxy: EventLoopProxy<RuntimeEvent<T>>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatEventLoopProxy<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServocatEventLoopProxy")
            .finish_non_exhaustive()
    }
}

impl<T: UserEvent> Clone for ServocatEventLoopProxy<T> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
        }
    }
}

/// Builder for [`ServocatRuntime`] windows.  v1.1 captures the
/// attributes that map cleanly onto winit's `WindowAttributes` so
/// queued windows are actually constructed with the requested size,
/// title, etc.
#[derive(Debug, Default, Clone)]
pub struct ServocatWindowBuilder {
    title: Option<String>,
    inner_size: Option<(f64, f64)>,
    min_inner_size: Option<(f64, f64)>,
    max_inner_size: Option<(f64, f64)>,
    position: Option<(f64, f64)>,
    resizable: Option<bool>,
    maximized: Option<bool>,
    visible: Option<bool>,
    decorations: Option<bool>,
    fullscreen: Option<bool>,
    transparent: Option<bool>,
    has_icon: bool,
    theme: Option<Theme>,
}

impl ServocatWindowBuilder {
    fn to_winit_attributes(&self) -> WindowAttributes {
        let base = Window::default_attributes();
        let with_title = self
            .title
            .as_ref()
            .map_or(base.clone(), |title| base.clone().with_title(title));
        let with_inner = self.inner_size.map_or(with_title.clone(), |(w, h)| {
            with_title.clone().with_inner_size(LogicalSize::new(w, h))
        });
        let with_min = self.min_inner_size.map_or(with_inner.clone(), |(w, h)| {
            with_inner
                .clone()
                .with_min_inner_size(LogicalSize::new(w, h))
        });
        let with_max = self.max_inner_size.map_or(with_min.clone(), |(w, h)| {
            with_min.clone().with_max_inner_size(LogicalSize::new(w, h))
        });
        let with_position = self.position.map_or(with_max.clone(), |(x, y)| {
            with_max.clone().with_position(LogicalPosition::new(x, y))
        });
        let with_resizable = self.resizable.map_or(with_position.clone(), |resizable| {
            with_position.clone().with_resizable(resizable)
        });
        let with_maximized = self.maximized.map_or(with_resizable.clone(), |maximized| {
            with_resizable.clone().with_maximized(maximized)
        });
        let with_visible = self.visible.map_or(with_maximized.clone(), |visible| {
            with_maximized.clone().with_visible(visible)
        });
        let with_decorations = self
            .decorations
            .map_or(with_visible.clone(), |decorations| {
                with_visible.clone().with_decorations(decorations)
            });
        let with_fullscreen =
            self.fullscreen
                .filter(|f| *f)
                .map_or(with_decorations.clone(), |_fullscreen| {
                    with_decorations
                        .clone()
                        .with_fullscreen(Some(Fullscreen::Borderless(None)))
                });
        self.transparent
            .map_or(with_fullscreen.clone(), |transparent| {
                with_fullscreen.clone().with_transparent(transparent)
            })
    }
}

impl WindowBuilderBase for ServocatWindowBuilder {}

impl WindowBuilder for ServocatWindowBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn with_config(_config: &WindowConfig) -> Self {
        Self::default()
    }

    fn center(self) -> Self {
        self
    }

    fn position(self, x: f64, y: f64) -> Self {
        Self {
            position: Some((x, y)),
            ..self
        }
    }

    fn inner_size(self, width: f64, height: f64) -> Self {
        Self {
            inner_size: Some((width, height)),
            ..self
        }
    }

    fn min_inner_size(self, min_width: f64, min_height: f64) -> Self {
        Self {
            min_inner_size: Some((min_width, min_height)),
            ..self
        }
    }

    fn max_inner_size(self, max_width: f64, max_height: f64) -> Self {
        Self {
            max_inner_size: Some((max_width, max_height)),
            ..self
        }
    }

    fn inner_size_constraints(self, _constraints: WindowSizeConstraints) -> Self {
        self
    }

    fn prevent_overflow(self) -> Self {
        self
    }

    fn prevent_overflow_with_margin(self, _margin: tauri_runtime::dpi::Size) -> Self {
        self
    }

    fn resizable(self, resizable: bool) -> Self {
        Self {
            resizable: Some(resizable),
            ..self
        }
    }

    fn maximizable(self, _maximizable: bool) -> Self {
        self
    }

    fn minimizable(self, _minimizable: bool) -> Self {
        self
    }

    fn closable(self, _closable: bool) -> Self {
        self
    }

    fn title<S: Into<String>>(self, title: S) -> Self {
        Self {
            title: Some(title.into()),
            ..self
        }
    }

    fn fullscreen(self, fullscreen: bool) -> Self {
        Self {
            fullscreen: Some(fullscreen),
            ..self
        }
    }

    fn focused(self, _focused: bool) -> Self {
        self
    }

    fn focusable(self, _focusable: bool) -> Self {
        self
    }

    fn maximized(self, maximized: bool) -> Self {
        Self {
            maximized: Some(maximized),
            ..self
        }
    }

    fn visible(self, visible: bool) -> Self {
        Self {
            visible: Some(visible),
            ..self
        }
    }

    #[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]
    fn transparent(self, transparent: bool) -> Self {
        Self {
            transparent: Some(transparent),
            ..self
        }
    }

    fn decorations(self, decorations: bool) -> Self {
        Self {
            decorations: Some(decorations),
            ..self
        }
    }

    fn always_on_bottom(self, _always_on_bottom: bool) -> Self {
        self
    }

    fn always_on_top(self, _always_on_top: bool) -> Self {
        self
    }

    fn visible_on_all_workspaces(self, _visible_on_all_workspaces: bool) -> Self {
        self
    }

    fn content_protected(self, _protected: bool) -> Self {
        self
    }

    fn icon(self, _icon: Icon<'_>) -> Result<Self> {
        Ok(Self {
            has_icon: true,
            ..self
        })
    }

    fn skip_taskbar(self, _skip: bool) -> Self {
        self
    }

    fn background_color(self, _color: Color) -> Self {
        self
    }

    fn shadow(self, _enable: bool) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn parent(self, _parent: *mut std::ffi::c_void) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn title_bar_style(self, _style: TitleBarStyle) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn traffic_light_position<P: Into<tauri_runtime::dpi::Position>>(self, _position: P) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn hidden_title(self, _hidden: bool) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn tabbing_identifier(self, _identifier: &str) -> Self {
        self
    }

    fn theme(self, theme: Option<Theme>) -> Self {
        Self { theme, ..self }
    }

    fn has_icon(&self) -> bool {
        self.has_icon
    }

    fn get_theme(&self) -> Option<Theme> {
        self.theme
    }

    fn window_classname<S: Into<String>>(self, _window_classname: S) -> Self {
        self
    }
}

impl<T: UserEvent> TauriEventLoopProxy<T> for ServocatEventLoopProxy<T> {
    fn send_event(&self, event: T) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::User(event))
            .map_err(|_e| Error::EventLoopClosed)
    }
}

impl<T: UserEvent> Runtime<T> for ServocatRuntime<T> {
    type WindowDispatcher = ServocatWindowDispatch<T>;
    type WebviewDispatcher = ServocatWebviewDispatch<T>;
    type Handle = ServocatHandle<T>;
    type EventLoopProxy = ServocatEventLoopProxy<T>;

    fn new(_args: RuntimeInitArgs) -> Result<Self> {
        let event_loop = EventLoop::<RuntimeEvent<T>>::with_user_event()
            .build()
            .map_err(|_e| Error::CreateWindow)?;
        let proxy = event_loop.create_proxy();
        Ok(Self {
            event_loop: Some(event_loop),
            proxy,
        })
    }

    fn create_proxy(&self) -> Self::EventLoopProxy {
        ServocatEventLoopProxy {
            proxy: self.proxy.clone(),
        }
    }

    fn handle(&self) -> Self::Handle {
        ServocatHandle {
            proxy: self.proxy.clone(),
        }
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        pending: PendingWindow<T, Self>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self>> {
        queue_window(&self.proxy, pending)
    }

    fn create_webview(
        &self,
        window_id: WindowId,
        pending: PendingWebview<T, Self>,
    ) -> Result<DetachedWebview<T, Self>> {
        queue_webview(&self.proxy, window_id, pending)
    }

    fn primary_monitor(&self) -> Option<Monitor> {
        // Pre-run monitor enumeration isn't exposed by winit 0.30's
        // `EventLoop` (monitor methods moved to `ActiveEventLoop`,
        // only available from inside the handler).  Callers should
        // use `RuntimeHandle::primary_monitor` after `Runtime::run`
        // starts; this method returns `None` to keep the Tauri trait
        // contract intact.
        None
    }

    fn monitor_from_point(&self, _x: f64, _y: f64) -> Option<Monitor> {
        None
    }

    fn available_monitors(&self) -> Vec<Monitor> {
        Vec::new()
    }

    fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
        Err(Error::FailedToGetCursorPosition)
    }

    fn set_theme(&self, _theme: Option<Theme>) {}

    #[cfg(target_os = "macos")]
    fn set_activation_policy(&mut self, _activation_policy: tauri_runtime::ActivationPolicy) {}

    #[cfg(target_os = "macos")]
    fn set_dock_visibility(&mut self, _visible: bool) {}

    #[cfg(target_os = "macos")]
    fn show(&self) {}

    #[cfg(target_os = "macos")]
    fn hide(&self) {}

    fn set_device_event_filter(&mut self, _filter: DeviceEventFilter) {}

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    fn run_iteration<F: FnMut(RunEvent<T>) + 'static>(&mut self, _callback: F) {}

    fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, callback: F) -> i32 {
        self.run(callback);
        0
    }

    fn run<F: FnMut(RunEvent<T>) + 'static>(mut self, callback: F) {
        let proxy = self.proxy.clone();
        let _ = self.event_loop.take().map(|event_loop| {
            let mut app = AppHandler::new(callback, proxy);
            let _ = event_loop.run_app(&mut app);
        });
    }
}

impl<T: UserEvent> RuntimeHandle<T> for ServocatHandle<T> {
    type Runtime = ServocatRuntime<T>;

    fn create_proxy(&self) -> <Self::Runtime as Runtime<T>>::EventLoopProxy {
        ServocatEventLoopProxy {
            proxy: self.proxy.clone(),
        }
    }

    #[cfg(target_os = "macos")]
    fn set_activation_policy(
        &self,
        _activation_policy: tauri_runtime::ActivationPolicy,
    ) -> Result<()> {
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn set_dock_visibility(&self, _visible: bool) -> Result<()> {
        Ok(())
    }

    fn request_exit(&self, code: i32) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Exit { code })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        queue_window(&self.proxy, pending)
    }

    fn create_webview(
        &self,
        window_id: WindowId,
        pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        queue_webview(&self.proxy, window_id, pending)
    }

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::RunOnMainThread { thunk: Box::new(f) })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }

    fn primary_monitor(&self) -> Option<Monitor> {
        query(&self.proxy, |reply| RuntimeEvent::QueryPrimaryMonitor {
            reply,
        })
        .ok()
        .flatten()
    }

    fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
        query(&self.proxy, |reply| RuntimeEvent::QueryMonitorFromPoint {
            x,
            y,
            reply,
        })
        .ok()
        .flatten()
    }

    fn available_monitors(&self) -> Vec<Monitor> {
        query(&self.proxy, |reply| RuntimeEvent::QueryAvailableMonitors {
            reply,
        })
        .unwrap_or_default()
    }

    fn cursor_position(&self) -> Result<PhysicalPosition<f64>> {
        Err(Error::FailedToGetCursorPosition)
    }

    fn set_theme(&self, _theme: Option<Theme>) {}

    #[cfg(target_os = "macos")]
    fn show(&self) -> Result<()> {
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn hide(&self) -> Result<()> {
        Ok(())
    }

    fn set_device_event_filter(&self, _filter: DeviceEventFilter) {}

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn fetch_data_store_identifiers<F: FnOnce(Vec<[u8; 16]>) + Send + 'static>(
        &self,
        _cb: F,
    ) -> Result<()> {
        Err(Error::FailedToRemoveDataStore)
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn remove_data_store<F: FnOnce(Result<()>) + Send + 'static>(
        &self,
        _uuid: [u8; 16],
        _cb: F,
    ) -> Result<()> {
        Err(Error::FailedToRemoveDataStore)
    }
}

impl<T: UserEvent> WindowDispatch<T> for ServocatWindowDispatch<T> {
    type Runtime = ServocatRuntime<T>;
    type WindowBuilder = ServocatWindowBuilder;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::RunOnMainThread { thunk: Box::new(f) })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn on_window_event<F: Fn(&TauriWindowEvent) + Send + 'static>(&self, _f: F) -> WindowEventId {
        0
    }

    fn scale_factor(&self) -> Result<f64> {
        query(&self.proxy, |reply| RuntimeEvent::QueryScaleFactor {
            id: self.window_id,
            reply,
        })
    }

    fn inner_position(&self) -> Result<PhysicalPosition<i32>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryInnerPosition {
            id: self.window_id,
            reply,
        })
    }

    fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryOuterPosition {
            id: self.window_id,
            reply,
        })
    }

    fn inner_size(&self) -> Result<PhysicalSize<u32>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryInnerSize {
            id: self.window_id,
            reply,
        })
    }

    fn outer_size(&self) -> Result<PhysicalSize<u32>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryOuterSize {
            id: self.window_id,
            reply,
        })
    }

    fn is_fullscreen(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsFullscreen {
            id: self.window_id,
            reply,
        })
    }

    fn is_minimized(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsMinimized {
            id: self.window_id,
            reply,
        })
    }

    fn is_maximized(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsMaximized {
            id: self.window_id,
            reply,
        })
    }

    fn is_focused(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsFocused {
            id: self.window_id,
            reply,
        })
    }

    fn is_decorated(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsDecorated {
            id: self.window_id,
            reply,
        })
    }

    fn is_resizable(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsResizable {
            id: self.window_id,
            reply,
        })
    }

    fn is_maximizable(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsMaximizable {
            id: self.window_id,
            reply,
        })
    }

    fn is_minimizable(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsMinimizable {
            id: self.window_id,
            reply,
        })
    }

    fn is_closable(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsClosable {
            id: self.window_id,
            reply,
        })
    }

    fn is_visible(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsVisible {
            id: self.window_id,
            reply,
        })
    }

    fn is_enabled(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsEnabled {
            id: self.window_id,
            reply,
        })
    }

    fn is_always_on_top(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsAlwaysOnTop {
            id: self.window_id,
            reply,
        })
    }

    fn title(&self) -> Result<String> {
        query(&self.proxy, |reply| RuntimeEvent::QueryTitle {
            id: self.window_id,
            reply,
        })
    }

    fn current_monitor(&self) -> Result<Option<Monitor>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryCurrentMonitor {
            id: self.window_id,
            reply,
        })
    }

    fn primary_monitor(&self) -> Result<Option<Monitor>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryPrimaryMonitor {
            reply,
        })
    }

    fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryMonitorFromPoint {
            x,
            y,
            reply,
        })
    }

    fn available_monitors(&self) -> Result<Vec<Monitor>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryAvailableMonitors {
            reply,
        })
    }

    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }

    fn theme(&self) -> Result<Theme> {
        query(&self.proxy, |reply| RuntimeEvent::QueryTheme {
            id: self.window_id,
            reply,
        })
    }

    fn center(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::CenterWindow { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn request_user_attention(
        &self,
        request_type: Option<tauri_runtime::UserAttentionType>,
    ) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::RequestUserAttention {
                id: self.window_id,
                request_type,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &mut self,
        pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        queue_window(&self.proxy, pending)
    }

    fn create_webview(
        &mut self,
        pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        queue_webview(&self.proxy, self.window_id, pending)
    }

    fn set_resizable(&self, resizable: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetResizable {
                id: self.window_id,
                resizable,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetEnabled {
                id: self.window_id,
                enabled,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_maximizable(&self, maximizable: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetMaximizable {
                id: self.window_id,
                maximizable,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_minimizable(&self, minimizable: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetMinimizable {
                id: self.window_id,
                minimizable,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_closable(&self, closable: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetClosable {
                id: self.window_id,
                closable,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetTitle {
                id: self.window_id,
                title: title.into(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn maximize(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Maximize { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn unmaximize(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Unmaximize { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn minimize(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Minimize { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn unminimize(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Unminimize { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn show(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Show { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn hide(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Hide { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn close(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Close { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn destroy(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Destroy { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_decorations(&self, decorations: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetDecorations {
                id: self.window_id,
                decorations,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_shadow(&self, _enable: bool) -> Result<()> {
        Ok(())
    }

    fn set_always_on_bottom(&self, _always_on_bottom: bool) -> Result<()> {
        Ok(())
    }

    fn set_always_on_top(&self, always_on_top: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetAlwaysOnTop {
                id: self.window_id,
                always_on_top,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_visible_on_all_workspaces(&self, _visible_on_all_workspaces: bool) -> Result<()> {
        Ok(())
    }

    fn set_background_color(&self, _color: Option<Color>) -> Result<()> {
        Ok(())
    }

    fn set_content_protected(&self, _protected: bool) -> Result<()> {
        Ok(())
    }

    fn set_size(&self, size: Size) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetSize {
                id: self.window_id,
                size,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_min_size(&self, size: Option<Size>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetMinSize {
                id: self.window_id,
                size,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_max_size(&self, size: Option<Size>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetMaxSize {
                id: self.window_id,
                size,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_size_constraints(&self, _constraints: WindowSizeConstraints) -> Result<()> {
        Ok(())
    }

    fn set_position(&self, position: Position) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetPosition {
                id: self.window_id,
                position,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_fullscreen(&self, fullscreen: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetFullscreen {
                id: self.window_id,
                fullscreen,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    #[cfg(target_os = "macos")]
    fn set_simple_fullscreen(&self, _enable: bool) -> Result<()> {
        Ok(())
    }

    fn set_focus(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Focus { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_focusable(&self, focusable: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetFocusable {
                id: self.window_id,
                focusable,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_icon(&self, icon: Icon<'_>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWindowIcon {
                id: self.window_id,
                rgba: icon.rgba.into_owned(),
                width: icon.width,
                height: icon.height,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_skip_taskbar(&self, skip: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetSkipTaskbar {
                id: self.window_id,
                skip,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_cursor_grab(&self, grab: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetCursorGrab {
                id: self.window_id,
                grab,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_cursor_visible(&self, visible: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetCursorVisible {
                id: self.window_id,
                visible,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_cursor_icon(&self, icon: CursorIcon) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetCursorIcon {
                id: self.window_id,
                icon,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_cursor_position<Pos: Into<Position>>(&self, position: Pos) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetCursorPosition {
                id: self.window_id,
                position: position.into(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_ignore_cursor_events(&self, ignore: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetIgnoreCursorEvents {
                id: self.window_id,
                ignore,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn start_dragging(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::StartDragging { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn start_resize_dragging(&self, _direction: tauri_runtime::ResizeDirection) -> Result<()> {
        Ok(())
    }

    fn set_badge_count(
        &self,
        _count: Option<i64>,
        _desktop_filename: Option<String>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_badge_label(&self, _label: Option<String>) -> Result<()> {
        Ok(())
    }

    fn set_overlay_icon(&self, _icon: Option<Icon<'_>>) -> Result<()> {
        Ok(())
    }

    fn set_progress_bar(&self, _progress_state: ProgressBarState) -> Result<()> {
        Ok(())
    }

    fn set_title_bar_style(&self, _style: TitleBarStyle) -> Result<()> {
        Ok(())
    }

    fn set_traffic_light_position(&self, _position: Position) -> Result<()> {
        Ok(())
    }

    fn set_theme(&self, theme: Option<Theme>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWindowTheme {
                id: self.window_id,
                theme,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }
}

impl<T: UserEvent> WebviewDispatch<T> for ServocatWebviewDispatch<T> {
    type Runtime = ServocatRuntime<T>;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::RunOnMainThread { thunk: Box::new(f) })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn on_webview_event<F: Fn(&WebviewEvent) + Send + 'static>(&self, _f: F) -> WebviewEventId {
        0
    }

    fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(&self, f: F) -> Result<()> {
        // FFI carve-out: `Box<dyn std::any::Any>` is the parameter
        // shape the `tauri_runtime::WebviewDispatch::with_webview`
        // trait method requires.  No native webview handle exists in
        // the cat-stack runtime, so the closure can't introspect a
        // platform webview; we still ship it to the main thread with
        // `Box::new(())` so callers that use `with_webview` purely
        // to schedule a host-side closure on the main thread (a
        // common Tauri pattern) get the synchronous-dispatch
        // behaviour they expect.
        let thunk: Box<dyn FnOnce() + Send + 'static> = Box::new(move || {
            f(Box::new(()));
        });
        self.proxy
            .send_event(RuntimeEvent::RunOnMainThread { thunk })
            .map_err(|_e| Error::EventLoopClosed)
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn open_devtools(&self) {}

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn close_devtools(&self) {}

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn is_devtools_open(&self) -> Result<bool> {
        Ok(false)
    }

    fn url(&self) -> Result<String> {
        Ok(String::new())
    }

    fn bounds(&self) -> Result<Rect> {
        query(&self.proxy, |reply| RuntimeEvent::QueryWebviewBounds {
            webview_id: self.webview_id,
            reply,
        })
    }

    fn position(&self) -> Result<PhysicalPosition<i32>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryWebviewPosition {
            webview_id: self.webview_id,
            reply,
        })
    }

    fn size(&self) -> Result<PhysicalSize<u32>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryWebviewSize {
            webview_id: self.webview_id,
            reply,
        })
    }

    fn navigate(&self, url: Url) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Navigate {
                webview_id: self.webview_id,
                url: url.into(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn reload(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Reload {
                webview_id: self.webview_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn print(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::PrintWebview {
                webview_id: self.webview_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn close(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::CloseWebviewParentWindow {
                webview_id: self.webview_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_bounds(&self, bounds: Rect) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWebviewBounds {
                webview_id: self.webview_id,
                bounds,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_size(&self, size: Size) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWebviewSize {
                webview_id: self.webview_id,
                size,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_position(&self, position: Position) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWebviewPosition {
                webview_id: self.webview_id,
                position,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_focus(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::FocusWebviewParentWindow {
                webview_id: self.webview_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn hide(&self) -> Result<()> {
        Ok(())
    }

    fn show(&self) -> Result<()> {
        Ok(())
    }

    fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::EvalScript {
                webview_id: self.webview_id,
                script: script.into(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn eval_script_with_callback<S: Into<String>>(
        &self,
        script: S,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        // FFI carve-out: storing user closures in a `RuntimeEvent`
        // variant requires `Box<dyn Fn(String) + Send + 'static>`
        // because `winit::EventLoop<RuntimeEvent<T>>` is parameterized
        // on a concrete enum -- closures with arbitrary captures can't
        // be encoded in a per-variant generic.
        self.proxy
            .send_event(RuntimeEvent::EvalScriptWithCallback {
                webview_id: self.webview_id,
                script: script.into(),
                callback: Box::new(callback),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn reparent(&self, window_id: WindowId) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::ReparentWebview {
                webview_id: self.webview_id,
                new_window_id: window_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn cookies_for_url(&self, url: Url) -> Result<Vec<Cookie<'static>>> {
        query(&self.proxy, |reply| {
            RuntimeEvent::QueryWebviewCookiesForUrl {
                webview_id: self.webview_id,
                url,
                reply,
            }
        })
    }

    fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
        query(&self.proxy, |reply| RuntimeEvent::QueryWebviewCookies {
            webview_id: self.webview_id,
            reply,
        })
    }

    fn set_cookie(&self, cookie: Cookie<'_>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::AddWebviewCookie {
                webview_id: self.webview_id,
                cookie: cookie.into_owned(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn delete_cookie(&self, cookie: Cookie<'_>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::RemoveWebviewCookie {
                webview_id: self.webview_id,
                cookie: cookie.into_owned(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_auto_resize(&self, auto_resize: bool) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWebviewAutoResize {
                webview_id: self.webview_id,
                auto_resize,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_zoom(&self, scale_factor: f64) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWebviewZoom {
                webview_id: self.webview_id,
                scale: scale_factor,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_background_color(&self, color: Option<Color>) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::SetWebviewBackgroundColor {
                webview_id: self.webview_id,
                color,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn clear_all_browsing_data(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::ClearWebviewBrowsingData {
                webview_id: self.webview_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }
}

fn queue_window<T: UserEvent>(
    proxy: &EventLoopProxy<RuntimeEvent<T>>,
    pending: PendingWindow<T, ServocatRuntime<T>>,
) -> Result<DetachedWindow<T, ServocatRuntime<T>>> {
    let id = allocate_window_id();
    let label = pending.label;
    let attributes = pending.window_builder.clone();
    proxy
        .send_event(RuntimeEvent::CreateWindow {
            id,
            label: label.clone(),
            attributes,
        })
        .map_err(|_e| Error::CreateWindow)?;
    Ok(DetachedWindow {
        id,
        label,
        dispatcher: ServocatWindowDispatch {
            proxy: proxy.clone(),
            window_id: id,
            _marker: PhantomData,
        },
        webview: None,
    })
}

/// Block the calling thread waiting for a reply from the event loop.
/// Used by [`WindowDispatch`] getters to round-trip a query through
/// the handler and back.
fn query<T, R, F>(proxy: &EventLoopProxy<RuntimeEvent<T>>, builder: F) -> Result<R>
where
    T: UserEvent,
    R: Send + 'static,
    F: FnOnce(mpsc::Sender<R>) -> RuntimeEvent<T>,
{
    let (tx, rx) = mpsc::channel::<R>();
    proxy
        .send_event(builder(tx))
        .map_err(|_e| Error::EventLoopClosed)?;
    rx.recv_timeout(Duration::from_millis(500))
        .map_err(|_e| Error::FailedToReceiveMessage)
}

fn queue_webview<T: UserEvent>(
    proxy: &EventLoopProxy<RuntimeEvent<T>>,
    window_id: WindowId,
    pending: PendingWebview<T, ServocatRuntime<T>>,
) -> Result<DetachedWebview<T, ServocatRuntime<T>>> {
    let label = pending.label;
    let webview_id = allocate_webview_id();
    proxy
        .send_event(RuntimeEvent::CreateWebview {
            window_id,
            webview_id,
            label: label.clone(),
            url: pending.url,
            ipc_handler: pending.ipc_handler,
        })
        .map_err(|err| Error::CreateWebview(skeleton_error_from(err.to_string())))?;
    Ok(DetachedWebview {
        label,
        dispatcher: ServocatWebviewDispatch {
            proxy: proxy.clone(),
            webview_id,
            _marker: PhantomData,
        },
    })
}

/// JS-side -> Rust-side IPC dispatch trait.  Implemented for each
/// `T: UserEvent`, but stored type-erased in [`IPC_DISPATCH`] so
/// boa-cat's bare-fn-pointer [`NativeFn`] shim can reach it.
trait IpcDispatch {
    fn dispatch(&self, request: http::Request<String>);
}

struct IpcDispatchImpl<T: UserEvent> {
    proxy: EventLoopProxy<RuntimeEvent<T>>,
    webview_id: WebviewId,
}

impl<T: UserEvent> IpcDispatch for IpcDispatchImpl<T> {
    fn dispatch(&self, request: http::Request<String>) {
        let _ = self.proxy.send_event(RuntimeEvent::IpcRequest {
            webview_id: self.webview_id,
            request,
        });
    }
}

thread_local! {
    // FFI carve-out: boa-cat's `NativeFn` is a bare `fn` pointer
    // (no closure capture); we stash a type-erased dispatcher in
    // thread-local storage for the duration of one eval so the
    // `post_ipc_message` shim can reach the runtime's event-loop
    // proxy.  Set + cleared by [`AppHandler::eval_with_ipc_bridge`].
    static IPC_DISPATCH: RefCell<Option<Box<dyn IpcDispatch>>> =
        const { RefCell::new(None) };
}

/// `__TAURI__.post_ipc_message(payload)` shim: stringifies its first
/// argument, wraps it in an `http::Request<String>` with the
/// `ipc://post-message` URI, and fires a [`RuntimeEvent::IpcRequest`]
/// through the thread-local dispatcher set by the current eval.
///
/// # Errors
///
/// Never returns `Err`; if the request fails to build or no
/// dispatcher is active the message is silently dropped (no
/// dispatcher means we're being called outside a webview eval).
#[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
fn post_ipc_message_impl(args: Vec<Value>, _this: Value, heap: BoaHeap, fuel: Fuel) -> EvalResult {
    let body = args.first().map_or_else(String::new, |value| match value {
        Value::String(s) => s.clone(),
        Value::Undefined
        | Value::Null
        | Value::Boolean(_)
        | Value::Number(_)
        | Value::Object(_)
        | Value::Function(_)
        | Value::Native(_) => format!("{value}"),
    });
    let _ = http::Request::builder()
        .uri("ipc://post-message")
        .body(body)
        .ok()
        .map(|request| {
            IPC_DISPATCH.with(|slot| {
                let _ = slot
                    .borrow()
                    .as_ref()
                    .map(|dispatcher| dispatcher.dispatch(request));
            });
        });
    Ok((Outcome::Normal(Value::Undefined), heap, fuel))
}

/// Per-window flag bookkeeping for methods that have no direct winit
/// equivalent (`is_enabled` / `is_maximizable` / `is_minimizable` /
/// `is_closable` / `is_focusable`) plus the always-on-top mirror.
/// Default values follow winit's defaults: enabled, max/min/closable,
/// and focusable on; always-on-top off.  Boolean count is the trait
/// surface's natural shape; the lint isn't useful here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
struct WindowFlags {
    enabled: bool,
    maximizable: bool,
    minimizable: bool,
    closable: bool,
    always_on_top: bool,
    focusable: bool,
    skip_taskbar: bool,
}

impl Default for WindowFlags {
    fn default() -> Self {
        Self {
            enabled: true,
            maximizable: true,
            minimizable: true,
            closable: true,
            always_on_top: false,
            focusable: true,
            skip_taskbar: false,
        }
    }
}

/// Per-window webview state held by [`AppHandler`].  v1.2 keeps the
/// caller-supplied HTML/CSS so `reload` + `eval_script` can rebuild
/// the pipeline without re-fetching; v1.5 adds the label + optional
/// `ipc_handler` so [`ServocatWebviewDispatch::send_ipc`] can route
/// requests to the host's `invoke_handler`; v1.7 adds a tracked
/// bounds rect that `set_bounds` / `set_size` / `set_position` update
/// and `bounds()` / `position()` / `size()` read.
struct WebviewState<T: UserEvent> {
    label: String,
    html: String,
    css: String,
    viewport: Viewport,
    current_frame: Option<Frame>,
    text_renderer: TextRenderer,
    ipc_handler: Option<WebviewIpcHandler<T, ServocatRuntime<T>>>,
    bounds: Rect,
    zoom: f64,
    background_color: Option<Color>,
    auto_resize: bool,
    cookies: Vec<Cookie<'static>>,
}

impl<T: UserEvent> WebviewState<T> {
    fn new(
        label: String,
        html: String,
        viewport: Viewport,
        ipc_handler: Option<WebviewIpcHandler<T, ServocatRuntime<T>>>,
    ) -> Self {
        Self {
            label,
            html,
            css: String::new(),
            viewport,
            current_frame: None,
            text_renderer: TextRenderer::new(),
            ipc_handler,
            bounds: Rect {
                position: Position::Physical(PhysicalPosition::new(0, 0)),
                size: tauri_runtime::dpi::Size::Physical(PhysicalSize::new(
                    viewport.width(),
                    viewport.height(),
                )),
            },
            zoom: 1.0,
            background_color: None,
            auto_resize: false,
            cookies: Vec::new(),
        }
    }

    fn rebuild_frame(&mut self) {
        self.current_frame = crate::pipeline::render(&self.html, &self.css, self.viewport).ok();
    }

    fn eval(&mut self, script: &str) -> Option<String> {
        let frame = run_script_with_backprop(
            &self.html,
            &self.css,
            script,
            self.viewport,
            &HostCommands::new().with("post_ipc_message", post_ipc_message_impl),
        )
        .ok()?;
        let value = format!("{}", frame.script_value());
        self.current_frame = Some(frame);
        Some(value)
    }
}

/// Install a thread-local IPC dispatcher pointing at the given
/// window for the duration of `body`, then clear it.  Used by the
/// `EvalScript` / `EvalScriptWithCallback` handler arms so the
/// `post_ipc_message` shim can reach the right window's `ipc_handler`.
fn with_ipc_dispatch<T: UserEvent, R>(
    proxy: EventLoopProxy<RuntimeEvent<T>>,
    webview_id: WebviewId,
    body: impl FnOnce() -> R,
) -> R {
    let dispatcher: Box<dyn IpcDispatch> = Box::new(IpcDispatchImpl { proxy, webview_id });
    IPC_DISPATCH.with(|slot| {
        let _ = slot.borrow_mut().replace(dispatcher);
    });
    let result = body();
    IPC_DISPATCH.with(|slot| {
        let _ = slot.borrow_mut().take();
    });
    result
}

/// Extract a usable HTML body from a URL plus any cookies the
/// response set (only `http://` fetches return cookies; the other
/// schemes return an empty `Vec`).  v2.2 closes the cookie loop:
/// `http://` `Set-Cookie` headers are parsed via the `cookie` crate
/// and returned for the caller to merge into the webview's jar.
fn html_from_url(url: &str, cookies: &[Cookie<'static>]) -> (String, Vec<Cookie<'static>>) {
    try_data_url(url)
        .map(|html| (html, Vec::new()))
        .or_else(|| try_file_url(url).map(|html| (html, Vec::new())))
        .or_else(|| try_http_url(url, cookies))
        .unwrap_or_else(|| {
            (
                format!(
                    "<html><body><h1>tauri-runtime-servocat</h1>\
                     <p>Unsupported URL scheme: <code>{url}</code></p>\
                     </body></html>"
                ),
                Vec::new(),
            )
        })
}

fn try_data_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    (parsed.scheme() == "data").then_some(())?;
    let (_mime, body) = parsed.path().split_once(',')?;
    let decoded = percent_encoding::percent_decode_str(body)
        .decode_utf8()
        .ok()?;
    Some(decoded.into_owned())
}

fn try_file_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    (parsed.scheme() == "file").then_some(())?;
    let path = parsed.to_file_path().ok()?;
    std::fs::read_to_string(path).ok()
}

fn try_http_url(url: &str, cookies: &[Cookie<'static>]) -> Option<(String, Vec<Cookie<'static>>)> {
    let parsed = url::Url::parse(url).ok()?;
    (parsed.scheme() == "http").then_some(())?;
    let host = parsed.host_str().unwrap_or("");
    let net_url = net_cat::url::Url::parse(url).ok()?;
    let request = net_cat::request::Request::new(net_cat::method::Method::Get, net_url);
    let request_with_cookies = cookie_header_value(cookies, host)
        .into_iter()
        .fold(request, |req, header| req.with_header("Cookie", header));
    let response = net_cat::fetch(&request_with_cookies).ok()?;
    let parsed_cookies: Vec<Cookie<'static>> = response
        .headers()
        .iter()
        .filter(|(name, _value)| name.eq_ignore_ascii_case("Set-Cookie"))
        .filter_map(|(_name, value)| Cookie::parse(value.clone()).ok())
        .map(Cookie::into_owned)
        .collect();
    Some((response.body_text(), parsed_cookies))
}

fn cookie_header_value(cookies: &[Cookie<'static>], host: &str) -> Option<String> {
    let serialized: Vec<String> = cookies
        .iter()
        .filter(|cookie| cookie.domain().is_none_or(|domain| domain == host))
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect();
    (!serialized.is_empty()).then(|| serialized.join("; "))
}

fn merge_cookies(jar: &mut Vec<Cookie<'static>>, new_cookies: Vec<Cookie<'static>>) {
    // Replace-by-name semantics: an inbound `Set-Cookie` for the same
    // name evicts the existing entry, mirroring how
    // `AddWebviewCookie` already behaves.  `for_each` over an
    // iterator (rather than a `for` loop) keeps the combinator
    // style; the lint that prefers `for` is overridden here.
    #[allow(clippy::needless_for_each)]
    new_cookies.into_iter().for_each(|cookie| {
        let name = cookie.name().to_owned();
        jar.retain(|existing| existing.name() != name.as_str());
        jar.push(cookie);
    });
}

fn print_to_png(
    frame: &Frame,
    width: u32,
    height: u32,
    label: &str,
    text_renderer: &mut TextRenderer,
) -> Option<std::path::PathBuf> {
    let pixels = render_to_pixels_with(frame, width, height, text_renderer);
    // FFI carve-out: tiny-skia's `Pixmap` requires `&mut` for
    // `data_mut`; we copy our rasterized RGBA into it so the
    // PNG-format encoder can serialize the same bytes that
    // softbuffer displays.
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    let data = pixmap.data_mut();
    (data.len() == pixels.rgba().len()).then_some(())?;
    data.copy_from_slice(pixels.rgba());
    let png = pixmap.encode_png().ok()?;
    let safe_label: String = label
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let stem = if safe_label.is_empty() {
        "webview".to_owned()
    } else {
        safe_label
    };
    let path = std::env::temp_dir().join(format!("tauri-runtime-servocat-print-{stem}.png"));
    std::fs::write(&path, png).ok()?;
    Some(path)
}

fn convert_size(size: Size) -> winit::dpi::Size {
    match size {
        Size::Logical(s) => winit::dpi::Size::Logical(LogicalSize::new(s.width, s.height)),
        Size::Physical(s) => {
            winit::dpi::Size::Physical(winit::dpi::PhysicalSize::new(s.width, s.height))
        }
    }
}

fn convert_cursor_icon(icon: CursorIcon) -> winit::window::CursorIcon {
    match icon {
        CursorIcon::Default | CursorIcon::Arrow => winit::window::CursorIcon::Default,
        CursorIcon::Crosshair => winit::window::CursorIcon::Crosshair,
        CursorIcon::Hand => winit::window::CursorIcon::Pointer,
        CursorIcon::Move => winit::window::CursorIcon::Move,
        CursorIcon::Text => winit::window::CursorIcon::Text,
        CursorIcon::Wait => winit::window::CursorIcon::Wait,
        CursorIcon::Help => winit::window::CursorIcon::Help,
        CursorIcon::Progress => winit::window::CursorIcon::Progress,
        CursorIcon::NotAllowed => winit::window::CursorIcon::NotAllowed,
        CursorIcon::ContextMenu => winit::window::CursorIcon::ContextMenu,
        CursorIcon::Cell => winit::window::CursorIcon::Cell,
        CursorIcon::VerticalText => winit::window::CursorIcon::VerticalText,
        CursorIcon::Alias => winit::window::CursorIcon::Alias,
        CursorIcon::Copy => winit::window::CursorIcon::Copy,
        CursorIcon::NoDrop => winit::window::CursorIcon::NoDrop,
        CursorIcon::Grab => winit::window::CursorIcon::Grab,
        CursorIcon::Grabbing => winit::window::CursorIcon::Grabbing,
        CursorIcon::AllScroll => winit::window::CursorIcon::AllScroll,
        CursorIcon::ZoomIn => winit::window::CursorIcon::ZoomIn,
        CursorIcon::ZoomOut => winit::window::CursorIcon::ZoomOut,
        CursorIcon::EResize => winit::window::CursorIcon::EResize,
        CursorIcon::NResize => winit::window::CursorIcon::NResize,
        CursorIcon::NeResize => winit::window::CursorIcon::NeResize,
        CursorIcon::NwResize => winit::window::CursorIcon::NwResize,
        CursorIcon::SResize => winit::window::CursorIcon::SResize,
        CursorIcon::SeResize => winit::window::CursorIcon::SeResize,
        CursorIcon::SwResize => winit::window::CursorIcon::SwResize,
        CursorIcon::WResize => winit::window::CursorIcon::WResize,
        CursorIcon::EwResize => winit::window::CursorIcon::EwResize,
        CursorIcon::NsResize => winit::window::CursorIcon::NsResize,
        CursorIcon::NeswResize => winit::window::CursorIcon::NeswResize,
        CursorIcon::NwseResize => winit::window::CursorIcon::NwseResize,
        CursorIcon::ColResize => winit::window::CursorIcon::ColResize,
        CursorIcon::RowResize => winit::window::CursorIcon::RowResize,
        _other => winit::window::CursorIcon::Default,
    }
}

fn winit_monitor_to_tauri(handle: &winit::monitor::MonitorHandle) -> Monitor {
    let size = handle.size();
    let position = handle.position();
    let physical_size = PhysicalSize::new(size.width, size.height);
    let physical_position = PhysicalPosition::new(position.x, position.y);
    Monitor {
        name: handle.name(),
        size: physical_size,
        position: physical_position,
        work_area: PhysicalRect {
            position: physical_position,
            size: physical_size,
        },
        scale_factor: handle.scale_factor(),
    }
}

fn skeleton_error_from(msg: String) -> Box<dyn std::error::Error + Send + Sync> {
    Box::from(msg)
}

struct AppHandler<T: UserEvent, F: FnMut(RunEvent<T>) + 'static> {
    callback: F,
    proxy: EventLoopProxy<RuntimeEvent<T>>,
    windows: BTreeMap<u32, Window>,
    labels: BTreeMap<u32, String>,
    winit_to_tauri: BTreeMap<WinitWindowId, u32>,
    /// Webview state keyed by `WebviewId.raw()` so the dispatcher's
    /// id survives a `reparent(new_window_id)` call.
    webviews: BTreeMap<u32, WebviewState<T>>,
    /// Maps `WebviewId.raw()` -> current parent `WindowId.raw()`.
    webview_to_window: BTreeMap<u32, u32>,
    /// Maps parent `WindowId.raw()` -> attached `WebviewId.raw()`
    /// (single webview per window in our model).
    window_to_webview: BTreeMap<u32, u32>,
    flags: BTreeMap<u32, WindowFlags>,
    ready_dispatched: bool,
}

impl<T: UserEvent, F: FnMut(RunEvent<T>) + 'static> AppHandler<T, F> {
    fn new(callback: F, proxy: EventLoopProxy<RuntimeEvent<T>>) -> Self {
        Self {
            callback,
            proxy,
            windows: BTreeMap::new(),
            labels: BTreeMap::new(),
            winit_to_tauri: BTreeMap::new(),
            webviews: BTreeMap::new(),
            webview_to_window: BTreeMap::new(),
            window_to_webview: BTreeMap::new(),
            flags: BTreeMap::new(),
            ready_dispatched: false,
        }
    }

    fn dispatch_window_event(&mut self, raw_id: u32, event: TauriWindowEvent) {
        let label = self.labels.get(&raw_id).cloned().unwrap_or_default();
        (self.callback)(RunEvent::WindowEvent { label, event });
    }

    fn request_redraw(&self, raw_id: u32) {
        let _ = self.windows.get(&raw_id).map(Window::request_redraw);
    }

    fn parent_window_of(&self, webview_raw: u32) -> Option<u32> {
        self.webview_to_window.get(&webview_raw).copied()
    }

    fn request_redraw_for_webview(&self, webview_raw: u32) {
        let _ = self
            .parent_window_of(webview_raw)
            .map(|window_raw| self.request_redraw(window_raw));
    }

    fn present(&mut self, window_raw: u32) {
        let webview_raw_opt = self.window_to_webview.get(&window_raw).copied();
        let window_opt = self.windows.get(&window_raw);
        let webview_opt = webview_raw_opt.and_then(|wv| self.webviews.get_mut(&wv));
        let _ = window_opt
            .zip(webview_opt)
            .and_then(|(window, webview)| present_webview(window, webview));
    }
}

fn present_webview<T: UserEvent>(window: &Window, webview: &mut WebviewState<T>) -> Option<()> {
    let size = window.inner_size();
    let width = NonZeroU32::new(size.width)?;
    let height = NonZeroU32::new(size.height)?;
    let frame = webview.current_frame.as_ref()?;
    let pixels = render_to_pixels_with(frame, size.width, size.height, &mut webview.text_renderer);
    // v2.0 visual application of `WebviewDispatch::set_background_color`:
    // composite the rasterized page over the tracked background colour
    // instead of always over white.
    let background = webview
        .background_color
        .map_or((255_u8, 255_u8, 255_u8), |color| {
            (color.0, color.1, color.2)
        });
    // v2.1 visual application of `WebviewDispatch::set_zoom`: sample
    // the source frame at `1/zoom` per destination pixel.  `zoom == 1`
    // collapses to direct mapping; non-finite or non-positive values
    // are clamped to 1.0 to keep the inverse well-defined.
    let zoom = if webview.zoom.is_finite() && webview.zoom > 0.0 {
        webview.zoom
    } else {
        1.0
    };
    let context = Context::new(window).ok()?;
    // FFI carve-out: softbuffer's `Surface` / `Buffer` require `&mut`
    // for `resize`, `buffer_mut`, and `present`.
    let mut surface = Surface::new(&context, window).ok()?;
    surface.resize(width, height).ok()?;
    let mut buffer = surface.buffer_mut().ok()?;
    write_pixels(
        &pixels,
        background,
        zoom,
        size.width,
        size.height,
        &mut buffer,
    );
    buffer.present().ok()?;
    Some(())
}

fn write_pixels(
    pixels: &PixelBuffer,
    background: (u8, u8, u8),
    zoom: f64,
    dst_w: u32,
    dst_h: u32,
    buffer: &mut softbuffer::Buffer<'_, &Window, &Window>,
) {
    let src_w = pixels.width().max(1);
    let src_h = pixels.height().max(1);
    let inv_zoom = 1.0 / zoom;
    let rgba = pixels.rgba();
    let bg_word = bg_to_word(background);
    (0..dst_h)
        .flat_map(|y| (0..dst_w).map(move |x| (x, y)))
        .map(|(x, y)| {
            sample_source_word(rgba, src_w, src_h, x, y, inv_zoom, background).unwrap_or(bg_word)
        })
        .zip(buffer.iter_mut())
        .for_each(|(word, slot)| *slot = word);
}

fn bg_to_word(background: (u8, u8, u8)) -> u32 {
    let (r, g, b) = background;
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

fn sample_source_word(
    rgba: &[u8],
    src_w: u32,
    src_h: u32,
    dst_x: u32,
    dst_y: u32,
    inv_zoom: f64,
    background: (u8, u8, u8),
) -> Option<u32> {
    let src_x_f = f64::from(dst_x) * inv_zoom;
    let src_y_f = f64::from(dst_y) * inv_zoom;
    (src_x_f.is_finite() && src_y_f.is_finite()).then_some(())?;
    (src_x_f >= 0.0 && src_y_f >= 0.0).then_some(())?;
    (src_x_f < f64::from(src_w) && src_y_f < f64::from(src_h)).then_some(())?;
    // v2.3 bilinear interpolation: weight the four surrounding source
    // pixels by their distance to the floating-point sample point.
    // For `zoom == 1.0`, the fractional parts are zero and the
    // formula collapses to a direct lookup of the floor pixel.
    let x0_f = src_x_f.floor();
    let y0_f = src_y_f.floor();
    // FFI carve-out (lossy cast): `u32::try_from(f64)` doesn't exist;
    // the finiteness + bounds checks above keep both values in the
    // valid `[0, src_w-1] x [0, src_h-1]` range, so `as u32` is safe.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let x0 = (x0_f as u32).min(src_w - 1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let y0 = (y0_f as u32).min(src_h - 1);
    let x1 = x0.saturating_add(1).min(src_w - 1);
    let y1 = y0.saturating_add(1).min(src_h - 1);
    let x_frac = src_x_f - x0_f;
    let y_frac = src_y_f - y0_f;
    let p00 = sample_pixel(rgba, src_w, x0, y0)?;
    let p10 = sample_pixel(rgba, src_w, x1, y0)?;
    let p01 = sample_pixel(rgba, src_w, x0, y1)?;
    let p11 = sample_pixel(rgba, src_w, x1, y1)?;
    let w00 = (1.0 - x_frac) * (1.0 - y_frac);
    let w10 = x_frac * (1.0 - y_frac);
    let w01 = (1.0 - x_frac) * y_frac;
    let w11 = x_frac * y_frac;
    let weights = (w00, w10, w01, w11);
    let r = blend_channel((p00.0, p10.0, p01.0, p11.0), weights);
    let g = blend_channel((p00.1, p10.1, p01.1, p11.1), weights);
    let b = blend_channel((p00.2, p10.2, p01.2, p11.2), weights);
    let a = blend_channel((p00.3, p10.3, p01.3, p11.3), weights);
    Some(rgba_to_softbuffer_word(&[r, g, b, a], background))
}

fn sample_pixel(rgba: &[u8], src_w: u32, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
    let x_usize = usize::try_from(x).ok()?;
    let y_usize = usize::try_from(y).ok()?;
    let src_w_usize = usize::try_from(src_w).ok()?;
    let idx = y_usize
        .checked_mul(src_w_usize)?
        .checked_add(x_usize)?
        .checked_mul(4)?;
    let chunk = rgba.get(idx..idx.checked_add(4)?)?;
    let red = chunk.first().copied()?;
    let green = chunk.get(1).copied()?;
    let blue = chunk.get(2).copied()?;
    let alpha = chunk.get(3).copied()?;
    Some((red, green, blue, alpha))
}

fn blend_channel(channels: (u8, u8, u8, u8), weights: (f64, f64, f64, f64)) -> u8 {
    let (p00, p10, p01, p11) = channels;
    let (w00, w10, w01, w11) = weights;
    let mixed =
        f64::from(p00) * w00 + f64::from(p10) * w10 + f64::from(p01) * w01 + f64::from(p11) * w11;
    // FFI carve-out (lossy cast): the bilinear sum is bounded by the
    // input channel range `[0, 255]` because the weights sum to 1.
    // We clamp + cast to `u8`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = mixed.clamp(0.0, 255.0).round() as u8;
    byte
}

fn rgba_to_softbuffer_word(chunk: &[u8], background: (u8, u8, u8)) -> u32 {
    let red = chunk.first().copied().unwrap_or(0);
    let green = chunk.get(1).copied().unwrap_or(0);
    let blue = chunk.get(2).copied().unwrap_or(0);
    let alpha = chunk.get(3).copied().unwrap_or(0);
    let (bg_r, bg_g, bg_b) = background;
    // Composite premultiplied RGBA over the webview's tracked
    // background colour:
    //   out = src + ((1 - src_a / 255) * bg)
    // Both inputs in [0, 255]; the inverse-alpha multiplication is
    // done in u32 to avoid premature truncation, then we scale back.
    let inv = u32::from(255_u8 - alpha);
    let out_r = (u32::from(red) + (u32::from(bg_r) * inv) / 255).min(255);
    let out_g = (u32::from(green) + (u32::from(bg_g) * inv) / 255).min(255);
    let out_b = (u32::from(blue) + (u32::from(bg_b) * inv) / 255).min(255);
    (out_r << 16) | (out_g << 8) | out_b
}

impl<T: UserEvent, F: FnMut(RunEvent<T>) + 'static> ApplicationHandler<RuntimeEvent<T>>
    for AppHandler<T, F>
{
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if !self.ready_dispatched {
            (self.callback)(RunEvent::Ready);
            self.ready_dispatched = true;
        }
        (self.callback)(RunEvent::Resumed);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RuntimeEvent<T>) {
        match event {
            RuntimeEvent::User(payload) => (self.callback)(RunEvent::UserEvent(payload)),
            RuntimeEvent::CreateWindow {
                id,
                label,
                attributes,
            } => {
                let raw = raw_window_id(id);
                let attrs = attributes.to_winit_attributes();
                let _ = event_loop.create_window(attrs).map(|window| {
                    let winit_id = window.id();
                    let _ = self.winit_to_tauri.insert(winit_id, raw);
                    let _ = self.windows.insert(raw, window);
                    let _ = self.labels.insert(raw, label);
                    let _ = self.flags.insert(raw, WindowFlags::default());
                });
            }
            RuntimeEvent::SetTitle { id, title } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_title(&title));
            }
            RuntimeEvent::SetSize { id, size } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(|window| match size {
                    Size::Logical(s) => {
                        let _ = window.request_inner_size(LogicalSize::new(s.width, s.height));
                    }
                    Size::Physical(s) => {
                        let _ = window
                            .request_inner_size(winit::dpi::PhysicalSize::new(s.width, s.height));
                    }
                });
            }
            RuntimeEvent::SetPosition { id, position } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(|window| match position {
                    Position::Logical(p) => {
                        window.set_outer_position(LogicalPosition::new(p.x, p.y));
                    }
                    Position::Physical(p) => {
                        window.set_outer_position(WinitPhysicalPosition::new(p.x, p.y));
                    }
                });
            }
            RuntimeEvent::Show { id } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_visible(true));
            }
            RuntimeEvent::Hide { id } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_visible(false));
            }
            RuntimeEvent::Focus { id } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(Window::focus_window);
            }
            RuntimeEvent::Close { id } | RuntimeEvent::Destroy { id } => {
                let raw = raw_window_id(id);
                let _ = self.windows.remove(&raw).map(|window| {
                    let winit_id = window.id();
                    let _ = self.winit_to_tauri.remove(&winit_id);
                    drop(window);
                });
                self.dispatch_window_event(raw, TauriWindowEvent::Destroyed);
                let _ = self.labels.remove(&raw);
                let _ = self.flags.remove(&raw);
                // If a webview was attached to this window, evict it.
                let _ = self.window_to_webview.remove(&raw).map(|webview_raw| {
                    let _ = self.webviews.remove(&webview_raw);
                    let _ = self.webview_to_window.remove(&webview_raw);
                });
            }
            RuntimeEvent::Maximize { id } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_maximized(true));
            }
            RuntimeEvent::Unmaximize { id } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_maximized(false));
            }
            RuntimeEvent::Minimize { id } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_minimized(true));
            }
            RuntimeEvent::Unminimize { id } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_minimized(false));
            }
            RuntimeEvent::SetResizable { id, resizable } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_resizable(resizable));
            }
            RuntimeEvent::SetDecorations { id, decorations } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_decorations(decorations));
            }
            RuntimeEvent::SetFullscreen { id, fullscreen } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(|window| {
                    let mode = fullscreen.then(|| Fullscreen::Borderless(None));
                    window.set_fullscreen(mode);
                });
            }
            RuntimeEvent::CreateWebview {
                window_id,
                webview_id,
                label,
                url,
                ipc_handler,
            } => {
                let window_raw = raw_window_id(window_id);
                let webview_raw = webview_id.raw();
                let viewport = self.windows.get(&window_raw).map_or_else(
                    || Viewport::new(800, 600),
                    |window| {
                        let size = window.inner_size();
                        Viewport::new(size.width, size.height)
                    },
                );
                // CreateWebview's webview state has no cookies yet,
                // so we can't attach any.  Future versions could
                // accept seeded cookies via `PendingWebview`.
                let (html, response_cookies) = html_from_url(&url, &[]);
                let mut state = WebviewState::new(label, html, viewport, ipc_handler);
                merge_cookies(&mut state.cookies, response_cookies);
                state.rebuild_frame();
                let _ = self.webviews.insert(webview_raw, state);
                let _ = self.webview_to_window.insert(webview_raw, window_raw);
                let _ = self.window_to_webview.insert(window_raw, webview_raw);
                self.request_redraw(window_raw);
            }
            RuntimeEvent::ReparentWebview {
                webview_id,
                new_window_id,
            } => {
                let webview_raw = webview_id.raw();
                let new_window_raw = raw_window_id(new_window_id);
                let old_window_raw = self.webview_to_window.get(&webview_raw).copied();
                let _ = old_window_raw.map(|old| {
                    if self.window_to_webview.get(&old) == Some(&webview_raw) {
                        let _ = self.window_to_webview.remove(&old);
                    }
                    self.request_redraw(old);
                });
                let _ = self.webview_to_window.insert(webview_raw, new_window_raw);
                let _ = self.window_to_webview.insert(new_window_raw, webview_raw);
                self.request_redraw(new_window_raw);
            }
            RuntimeEvent::CloseWebviewParentWindow { webview_id } => {
                let _ = self.parent_window_of(webview_id.raw()).map(|window_raw| {
                    let _ = self.windows.remove(&window_raw).map(|window| {
                        let winit_id = window.id();
                        let _ = self.winit_to_tauri.remove(&winit_id);
                        drop(window);
                    });
                    self.dispatch_window_event(window_raw, TauriWindowEvent::Destroyed);
                    let _ = self.labels.remove(&window_raw);
                    let _ = self.flags.remove(&window_raw);
                    let _ = self.window_to_webview.remove(&window_raw);
                });
            }
            RuntimeEvent::FocusWebviewParentWindow { webview_id } => {
                let _ = self
                    .parent_window_of(webview_id.raw())
                    .and_then(|window_raw| self.windows.get(&window_raw))
                    .map(Window::focus_window);
            }
            RuntimeEvent::IpcRequest {
                webview_id,
                request,
            } => {
                let webview_raw = webview_id.raw();
                let dispatch_pair = self.webviews.get(&webview_raw).and_then(|webview| {
                    webview
                        .ipc_handler
                        .as_ref()
                        .map(|handler| (handler, webview.label.clone()))
                });
                let _ = dispatch_pair.map(|(handler, label)| {
                    let detached = DetachedWebview {
                        label,
                        dispatcher: ServocatWebviewDispatch {
                            proxy: self.proxy.clone(),
                            webview_id,
                            _marker: PhantomData,
                        },
                    };
                    handler(detached, request);
                });
            }
            RuntimeEvent::Navigate { webview_id, url } => {
                let webview_raw = webview_id.raw();
                let cookies = self
                    .webviews
                    .get(&webview_raw)
                    .map(|webview| webview.cookies.clone())
                    .unwrap_or_default();
                let (html, response_cookies) = html_from_url(&url, &cookies);
                let _ = self.webviews.get_mut(&webview_raw).map(|webview| {
                    webview.html = html;
                    merge_cookies(&mut webview.cookies, response_cookies);
                    webview.rebuild_frame();
                });
                self.request_redraw_for_webview(webview_raw);
            }
            RuntimeEvent::Reload { webview_id } => {
                let webview_raw = webview_id.raw();
                let _ = self
                    .webviews
                    .get_mut(&webview_raw)
                    .map(WebviewState::rebuild_frame);
                self.request_redraw_for_webview(webview_raw);
            }
            RuntimeEvent::EvalScript { webview_id, script } => {
                let webview_raw = webview_id.raw();
                let proxy = self.proxy.clone();
                with_ipc_dispatch(proxy, webview_id, || {
                    let _ = self
                        .webviews
                        .get_mut(&webview_raw)
                        .map(|webview| webview.eval(&script));
                });
                self.request_redraw_for_webview(webview_raw);
            }
            RuntimeEvent::EvalScriptWithCallback {
                webview_id,
                script,
                callback,
            } => {
                let webview_raw = webview_id.raw();
                let proxy = self.proxy.clone();
                let result = with_ipc_dispatch(proxy, webview_id, || {
                    self.webviews
                        .get_mut(&webview_raw)
                        .and_then(|webview| webview.eval(&script))
                        .unwrap_or_default()
                });
                callback(result);
                self.request_redraw_for_webview(webview_raw);
            }
            RuntimeEvent::QueryInnerSize { id, reply } => {
                let raw = raw_window_id(id);
                let size = self.windows.get(&raw).map_or_else(
                    || PhysicalSize::new(0, 0),
                    |window| {
                        let s = window.inner_size();
                        PhysicalSize::new(s.width, s.height)
                    },
                );
                let _ = reply.send(size);
            }
            RuntimeEvent::QueryOuterSize { id, reply } => {
                let raw = raw_window_id(id);
                let size = self.windows.get(&raw).map_or_else(
                    || PhysicalSize::new(0, 0),
                    |window| {
                        let s = window.outer_size();
                        PhysicalSize::new(s.width, s.height)
                    },
                );
                let _ = reply.send(size);
            }
            RuntimeEvent::QueryInnerPosition { id, reply } => {
                let raw = raw_window_id(id);
                let position = self.windows.get(&raw).map_or_else(
                    || PhysicalPosition::new(0, 0),
                    |window| {
                        window.inner_position().map_or_else(
                            |_e| PhysicalPosition::new(0, 0),
                            |p| PhysicalPosition::new(p.x, p.y),
                        )
                    },
                );
                let _ = reply.send(position);
            }
            RuntimeEvent::QueryOuterPosition { id, reply } => {
                let raw = raw_window_id(id);
                let position = self.windows.get(&raw).map_or_else(
                    || PhysicalPosition::new(0, 0),
                    |window| {
                        window.outer_position().map_or_else(
                            |_e| PhysicalPosition::new(0, 0),
                            |p| PhysicalPosition::new(p.x, p.y),
                        )
                    },
                );
                let _ = reply.send(position);
            }
            RuntimeEvent::QueryScaleFactor { id, reply } => {
                let raw = raw_window_id(id);
                let factor = self
                    .windows
                    .get(&raw)
                    .map_or(1.0, winit::window::Window::scale_factor);
                let _ = reply.send(factor);
            }
            RuntimeEvent::QueryTitle { id, reply } => {
                let raw = raw_window_id(id);
                let title = self
                    .windows
                    .get(&raw)
                    .map_or_else(String::new, winit::window::Window::title);
                let _ = reply.send(title);
            }
            RuntimeEvent::QueryIsVisible { id, reply } => {
                let raw = raw_window_id(id);
                let visible = self
                    .windows
                    .get(&raw)
                    .and_then(winit::window::Window::is_visible)
                    .unwrap_or(false);
                let _ = reply.send(visible);
            }
            RuntimeEvent::QueryIsFocused { id, reply } => {
                let raw = raw_window_id(id);
                let focused = self
                    .windows
                    .get(&raw)
                    .is_some_and(winit::window::Window::has_focus);
                let _ = reply.send(focused);
            }
            RuntimeEvent::QueryIsMaximized { id, reply } => {
                let raw = raw_window_id(id);
                let maximized = self
                    .windows
                    .get(&raw)
                    .is_some_and(winit::window::Window::is_maximized);
                let _ = reply.send(maximized);
            }
            RuntimeEvent::QueryIsMinimized { id, reply } => {
                let raw = raw_window_id(id);
                let minimized = self
                    .windows
                    .get(&raw)
                    .and_then(winit::window::Window::is_minimized)
                    .unwrap_or(false);
                let _ = reply.send(minimized);
            }
            RuntimeEvent::QueryIsFullscreen { id, reply } => {
                let raw = raw_window_id(id);
                let fullscreen = self
                    .windows
                    .get(&raw)
                    .is_some_and(|w| w.fullscreen().is_some());
                let _ = reply.send(fullscreen);
            }
            RuntimeEvent::QueryIsDecorated { id, reply } => {
                let raw = raw_window_id(id);
                let decorated = self
                    .windows
                    .get(&raw)
                    .is_some_and(winit::window::Window::is_decorated);
                let _ = reply.send(decorated);
            }
            RuntimeEvent::QueryIsResizable { id, reply } => {
                let raw = raw_window_id(id);
                let resizable = self
                    .windows
                    .get(&raw)
                    .is_some_and(winit::window::Window::is_resizable);
                let _ = reply.send(resizable);
            }
            RuntimeEvent::QueryPrimaryMonitor { reply } => {
                let monitor = event_loop
                    .primary_monitor()
                    .as_ref()
                    .map(winit_monitor_to_tauri);
                let _ = reply.send(monitor);
            }
            RuntimeEvent::QueryAvailableMonitors { reply } => {
                let monitors: Vec<Monitor> = event_loop
                    .available_monitors()
                    .map(|handle| winit_monitor_to_tauri(&handle))
                    .collect();
                let _ = reply.send(monitors);
            }
            RuntimeEvent::QueryMonitorFromPoint { x, y, reply } => {
                let position = winit::dpi::PhysicalPosition::new(x, y);
                let monitor = event_loop
                    .available_monitors()
                    .find(|handle| {
                        let pos = handle.position();
                        let size = handle.size();
                        let in_x = position.x >= f64::from(pos.x)
                            && position.x < f64::from(pos.x) + f64::from(size.width);
                        let in_y = position.y >= f64::from(pos.y)
                            && position.y < f64::from(pos.y) + f64::from(size.height);
                        in_x && in_y
                    })
                    .as_ref()
                    .map(winit_monitor_to_tauri);
                let _ = reply.send(monitor);
            }
            RuntimeEvent::QueryCurrentMonitor { id, reply } => {
                let raw = raw_window_id(id);
                let monitor = self
                    .windows
                    .get(&raw)
                    .and_then(winit::window::Window::current_monitor)
                    .as_ref()
                    .map(winit_monitor_to_tauri);
                let _ = reply.send(monitor);
            }
            RuntimeEvent::RunOnMainThread { thunk } => {
                thunk();
            }
            RuntimeEvent::QueryTheme { id, reply } => {
                let raw = raw_window_id(id);
                let theme = self
                    .windows
                    .get(&raw)
                    .and_then(winit::window::Window::theme)
                    .map_or(Theme::Light, |t| match t {
                        winit::window::Theme::Light => Theme::Light,
                        winit::window::Theme::Dark => Theme::Dark,
                    });
                let _ = reply.send(theme);
            }
            RuntimeEvent::SetCursorVisible { id, visible } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_cursor_visible(visible));
            }
            RuntimeEvent::SetCursorGrab { id, grab } => {
                let raw = raw_window_id(id);
                let mode = if grab {
                    winit::window::CursorGrabMode::Confined
                } else {
                    winit::window::CursorGrabMode::None
                };
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_cursor_grab(mode));
            }
            RuntimeEvent::SetCursorIcon { id, icon } => {
                let raw = raw_window_id(id);
                let winit_icon = convert_cursor_icon(icon);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_cursor(winit_icon));
            }
            RuntimeEvent::SetCursorPosition { id, position } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(|window| match position {
                    Position::Logical(p) => {
                        let _ = window.set_cursor_position(LogicalPosition::new(p.x, p.y));
                    }
                    Position::Physical(p) => {
                        let _ =
                            window.set_cursor_position(winit::dpi::PhysicalPosition::new(p.x, p.y));
                    }
                });
            }
            RuntimeEvent::SetIgnoreCursorEvents { id, ignore } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_cursor_hittest(!ignore));
            }
            RuntimeEvent::SetMinSize { id, size } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_min_inner_size(size.map(convert_size)));
            }
            RuntimeEvent::SetMaxSize { id, size } => {
                let raw = raw_window_id(id);
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_max_inner_size(size.map(convert_size)));
            }
            RuntimeEvent::SetAlwaysOnTop { id, always_on_top } => {
                let raw = raw_window_id(id);
                let level = if always_on_top {
                    winit::window::WindowLevel::AlwaysOnTop
                } else {
                    winit::window::WindowLevel::Normal
                };
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_window_level(level));
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.always_on_top = always_on_top;
                });
            }
            RuntimeEvent::SetWindowTheme { id, theme } => {
                let raw = raw_window_id(id);
                let winit_theme = theme.map(|t| match t {
                    Theme::Light => winit::window::Theme::Light,
                    Theme::Dark => winit::window::Theme::Dark,
                    _other => winit::window::Theme::Light,
                });
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.set_theme(winit_theme));
            }
            RuntimeEvent::StartDragging { id } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(Window::drag_window);
            }
            RuntimeEvent::RequestUserAttention { id, request_type } => {
                let raw = raw_window_id(id);
                let winit_attention = request_type.map(|t| match t {
                    tauri_runtime::UserAttentionType::Critical => {
                        winit::window::UserAttentionType::Critical
                    }
                    tauri_runtime::UserAttentionType::Informational => {
                        winit::window::UserAttentionType::Informational
                    }
                });
                let _ = self
                    .windows
                    .get(&raw)
                    .map(|window| window.request_user_attention(winit_attention));
            }
            RuntimeEvent::CenterWindow { id } => {
                let raw = raw_window_id(id);
                let _ = self.windows.get(&raw).map(|window| {
                    let _ = window.current_monitor().map(|monitor| {
                        let monitor_size = monitor.size();
                        let monitor_pos = monitor.position();
                        let window_size = window.outer_size();
                        let x = monitor_pos.x
                            + i32::try_from(monitor_size.width.saturating_sub(window_size.width))
                                .unwrap_or(0)
                                / 2;
                        let y = monitor_pos.y
                            + i32::try_from(monitor_size.height.saturating_sub(window_size.height))
                                .unwrap_or(0)
                                / 2;
                        window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
                    });
                });
            }
            RuntimeEvent::SetWindowIcon {
                id,
                rgba,
                width,
                height,
            } => {
                let raw = raw_window_id(id);
                let _ = winit::window::Icon::from_rgba(rgba, width, height)
                    .ok()
                    .map(|winit_icon| {
                        let _ = self
                            .windows
                            .get(&raw)
                            .map(|window| window.set_window_icon(Some(winit_icon)));
                    });
            }
            RuntimeEvent::SetEnabled { id, enabled } => {
                let raw = raw_window_id(id);
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.enabled = enabled;
                });
            }
            RuntimeEvent::SetMaximizable { id, maximizable } => {
                let raw = raw_window_id(id);
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.maximizable = maximizable;
                });
            }
            RuntimeEvent::SetMinimizable { id, minimizable } => {
                let raw = raw_window_id(id);
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.minimizable = minimizable;
                });
            }
            RuntimeEvent::SetClosable { id, closable } => {
                let raw = raw_window_id(id);
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.closable = closable;
                });
            }
            RuntimeEvent::SetFocusable { id, focusable } => {
                let raw = raw_window_id(id);
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.focusable = focusable;
                });
            }
            RuntimeEvent::SetSkipTaskbar { id, skip } => {
                let raw = raw_window_id(id);
                let _ = self.flags.get_mut(&raw).map(|flags| {
                    flags.skip_taskbar = skip;
                });
            }
            RuntimeEvent::QueryIsEnabled { id, reply } => {
                let raw = raw_window_id(id);
                let value = self.flags.get(&raw).is_some_and(|flags| flags.enabled);
                let _ = reply.send(value);
            }
            RuntimeEvent::QueryIsMaximizable { id, reply } => {
                let raw = raw_window_id(id);
                let value = self.flags.get(&raw).is_some_and(|flags| flags.maximizable);
                let _ = reply.send(value);
            }
            RuntimeEvent::QueryIsMinimizable { id, reply } => {
                let raw = raw_window_id(id);
                let value = self.flags.get(&raw).is_some_and(|flags| flags.minimizable);
                let _ = reply.send(value);
            }
            RuntimeEvent::QueryIsClosable { id, reply } => {
                let raw = raw_window_id(id);
                let value = self.flags.get(&raw).is_some_and(|flags| flags.closable);
                let _ = reply.send(value);
            }
            RuntimeEvent::QueryIsAlwaysOnTop { id, reply } => {
                let raw = raw_window_id(id);
                let value = self
                    .flags
                    .get(&raw)
                    .is_some_and(|flags| flags.always_on_top);
                let _ = reply.send(value);
            }
            RuntimeEvent::SetWebviewBounds { webview_id, bounds } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.bounds = bounds;
                });
            }
            RuntimeEvent::SetWebviewSize { webview_id, size } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.bounds = Rect {
                        position: webview.bounds.position,
                        size,
                    };
                });
            }
            RuntimeEvent::SetWebviewPosition {
                webview_id,
                position,
            } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.bounds = Rect {
                        position,
                        size: webview.bounds.size,
                    };
                });
            }
            RuntimeEvent::QueryWebviewBounds { webview_id, reply } => {
                let raw = webview_id.raw();
                let value = self.webviews.get(&raw).map_or_else(
                    || Rect {
                        position: Position::Physical(PhysicalPosition::new(0, 0)),
                        size: tauri_runtime::dpi::Size::Physical(PhysicalSize::new(0, 0)),
                    },
                    |webview| webview.bounds,
                );
                let _ = reply.send(value);
            }
            RuntimeEvent::QueryWebviewPosition { webview_id, reply } => {
                let raw = webview_id.raw();
                let value = self.webviews.get(&raw).map_or_else(
                    || PhysicalPosition::new(0, 0),
                    |webview| match webview.bounds.position {
                        Position::Physical(p) => PhysicalPosition::new(p.x, p.y),
                        Position::Logical(p) => {
                            #[allow(clippy::cast_possible_truncation)]
                            let x = p.x as i32;
                            #[allow(clippy::cast_possible_truncation)]
                            let y = p.y as i32;
                            PhysicalPosition::new(x, y)
                        }
                    },
                );
                let _ = reply.send(value);
            }
            RuntimeEvent::SetWebviewZoom { webview_id, scale } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.zoom = scale;
                });
                self.request_redraw_for_webview(raw);
            }
            RuntimeEvent::SetWebviewBackgroundColor { webview_id, color } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.background_color = color;
                });
            }
            RuntimeEvent::SetWebviewAutoResize {
                webview_id,
                auto_resize,
            } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.auto_resize = auto_resize;
                });
            }
            RuntimeEvent::AddWebviewCookie { webview_id, cookie } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    let name = cookie.name().to_owned();
                    webview
                        .cookies
                        .retain(|existing| existing.name() != name.as_str());
                    webview.cookies.push(cookie);
                });
            }
            RuntimeEvent::RemoveWebviewCookie { webview_id, cookie } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    let name = cookie.name().to_owned();
                    webview
                        .cookies
                        .retain(|existing| existing.name() != name.as_str());
                });
            }
            RuntimeEvent::ClearWebviewBrowsingData { webview_id } => {
                let raw = webview_id.raw();
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.cookies.clear();
                });
            }
            RuntimeEvent::QueryWebviewCookies { webview_id, reply } => {
                let raw = webview_id.raw();
                let value = self
                    .webviews
                    .get(&raw)
                    .map(|webview| webview.cookies.clone())
                    .unwrap_or_default();
                let _ = reply.send(value);
            }
            RuntimeEvent::PrintWebview { webview_id } => {
                let webview_raw = webview_id.raw();
                let window_raw_opt = self.parent_window_of(webview_raw);
                let size = window_raw_opt
                    .and_then(|window_raw| self.windows.get(&window_raw))
                    .map(winit::window::Window::inner_size);
                let _ = size.and_then(|sz| {
                    self.webviews.get_mut(&webview_raw).and_then(|webview| {
                        let WebviewState {
                            current_frame,
                            text_renderer,
                            label,
                            ..
                        } = webview;
                        let frame = current_frame.as_ref()?;
                        print_to_png(frame, sz.width, sz.height, label, text_renderer).map(|path| {
                            println!(
                                "[tauri-runtime-servocat] print: wrote frame to {}",
                                path.display()
                            );
                        })
                    })
                });
            }
            RuntimeEvent::QueryWebviewCookiesForUrl {
                webview_id,
                url,
                reply,
            } => {
                let raw = webview_id.raw();
                let host = url.host_str().unwrap_or("").to_owned();
                let value = self
                    .webviews
                    .get(&raw)
                    .map(|webview| {
                        webview
                            .cookies
                            .iter()
                            .filter(|cookie| {
                                cookie.domain().is_none_or(|domain| domain == host.as_str())
                            })
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let _ = reply.send(value);
            }
            RuntimeEvent::QueryWebviewSize { webview_id, reply } => {
                let raw = webview_id.raw();
                let value = self.webviews.get(&raw).map_or_else(
                    || PhysicalSize::new(0, 0),
                    |webview| match webview.bounds.size {
                        tauri_runtime::dpi::Size::Physical(s) => {
                            PhysicalSize::new(s.width, s.height)
                        }
                        tauri_runtime::dpi::Size::Logical(s) => {
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let w = s.width as u32;
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let h = s.height as u32;
                            PhysicalSize::new(w, h)
                        }
                    },
                );
                let _ = reply.send(value);
            }
            RuntimeEvent::Exit { code: _ } => {
                event_loop.exit();
                (self.callback)(RunEvent::Exit);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WinitWindowId,
        event: WinitWindowEvent,
    ) {
        let raw_opt = self.winit_to_tauri.get(&window_id).copied();
        match event {
            WinitWindowEvent::CloseRequested => {
                let _ = raw_opt.map(|raw| {
                    let (signal_tx, signal_rx) = mpsc::channel::<bool>();
                    self.dispatch_window_event(raw, TauriWindowEvent::CloseRequested { signal_tx });
                    // Tauri's contract: receive cancellation request; if true,
                    // keep the window open.  We poll once with try_recv and
                    // default to closing.
                    let prevent = signal_rx.try_recv().unwrap_or(false);
                    if !prevent {
                        let _ = self.windows.remove(&raw).map(|w| {
                            let winit_id = w.id();
                            let _ = self.winit_to_tauri.remove(&winit_id);
                        });
                        self.dispatch_window_event(raw, TauriWindowEvent::Destroyed);
                        let _ = self.labels.remove(&raw);
                        if self.windows.is_empty() {
                            event_loop.exit();
                        }
                    }
                });
            }
            WinitWindowEvent::Destroyed => {
                let _ = raw_opt.map(|raw| {
                    self.dispatch_window_event(raw, TauriWindowEvent::Destroyed);
                    let _ = self.windows.remove(&raw);
                    let _ = self.labels.remove(&raw);
                });
            }
            WinitWindowEvent::Resized(size) => {
                let _ = raw_opt.map(|raw| {
                    self.dispatch_window_event(
                        raw,
                        TauriWindowEvent::Resized(PhysicalSize::new(size.width, size.height)),
                    );
                    let webview_raw = self.window_to_webview.get(&raw).copied();
                    let _ = webview_raw
                        .and_then(|wv| self.webviews.get_mut(&wv))
                        .map(|webview| {
                            if webview.auto_resize {
                                webview.bounds = Rect {
                                    position: Position::Physical(PhysicalPosition::new(0, 0)),
                                    size: tauri_runtime::dpi::Size::Physical(PhysicalSize::new(
                                        size.width,
                                        size.height,
                                    )),
                                };
                            }
                        });
                });
            }
            WinitWindowEvent::Moved(position) => {
                let _ = raw_opt.map(|raw| {
                    self.dispatch_window_event(
                        raw,
                        TauriWindowEvent::Moved(PhysicalPosition::new(position.x, position.y)),
                    );
                });
            }
            WinitWindowEvent::Focused(focused) => {
                let _ = raw_opt.map(|raw| {
                    self.dispatch_window_event(raw, TauriWindowEvent::Focused(focused));
                });
            }
            WinitWindowEvent::RedrawRequested => {
                let _ = raw_opt.map(|raw| self.present(raw));
            }
            WinitWindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let _ = raw_opt.map(|raw| {
                    let new_inner = self.windows.get(&raw).map_or_else(
                        || PhysicalSize::new(0, 0),
                        |window| {
                            let size = window.inner_size();
                            PhysicalSize::new(size.width, size.height)
                        },
                    );
                    self.dispatch_window_event(
                        raw,
                        TauriWindowEvent::ScaleFactorChanged {
                            scale_factor,
                            new_inner_size: new_inner,
                        },
                    );
                });
            }
            _other => (),
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        (self.callback)(RunEvent::MainEventsCleared);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        (self.callback)(RunEvent::Exit);
    }
}

#[cfg(test)]
mod tests {
    use super::{Cookie, blend_channel, merge_cookies};

    fn fail(_msg: &'static str) -> tauri_runtime::Error {
        tauri_runtime::Error::FailedToReceiveMessage
    }

    #[test]
    fn merge_replaces_existing_by_name() -> Result<(), tauri_runtime::Error> {
        let mut jar = vec![Cookie::build(("session", "old")).build()];
        let incoming = vec![Cookie::build(("session", "new")).build()];
        merge_cookies(&mut jar, incoming);
        let first = jar.first().ok_or_else(|| fail("empty jar"))?;
        (jar.len() == 1 && first.value() == "new")
            .then_some(())
            .ok_or_else(|| fail("session was not replaced"))
    }

    #[test]
    fn merge_appends_unrelated_names() -> Result<(), tauri_runtime::Error> {
        let mut jar = vec![Cookie::build(("theme", "dark")).build()];
        let incoming = vec![Cookie::build(("lang", "en")).build()];
        merge_cookies(&mut jar, incoming);
        let names: Vec<String> = jar.iter().map(|c| c.name().to_owned()).collect();
        (names.iter().any(|n| n == "theme") && names.iter().any(|n| n == "lang"))
            .then_some(())
            .ok_or_else(|| fail("expected both theme and lang to be present"))
    }

    #[test]
    fn bilinear_corner_weight_selects_single_pixel() -> Result<(), tauri_runtime::Error> {
        // Weight (0, 0, 0, 1) means "take p11 entirely".
        let result = blend_channel((10, 20, 30, 40), (0.0, 0.0, 0.0, 1.0));
        (result == 40)
            .then_some(())
            .ok_or_else(|| fail("expected corner weight to select p11"))
    }

    #[test]
    fn bilinear_horizontal_midpoint_averages_two_pixels() -> Result<(), tauri_runtime::Error> {
        // Weight (0.5, 0.5, 0, 0) averages p00 and p10 along the
        // top edge; 100 and 200 -> 150.
        let result = blend_channel((100, 200, 0, 0), (0.5, 0.5, 0.0, 0.0));
        (result == 150)
            .then_some(())
            .ok_or_else(|| fail("expected horizontal midpoint to be 150"))
    }

    #[test]
    fn bilinear_centroid_averages_all_four() -> Result<(), tauri_runtime::Error> {
        // Equal 0.25 weights -> simple mean of the four channels.
        let result = blend_channel((40, 60, 80, 100), (0.25, 0.25, 0.25, 0.25));
        (result == 70)
            .then_some(())
            .ok_or_else(|| fail("expected centroid to be 70"))
    }
}
