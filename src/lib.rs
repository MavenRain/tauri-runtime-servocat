//! Servo-replacement runtime for Tauri (v0.1.0: headless pipeline +
//! script driver).
//!
//! # Example
//!
//! ```
//! # fn main() -> Result<(), tauri_runtime_servocat::Error> {
//! use tauri_runtime_servocat::{Viewport, render, run_script};
//!
//! let frame = render(
//!     "<html><body><p>hello</p></body></html>",
//!     "p { background-color: red; padding: 8px; }",
//!     Viewport::new(800, 600),
//! )?;
//! assert!(!frame.display_list().is_empty());
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
pub mod script;

pub use error::Error;
pub use frame::Frame;
pub use layout_cat::Viewport;
pub use pipeline::render;
pub use script::{DEFAULT_FUEL, run_script};
