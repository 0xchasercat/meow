//! `meow-ui` — the structural-cuteness terminal engine.
//!
//! The crate keeps vibe and engineering separate:
//! - envelope headers/icons/colors make the CLI feel like meow;
//! - bodies, snippets, and boxes remain technically precise;
//! - every renderer is pure and testable;
//! - the IO facade disables colors/animations for pipes/CI.

pub mod bento;
pub mod diagnostic;
pub mod envelope;
pub mod facade;
pub mod spinner;
pub mod theme;
pub mod waterfall;

pub use diagnostic::SourceDiagnostic;
pub use envelope::Tone;
pub use facade::Ui;
pub use spinner::{Spinner, WALKING_PAW_FRAMES};
pub use theme::{Rgb, Style};
