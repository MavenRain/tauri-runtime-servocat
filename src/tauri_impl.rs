//! v1.0 implementation of the `tauri_runtime::Runtime` trait surface.
//!
//! This is a *skeleton*: every non-cosmetic method on each of the five
//! traits ([`Runtime`], [`RuntimeHandle`], [`WindowDispatch`],
//! [`WebviewDispatch`], [`EventLoopProxy`]) returns a
//! [`tauri_runtime::Error`] (typically [`Error::WindowNotFound`] for
//! window-side operations, [`Error::FailedToSendMessage`] for IPC,
//! and [`Error::CreateWindow`] / [`Error::CreateWebview`] for
//! lifecycle) or a sensible zero value (empty `Vec`, `None`,
//! `false`, `Theme::Light`).  Cosmetic / void methods are no-ops.
//!
//! The intent is to compile against `tauri-runtime = "2"` so a Tauri
//! app can be parameterized over [`ServocatRuntime`].  Real
//! implementations of each method will land in subsequent 1.x patch
//! releases (1.1: window lifecycle wired to [`crate::run_window`];
//! 1.2: webview eval through the IPC bridge; 1.3: real event loop /
//! proxy; ...).
//!
//! [`Runtime`]: tauri_runtime::Runtime
//! [`RuntimeHandle`]: tauri_runtime::RuntimeHandle
//! [`WindowDispatch`]: tauri_runtime::window::WindowBuilder
//! [`WebviewDispatch`]: tauri_runtime::WebviewDispatch
//! [`EventLoopProxy`]: tauri_runtime::EventLoopProxy
//! [`Error::WindowNotFound`]: tauri_runtime::Error::WindowNotFound

#![allow(clippy::needless_pass_by_value, clippy::unused_self, unexpected_cfgs)]

use std::marker::PhantomData;

use cookie::Cookie;
use raw_window_handle::{DisplayHandle, HandleError, WindowHandle};
use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size};
use tauri_runtime::monitor::Monitor;
use tauri_runtime::webview::{DetachedWebview, PendingWebview};
use tauri_runtime::window::{
    CursorIcon, DetachedWindow, PendingWindow, RawWindow, WebviewEvent, WindowBuilder,
    WindowBuilderBase, WindowEvent, WindowId, WindowSizeConstraints,
};
use tauri_runtime::{
    DeviceEventFilter, Error, EventLoopProxy, Icon, ProgressBarState, Result, RunEvent, Runtime,
    RuntimeHandle, RuntimeInitArgs, UserEvent, WebviewDispatch, WebviewEventId, WindowDispatch,
    WindowEventId,
};
use tauri_utils::config::{Color, WindowConfig};
use tauri_utils::{Theme, TitleBarStyle};
use url::Url;

/// The Servocat runtime.  Stateless skeleton; see module docs.
pub struct ServocatRuntime<T: UserEvent> {
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatRuntime<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ServocatRuntime").finish()
    }
}

impl<T: UserEvent> Default for ServocatRuntime<T> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

/// `Send`-able handle to a [`ServocatRuntime`].
pub struct ServocatHandle<T: UserEvent> {
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatHandle<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ServocatHandle").finish()
    }
}

impl<T: UserEvent> Clone for ServocatHandle<T> {
    fn clone(&self) -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T: UserEvent> Default for ServocatHandle<T> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

/// `Send`-able dispatcher for [`ServocatRuntime`] windows.
pub struct ServocatWindowDispatch<T: UserEvent> {
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatWindowDispatch<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ServocatWindowDispatch").finish()
    }
}

impl<T: UserEvent> Clone for ServocatWindowDispatch<T> {
    fn clone(&self) -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T: UserEvent> Default for ServocatWindowDispatch<T> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

/// `Send`-able dispatcher for [`ServocatRuntime`] webviews.
pub struct ServocatWebviewDispatch<T: UserEvent> {
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatWebviewDispatch<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ServocatWebviewDispatch").finish()
    }
}

impl<T: UserEvent> Clone for ServocatWebviewDispatch<T> {
    fn clone(&self) -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T: UserEvent> Default for ServocatWebviewDispatch<T> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

/// `Send`-able event-loop proxy for [`ServocatRuntime`].
pub struct ServocatEventLoopProxy<T: UserEvent> {
    _marker: PhantomData<fn() -> T>,
}

impl<T: UserEvent> std::fmt::Debug for ServocatEventLoopProxy<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ServocatEventLoopProxy").finish()
    }
}

impl<T: UserEvent> Clone for ServocatEventLoopProxy<T> {
    fn clone(&self) -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T: UserEvent> Default for ServocatEventLoopProxy<T> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

/// Builder for [`ServocatRuntime`] windows.  Captures none of the
/// attributes set on it; all settings are quietly discarded in this
/// skeleton.
#[derive(Debug, Default, Clone)]
pub struct ServocatWindowBuilder {
    has_icon: bool,
    theme: Option<Theme>,
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

    fn position(self, _x: f64, _y: f64) -> Self {
        self
    }

    fn inner_size(self, _width: f64, _height: f64) -> Self {
        self
    }

    fn min_inner_size(self, _min_width: f64, _min_height: f64) -> Self {
        self
    }

