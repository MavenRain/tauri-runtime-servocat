//! v1.3 implementation of the `tauri_runtime::Runtime` trait surface.
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

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use cookie::Cookie;
use raw_window_handle::{DisplayHandle, HandleError, WindowHandle};
use softbuffer::{Context, Surface};
use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size};
use tauri_runtime::monitor::Monitor;
use tauri_runtime::webview::{DetachedWebview, PendingWebview};
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

fn allocate_window_id() -> WindowId {
    WindowId::from(NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst))
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
    /// into the window via softbuffer.
    CreateWebview {
        window_id: WindowId,
        label: String,
        url: String,
    },
    /// Navigates the webview attached to a window to a new URL.  The
    /// runtime re-builds the pipeline against the new content and
    /// requests a redraw.
    Navigate { window_id: WindowId, url: String },
    /// Reloads the webview attached to a window (re-builds the
    /// pipeline against the cached HTML/CSS and requests a redraw).
    Reload { window_id: WindowId },
    /// Runs a JS expression against the webview attached to a window
    /// via `run_script_with_backprop`, back-propagates DOM mutations
    /// into layout, and requests a redraw.
    EvalScript { window_id: WindowId, script: String },
    /// Like [`Self::EvalScript`] but also delivers a string-formatted
    /// representation of the script's return value back through the
    /// supplied callback after the eval completes.
    EvalScriptWithCallback {
        window_id: WindowId,
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

/// `Send`-able dispatcher for [`ServocatRuntime`] webviews.  Still
/// stub-shaped in v1.1; carries the parent window id for routing.
pub struct ServocatWebviewDispatch<T: UserEvent> {
    proxy: EventLoopProxy<RuntimeEvent<T>>,
    window_id: WindowId,
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatWebviewDispatch<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServocatWebviewDispatch")
            .field("window_id", &self.window_id)
            .finish_non_exhaustive()
    }
}

