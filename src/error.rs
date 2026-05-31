//! Meta-crate error type.

use boa_cat::Error as EngineError;
use css_cat::Error as CssError;
use html_cat::Error as HtmlError;
use web_api_cat::Error as WebApiError;

/// All errors `tauri-runtime-servocat` can produce.  Most variants wrap
/// errors from the underlying cat-stack crates; [`Self::Window`]
/// carries a stringified message from winit / softbuffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An HTML-parser error.
    Html(HtmlError),
    /// A CSS-parser error.
    Css(CssError),
    /// A JS engine error.
    Engine(EngineError),
    /// A web-api bridge error.
    WebApi(WebApiError),
    /// A windowing / surface error (winit, softbuffer).  The original
    /// error types are stringified to keep `Error` `Clone + PartialEq`
    /// and avoid leaking those crates' types from the public API.
    Window(String),
}

impl From<HtmlError> for Error {
    fn from(value: HtmlError) -> Self {
        Self::Html(value)
    }
}

impl From<CssError> for Error {
    fn from(value: CssError) -> Self {
        Self::Css(value)
    }
}

impl From<EngineError> for Error {
    fn from(value: EngineError) -> Self {
        Self::Engine(value)
    }
}

impl From<WebApiError> for Error {
    fn from(value: WebApiError) -> Self {
        Self::WebApi(value)
    }
}

impl From<winit::error::EventLoopError> for Error {
    fn from(value: winit::error::EventLoopError) -> Self {
        Self::Window(value.to_string())
    }
}

impl From<winit::error::OsError> for Error {
    fn from(value: winit::error::OsError) -> Self {
        Self::Window(value.to_string())
    }
}

impl From<softbuffer::SoftBufferError> for Error {
    fn from(value: softbuffer::SoftBufferError) -> Self {
        Self::Window(value.to_string())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Html(e) => write!(f, "html error: {e}"),
            Self::Css(e) => write!(f, "css error: {e}"),
            Self::Engine(e) => write!(f, "engine error: {e}"),
            Self::WebApi(e) => write!(f, "web-api error: {e}"),
            Self::Window(msg) => write!(f, "window error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