    fn max_inner_size(self, _max_width: f64, _max_height: f64) -> Self {
        self
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

    fn resizable(self, _resizable: bool) -> Self {
        self
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

    fn title<S: Into<String>>(self, _title: S) -> Self {
        self
    }

    fn fullscreen(self, _fullscreen: bool) -> Self {
        self
    }

    fn focused(self, _focused: bool) -> Self {
        self
    }

    fn focusable(self, _focusable: bool) -> Self {
        self
    }

    fn maximized(self, _maximized: bool) -> Self {
        self
    }

    fn visible(self, _visible: bool) -> Self {
        self
    }

    #[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]
    fn transparent(self, _transparent: bool) -> Self {
        self
    }

    fn decorations(self, _decorations: bool) -> Self {
        self
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

impl<T: UserEvent> EventLoopProxy<T> for ServocatEventLoopProxy<T> {
    fn send_event(&self, _event: T) -> Result<()> {
        Err(Error::EventLoopClosed)
    }
}

impl<T: UserEvent> Runtime<T> for ServocatRuntime<T> {
    type WindowDispatcher = ServocatWindowDispatch<T>;
    type WebviewDispatcher = ServocatWebviewDispatch<T>;
    type Handle = ServocatHandle<T>;
    type EventLoopProxy = ServocatEventLoopProxy<T>;

    fn new(_args: RuntimeInitArgs) -> Result<Self> {
        Ok(Self::default())
    }

    fn create_proxy(&self) -> Self::EventLoopProxy {
        ServocatEventLoopProxy::default()
    }

    fn handle(&self) -> Self::Handle {
        ServocatHandle::default()
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        _pending: PendingWindow<T, Self>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self>> {
        Err(Error::CreateWindow)
    }

    fn create_webview(
        &self,
        _window_id: WindowId,
        _pending: PendingWebview<T, Self>,
    ) -> Result<DetachedWebview<T, Self>> {
        Err(Error::CreateWebview(skeleton_error("create_webview")))
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

    fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, _callback: F) -> i32 {
        0
    }

    fn run<F: FnMut(RunEvent<T>) + 'static>(self, _callback: F) {}
}

impl<T: UserEvent> RuntimeHandle<T> for ServocatHandle<T> {
    type Runtime = ServocatRuntime<T>;

    fn create_proxy(&self) -> <Self::Runtime as Runtime<T>>::EventLoopProxy {
        ServocatEventLoopProxy::default()
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

    fn request_exit(&self, _code: i32) -> Result<()> {
        Err(Error::EventLoopClosed)
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        _pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        Err(Error::CreateWindow)
    }

    fn create_webview(
        &self,
        _window_id: WindowId,
        _pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        Err(Error::CreateWebview(skeleton_error("create_webview")))
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

    fn on_window_event<F: Fn(&WindowEvent) + Send + 'static>(&self, _f: F) -> WindowEventId {
        0
    }

    fn scale_factor(&self) -> Result<f64> {
        Ok(1.0)
    }

    fn inner_position(&self) -> Result<PhysicalPosition<i32>> {
        Ok(PhysicalPosition::new(0, 0))
    }

    fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
        Ok(PhysicalPosition::new(0, 0))
    }

    fn inner_size(&self) -> Result<PhysicalSize<u32>> {
        Ok(PhysicalSize::new(0, 0))
    }

    fn outer_size(&self) -> Result<PhysicalSize<u32>> {
        Ok(PhysicalSize::new(0, 0))
    }

    fn is_fullscreen(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_minimized(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_maximized(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_focused(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_decorated(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_resizable(&self) -> Result<bool> {
        Ok(false)
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
        Ok(false)
    }

    fn is_enabled(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_always_on_top(&self) -> Result<bool> {
        Ok(false)
    }

    fn title(&self) -> Result<String> {
        Ok(String::new())
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
        Ok(Theme::Light)
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
        _pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        Err(Error::CreateWindow)
    }

    fn create_webview(
        &mut self,
        _pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        Err(Error::CreateWebview(skeleton_error("create_webview")))
    }

    fn set_resizable(&self, _resizable: bool) -> Result<()> {
        Ok(())
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

    fn set_title<S: Into<String>>(&self, _title: S) -> Result<()> {
        Ok(())
    }

    fn maximize(&self) -> Result<()> {
        Ok(())
    }

    fn unmaximize(&self) -> Result<()> {
        Ok(())
    }

    fn minimize(&self) -> Result<()> {
        Ok(())
    }

    fn unminimize(&self) -> Result<()> {
        Ok(())
    }

    fn show(&self) -> Result<()> {
        Ok(())
    }

    fn hide(&self) -> Result<()> {
        Ok(())
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }

    fn destroy(&self) -> Result<()> {
        Ok(())
    }

    fn set_decorations(&self, _decorations: bool) -> Result<()> {
        Ok(())
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

    fn set_size(&self, _size: Size) -> Result<()> {
        Ok(())
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

    fn set_position(&self, _position: Position) -> Result<()> {
        Ok(())
    }

    fn set_fullscreen(&self, _fullscreen: bool) -> Result<()> {
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn set_simple_fullscreen(&self, _enable: bool) -> Result<()> {
        Ok(())
    }

    fn set_focus(&self) -> Result<()> {
        Ok(())
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

    fn navigate(&self, _url: Url) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn reload(&self) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn print(&self) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn close(&self) -> Result<()> {
        Ok(())
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
        Ok(())
    }

    fn hide(&self) -> Result<()> {
        Ok(())
    }

    fn show(&self) -> Result<()> {
        Ok(())
    }

    fn eval_script<S: Into<String>>(&self, _script: S) -> Result<()> {
        Err(Error::FailedToSendMessage)
    }

    fn eval_script_with_callback<S: Into<String>>(
        &self,
        _script: S,
        _callback: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        Err(Error::FailedToSendMessage)
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

fn skeleton_error(method: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::from(format!(
        "tauri-runtime-servocat 1.0 skeleton: {method} not implemented"
    ))
}