impl<T: UserEvent> Clone for ServocatWebviewDispatch<T> {
    fn clone(&self) -> Self {
        Self {
            proxy: self.proxy.clone(),
            window_id: self.window_id,
            _marker: PhantomData,
        }
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
        let _ = self.event_loop.take().map(|event_loop| {
            let mut app = AppHandler::new(callback);
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

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, _f: F) -> Result<()> {
        Err(Error::EventLoopClosed)
    }

    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }

    fn primary_monitor(&self) -> Option<Monitor> {
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

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, _f: F) -> Result<()> {
        Err(Error::EventLoopClosed)
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
        Ok(false)
    }

    fn is_minimizable(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_closable(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_visible(&self) -> Result<bool> {
        query(&self.proxy, |reply| RuntimeEvent::QueryIsVisible {
            id: self.window_id,
            reply,
        })
    }

    fn is_enabled(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_always_on_top(&self) -> Result<bool> {
        Ok(false)
    }

    fn title(&self) -> Result<String> {
        query(&self.proxy, |reply| RuntimeEvent::QueryTitle {
            id: self.window_id,
            reply,
        })
    }

    fn current_monitor(&self) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn primary_monitor(&self) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn monitor_from_point(&self, _x: f64, _y: f64) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn available_monitors(&self) -> Result<Vec<Monitor>> {
        Ok(Vec::new())
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
        Ok(())
    }

    fn request_user_attention(
        &self,
        _request_type: Option<tauri_runtime::UserAttentionType>,
    ) -> Result<()> {
        Ok(())
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

    fn set_enabled(&self, _enabled: bool) -> Result<()> {
        Ok(())
    }

    fn set_maximizable(&self, _maximizable: bool) -> Result<()> {
        Ok(())
    }

    fn set_minimizable(&self, _minimizable: bool) -> Result<()> {
        Ok(())
    }

    fn set_closable(&self, _closable: bool) -> Result<()> {
        Ok(())
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

    fn set_always_on_top(&self, _always_on_top: bool) -> Result<()> {
        Ok(())
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

    fn set_min_size(&self, _size: Option<Size>) -> Result<()> {
        Ok(())
    }

    fn set_max_size(&self, _size: Option<Size>) -> Result<()> {
        Ok(())
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

    fn set_focusable(&self, _focusable: bool) -> Result<()> {
        Ok(())
    }

    fn set_icon(&self, _icon: Icon<'_>) -> Result<()> {
        Ok(())
    }

    fn set_skip_taskbar(&self, _skip: bool) -> Result<()> {
        Ok(())
    }

    fn set_cursor_grab(&self, _grab: bool) -> Result<()> {
        Ok(())
    }

    fn set_cursor_visible(&self, _visible: bool) -> Result<()> {
        Ok(())
    }

    fn set_cursor_icon(&self, _icon: CursorIcon) -> Result<()> {
        Ok(())
    }

    fn set_cursor_position<Pos: Into<Position>>(&self, _position: Pos) -> Result<()> {
        Ok(())
    }

    fn set_ignore_cursor_events(&self, _ignore: bool) -> Result<()> {
        Ok(())
    }

    fn start_dragging(&self) -> Result<()> {
        Ok(())
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

    fn set_theme(&self, _theme: Option<Theme>) -> Result<()> {
        Ok(())
    }
}

impl<T: UserEvent> WebviewDispatch<T> for ServocatWebviewDispatch<T> {
    type Runtime = ServocatRuntime<T>;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, _f: F) -> Result<()> {
        Err(Error::EventLoopClosed)
    }

    fn on_webview_event<F: Fn(&WebviewEvent) + Send + 'static>(&self, _f: F) -> WebviewEventId {
        0
    }

    fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(
        &self,
        _f: F,
    ) -> Result<()> {
        Err(Error::FailedToSendMessage)
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
        Ok(Rect {
            position: tauri_runtime::dpi::Position::Physical(PhysicalPosition::new(0, 0)),
            size: tauri_runtime::dpi::Size::Physical(PhysicalSize::new(0, 0)),
        })
    }

    fn position(&self) -> Result<PhysicalPosition<i32>> {
        Ok(PhysicalPosition::new(0, 0))
    }

    fn size(&self) -> Result<PhysicalSize<u32>> {
        Ok(PhysicalSize::new(0, 0))
    }

    fn navigate(&self, url: Url) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Navigate {
                window_id: self.window_id,
                url: url.into(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn reload(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Reload {
                window_id: self.window_id,
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn print(&self) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn close(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Close { id: self.window_id })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn set_bounds(&self, _bounds: Rect) -> Result<()> {
        Ok(())
    }

    fn set_size(&self, _size: Size) -> Result<()> {
        Ok(())
    }

    fn set_position(&self, _position: Position) -> Result<()> {
        Ok(())
    }

    fn set_focus(&self) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::Focus { id: self.window_id })
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
                window_id: self.window_id,
                script: script.into(),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn eval_script_with_callback<S: Into<String>>(
        &self,
        script: S,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        self.proxy
            .send_event(RuntimeEvent::EvalScriptWithCallback {
                window_id: self.window_id,
                script: script.into(),
                callback: Box::new(callback),
            })
            .map_err(|_e| Error::EventLoopClosed)
    }

    fn reparent(&self, _window_id: WindowId) -> Result<()> {
        Err(Error::WindowNotFound)
    }

    fn cookies_for_url(&self, _url: Url) -> Result<Vec<Cookie<'static>>> {
        Ok(Vec::new())
    }

    fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
        Ok(Vec::new())
    }

    fn set_cookie(&self, _cookie: Cookie<'_>) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn delete_cookie(&self, _cookie: Cookie<'_>) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn set_auto_resize(&self, _auto_resize: bool) -> Result<()> {
        Ok(())
    }

    fn set_zoom(&self, _scale_factor: f64) -> Result<()> {
        Ok(())
    }

    fn set_background_color(&self, _color: Option<Color>) -> Result<()> {
        Ok(())
    }

    fn clear_all_browsing_data(&self) -> Result<()> {
        Ok(())
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
    proxy
        .send_event(RuntimeEvent::CreateWebview {
            window_id,
            label: label.clone(),
            url: pending.url,
        })
        .map_err(|err| Error::CreateWebview(skeleton_error_from(err.to_string())))?;
    Ok(DetachedWebview {
        label,
        dispatcher: ServocatWebviewDispatch {
            proxy: proxy.clone(),
            window_id,
            _marker: PhantomData,
        },
    })
}

/// Per-window webview state held by [`AppHandler`].  v1.2 keeps the
/// caller-supplied HTML/CSS so `reload` + `eval_script` can rebuild
/// the pipeline without re-fetching.
struct WebviewState {
    html: String,
    css: String,
    viewport: Viewport,
    current_frame: Option<Frame>,
    text_renderer: TextRenderer,
}

impl WebviewState {
    fn new(html: String, viewport: Viewport) -> Self {
        Self {
            html,
            css: String::new(),
            viewport,
            current_frame: None,
            text_renderer: TextRenderer::new(),
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
            &HostCommands::new(),
        )
        .ok()?;
        let value = format!("{}", frame.script_value());
        self.current_frame = Some(frame);
        Some(value)
    }
}

/// Extract a usable HTML body from a URL.  Currently handles
/// `data:text/html,<body>` (with optional `;charset=utf-8` /
/// `;base64` params dropped); everything else falls back to a
/// placeholder body containing the URL.  Percent-decoding is
/// intentionally not done in v1.2 -- pass literal HTML in the data
/// URL body.
fn html_from_url(url: &str) -> String {
    let parsed = url::Url::parse(url).ok();
    let from_data = parsed
        .as_ref()
        .filter(|u| u.scheme() == "data")
        .and_then(|u| u.path().split_once(','))
        .map(|(_mime, body)| body.to_owned());
    from_data.unwrap_or_else(|| {
        format!(
            "<html><body><h1>tauri-runtime-servocat</h1>\
             <p>Unsupported URL scheme.  v1.2 handles \
             <code>data:text/html,...</code>; got: {url}</p></body></html>"
        )
    })
}

fn skeleton_error_from(msg: String) -> Box<dyn std::error::Error + Send + Sync> {
    Box::from(msg)
}

struct AppHandler<T: UserEvent, F: FnMut(RunEvent<T>) + 'static> {
    callback: F,
    windows: BTreeMap<u32, Window>,
    labels: BTreeMap<u32, String>,
    winit_to_tauri: BTreeMap<WinitWindowId, u32>,
    webviews: BTreeMap<u32, WebviewState>,
    ready_dispatched: bool,
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent, F: FnMut(RunEvent<T>) + 'static> AppHandler<T, F> {
    fn new(callback: F) -> Self {
        Self {
            callback,
            windows: BTreeMap::new(),
            labels: BTreeMap::new(),
            winit_to_tauri: BTreeMap::new(),
            webviews: BTreeMap::new(),
            ready_dispatched: false,
            _marker: PhantomData,
        }
    }

    fn dispatch_window_event(&mut self, raw_id: u32, event: TauriWindowEvent) {
        let label = self.labels.get(&raw_id).cloned().unwrap_or_default();
        (self.callback)(RunEvent::WindowEvent { label, event });
    }

    fn request_redraw(&self, raw_id: u32) {
        let _ = self.windows.get(&raw_id).map(Window::request_redraw);
    }

    fn present(&mut self, raw_id: u32) {
        let window_opt = self.windows.get(&raw_id);
        let webview_opt = self.webviews.get_mut(&raw_id);
        let _ = window_opt
            .zip(webview_opt)
            .and_then(|(window, webview)| present_webview(window, webview));
    }
}

fn present_webview(window: &Window, webview: &mut WebviewState) -> Option<()> {
    let size = window.inner_size();
    let width = NonZeroU32::new(size.width)?;
    let height = NonZeroU32::new(size.height)?;
    let frame = webview.current_frame.as_ref()?;
    let pixels = render_to_pixels_with(frame, size.width, size.height, &mut webview.text_renderer);
    let context = Context::new(window).ok()?;
    // FFI carve-out: softbuffer's `Surface` / `Buffer` require `&mut`
    // for `resize`, `buffer_mut`, and `present`.
    let mut surface = Surface::new(&context, window).ok()?;
    surface.resize(width, height).ok()?;
    let mut buffer = surface.buffer_mut().ok()?;
    write_pixels(&pixels, &mut buffer);
    buffer.present().ok()?;
    Some(())
}

fn write_pixels(pixels: &PixelBuffer, buffer: &mut softbuffer::Buffer<'_, &Window, &Window>) {
    pixels
        .rgba()
        .chunks_exact(4)
        .map(rgba_to_softbuffer_word)
        .zip(buffer.iter_mut())
        .for_each(|(word, slot)| *slot = word);
}

fn rgba_to_softbuffer_word(chunk: &[u8]) -> u32 {
    let red = chunk.first().copied().unwrap_or(0);
    let green = chunk.get(1).copied().unwrap_or(0);
    let blue = chunk.get(2).copied().unwrap_or(0);
    let alpha = chunk.get(3).copied().unwrap_or(0);
    // Composite premultiplied RGBA over a solid white background:
    //   out = src + (1 - src_a) * dst  with dst = (1,1,1)
    let inv = 255_u8 - alpha;
    let out_r = red.saturating_add(inv);
    let out_g = green.saturating_add(inv);
    let out_b = blue.saturating_add(inv);
    (u32::from(out_r) << 16) | (u32::from(out_g) << 8) | u32::from(out_b)
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
                label: _label,
                url,
            } => {
                let raw = raw_window_id(window_id);
                let viewport = self.windows.get(&raw).map_or_else(
                    || Viewport::new(800, 600),
                    |window| {
                        let size = window.inner_size();
                        Viewport::new(size.width, size.height)
                    },
                );
                let html = html_from_url(&url);
                let mut state = WebviewState::new(html, viewport);
                state.rebuild_frame();
                let _ = self.webviews.insert(raw, state);
                self.request_redraw(raw);
            }
            RuntimeEvent::Navigate { window_id, url } => {
                let raw = raw_window_id(window_id);
                let html = html_from_url(&url);
                let _ = self.webviews.get_mut(&raw).map(|webview| {
                    webview.html = html;
                    webview.rebuild_frame();
                });
                self.request_redraw(raw);
            }
            RuntimeEvent::Reload { window_id } => {
                let raw = raw_window_id(window_id);
                let _ = self.webviews.get_mut(&raw).map(WebviewState::rebuild_frame);
                self.request_redraw(raw);
            }
            RuntimeEvent::EvalScript { window_id, script } => {
                let raw = raw_window_id(window_id);
                let _ = self
                    .webviews
                    .get_mut(&raw)
                    .map(|webview| webview.eval(&script));
                self.request_redraw(raw);
            }
            RuntimeEvent::EvalScriptWithCallback {
                window_id,
                script,
                callback,
            } => {
                let raw = raw_window_id(window_id);
                let result = self
                    .webviews
                    .get_mut(&raw)
                    .and_then(|webview| webview.eval(&script))
                    .unwrap_or_default();
                callback(result);
                self.request_redraw(raw);
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
