//! Typed errors for the graph facade. No `unwrap`/`expect` on the parse path —
//! Oxc recovers from syntax errors and the CST is still produced.

use std::path::PathBuf;

use oxc_span::Span;

/// Errors surfaced by the public facade. Consumers (RT, LOAD, TOOL, …) match on
/// this rather than on a salsa/Oxc internal.
#[derive(thiserror::Error, Debug)]
pub enum GraphError {
    /// A path that was never registered via `set_file` (or has been removed).
    #[error("unknown file: {0}")]
    UnknownFile(PathBuf),
}

/// A stage-5 lowering diagnostic: why a file cannot be lowered to runtime IR.
///
/// Covers a recovered parse/semantic error, a [`crate::StripPolicy`] rejection, or
/// the not-yet-available type strip (RT-003). Cause + remedy + span: the message
/// states *why* lowering failed and `help` states the fix. RT-003 supplies the real
/// strip-policy messages; this type is the wire the seam carries.
#[derive(Debug, Clone)]
pub struct StripDiagnostic {
    pub span: Span,
    pub message: String,
    pub help: String,
}
