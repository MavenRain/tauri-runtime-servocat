//! Servo-replacement runtime for Tauri.
//!
//! v0.1 shipped the headless pipeline + script driver; v0.2 added a
//! tiny-skia rasterizer ([`render_to_pixels`]); v0.3 adds cosmic-text
//! shaping + swash glyph raster so `FillText` commands render real
//! text.  Reuse a [`TextRenderer`] across calls via
//! [`render_to_pixels_with`] to amortize font-loading cost.
//!
//! # Example
//!
//! ```
//! # fn main() -> Result<(), tauri_runtime_servocat::Error> {
//! use tauri_runtime_servocat::{Viewport, render, render_to_pixels, run_script};
//!
//! let frame = render(
//!     "<html><body><p>hello</p></body></html>",
//!     "p { background-color: red; padding: 8px; }",
//!     Viewport::new(800, 600),
//! )?;
//! assert!(!frame.display_list().is_empty());
//!
//! let pixels = render_to_pixels(&frame, 800, 600);
//! assert_eq!(pixels.rgba().len(), 800 * 600 * 4);
//!
//! let scripted = run_script(
//!     "<html><body><p id='g'>hi</p></body></html>",
//!     "",
//!     "document.getElementById('g').textContent",
//!     Viewport::new(800, 600),
//! )?;
//! assert_eq!(format!("{}", scripted.script_value()), "\"hi\"");
//! # Ok(())
//! # }
//! ```

#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![allow(clippy::similar_names)]

pub mod error;
pub mod frame;
pub mod pipeline;
pub mod raster;
pub mod script;
pub mod text;

pub use error::Error;
pub use frame::Frame;
pub use layout_cat::Viewport;
pub use pipeline::render;
pub use raster::{PixelBuffer, render_to_pixels, render_to_pixels_with};
pub use script::{DEFAULT_FUEL, run_script};
pub use text::TextRenderer;
