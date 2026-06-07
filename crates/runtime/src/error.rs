//! Typed, causal error surface for the runtime (RT-001 · A1).
//!
//! The runtime NEVER panics on user JS (CRAFT Part B): an uncaught exception, a
//! module error, or an event-loop failure each becomes one of these variants and
//! `Display`s as a diagnostic that names the cause. `unwrap`/`expect`/`panic!` are
//! reserved for genuine internal invariants, never any path reachable from user JS.

use deno_core::error::{CoreError, CoreErrorKind, JsError};

/// Typed runtime failure. Every fallible runtime entrypoint returns this.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// V8 / isolate construction failed (not reachable from user JS).
    #[error("failed to initialize the V8 runtime: {0}")]
    Init(String),
    /// The module graph could not be loaded/instantiated (resolution failure,
    /// missing file, or a syntax/parse error in the source).
    #[error("module error for {specifier}: {source}")]
    Module {
        specifier: String,
        #[source]
        source: Box<CoreError>,
    },
    /// An uncaught JS exception — surfaced as data, never a Rust panic.
    #[error("uncaught exception in {specifier}:\n{report}")]
    Uncaught {
        specifier: String,
        report: Box<JsExceptionReport>,
    },
    /// The event loop failed for a non-JS-exception reason (e.g. a stalled
    /// top-level await deadlock or a terminated isolate).
    #[error("event loop error: {0}")]
    EventLoop(#[source] Box<CoreError>),
}

/// Structured form of a thrown JS value (not a stringly error). Built from
/// deno_core's [`JsError`]; carries the message, stack, and the top frame's
/// source position so diagnostics can point at the fix.
#[derive(Debug)]
pub struct JsExceptionReport {
    pub message: String,
    pub stack: Option<String>,
    pub file_name: Option<String>,
    pub line_number: Option<i64>,
    pub column_number: Option<i64>,
}

impl From<&JsError> for JsExceptionReport {
    fn from(err: &JsError) -> Self {
        // Prefer the rendered exception message ("Uncaught TypeError: boom"),
        // falling back to the bare `message` field.
        let message = if err.exception_message.is_empty() {
            err.message.clone().unwrap_or_default()
        } else {
            err.exception_message.clone()
        };
        let top = err.frames.first();
        JsExceptionReport {
            message,
            stack: err.stack.clone(),
            file_name: top.and_then(|f| f.file_name.clone()),
            line_number: top.and_then(|f| f.line_number),
            column_number: top.and_then(|f| f.column_number),
        }
    }
}

impl std::fmt::Display for JsExceptionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(file) = &self.file_name {
            write!(f, "\n  at {file}")?;
            if let Some(line) = self.line_number {
                write!(f, ":{line}")?;
                if let Some(col) = self.column_number {
                    write!(f, ":{col}")?;
                }
            }
        }
        if let Some(stack) = &self.stack {
            write!(f, "\n{stack}")?;
        }
        Ok(())
    }
}

/// Map an error surfaced while evaluating a module / draining the loop to the
/// right typed variant: a JS exception becomes [`RuntimeError::Uncaught`];
/// anything else is a real event-loop failure.
pub(crate) fn classify_eval_error(specifier: &str, err: CoreError) -> RuntimeError {
    match find_js_error(err.as_kind()) {
        Some(js) => RuntimeError::Uncaught {
            specifier: specifier.to_string(),
            report: Box::new(JsExceptionReport::from(js)),
        },
        None => RuntimeError::EventLoop(Box::new(err)),
    }
}

/// Walk a [`CoreErrorKind`] for the underlying thrown JS value, unwrapping the
/// `CouldNotExecute` wrapper deno_core adds around module evaluation failures.
fn find_js_error(kind: &CoreErrorKind) -> Option<&JsError> {
    match kind {
        CoreErrorKind::Js(js) => Some(js),
        CoreErrorKind::CouldNotExecute { error, .. } => find_js_error(error),
        _ => None,
    }
}

/// Build an [`RuntimeError::Uncaught`] from a script-eval `JsError`.
pub(crate) fn uncaught_from_js(specifier: &str, err: &JsError) -> RuntimeError {
    RuntimeError::Uncaught {
        specifier: specifier.to_string(),
        report: Box::new(JsExceptionReport::from(err)),
    }
}
