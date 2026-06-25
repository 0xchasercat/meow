//! `meow-ui` — the capability-aware terminal UX engine for meow.
//!
//! Gorgeous on a modern terminal; clean and greppable in CI, pipes, and
//! NO_COLOR. Renderers are pure (caps in, string out); the `Ui` facade is the
//! one place that touches real streams and the animation gate.

pub mod banner;
pub mod caps;
pub mod diagnostic;
pub mod facade;
pub mod fmt;
pub mod glyph;
pub mod paint;
pub mod palette;
pub mod panel;
pub mod progress;
pub mod spinner;
pub mod status;
pub mod table;
pub mod waterfall;
pub mod width;

pub use banner::CommandGroup;
pub use caps::{Caps, TermEnv};
pub use diagnostic::SourceDiagnostic;
pub use facade::Ui;
pub use glyph::Glyphs;
pub use paint::{Attr, ColorLevel};
pub use palette::Rgb;
pub use progress::ProgressBar;
pub use spinner::Spinner;
pub use status::Tone;
pub use table::Align;
pub use waterfall::Span;
