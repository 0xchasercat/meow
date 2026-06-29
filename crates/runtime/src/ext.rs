//! Op / extension registration seam (RT-001 · A4).
//!
//! The seam is the plain `deno_core` idiom — a `Vec<Extension>` on
//! [`crate::RuntimeOptions`] plus this crate's own extension built with
//! `extension!`/`#[op2]`. No bespoke `OpProvider` trait: `Extension` already IS
//! the registration unit (CRAFT — avoid speculative generality).
//!
//! At P0 the base runtime installs ONLY [`op_print`] backing
//! `console.log`/`console.error` so a trivial program is observable. Optional
//! `meow:*` surfaces (RT-005 `meow:http`, UI-001 `meow:ui`) live in separate
//! extensions layered through [`crate::RuntimeOptions`].

use std::cell::Cell;
use std::io::{self, Write};
use std::rc::Rc;

use deno_core::{op2, OpState};

use crate::fs_events::{op_meow_fs_events_close, op_meow_fs_events_open, op_meow_fs_events_poll};
pub mod http;
pub mod test;
pub mod ui;

/// The print callback `(message, is_err)` — a type alias keeps [`PrintSink`]
/// legible (clippy::type_complexity).
pub type PrintFn = Rc<dyn Fn(&str, bool)>;

/// Where `op_print` writes. Stored in [`OpState`]; absent by default (→ the host
/// stdout/stderr seam). Tests insert their own to capture output without going
/// through the OS — proving the print path end-to-end deterministically.
///
/// The inner `Fn` is the only print boundary; it is the single sanctioned host
/// I/O point at P0 (I-6: no other ambient reads/writes are introduced).
#[derive(Clone)]
pub struct PrintSink(pub PrintFn);

#[derive(Clone)]
pub(crate) struct ProcessExitCode {
    code: deno_os::ExitCode,
    requested: Rc<Cell<bool>>,
}

impl ProcessExitCode {
    pub(crate) fn new(code: deno_os::ExitCode) -> Self {
        Self {
            code,
            requested: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn record(&mut self, code: i32) {
        self.requested.set(true);
        self.code.set(code);
    }

    pub(crate) fn take(&self) -> Option<i32> {
        if self.requested.replace(false) {
            Some(self.code.get())
        } else {
            None
        }
    }
}

/// The ONLY host I/O op in the base runtime extension. Synchronous, infallible-by-
/// contract write that honors a [`PrintSink`] override when present, else writes
/// to stdout/stderr.
fn is_internal_process_exit_payload(msg: &str) -> bool {
    let trimmed = msg.trim();
    trimmed.starts_with(r#"{"__meowProcessExit":true,"#) && trimmed.ends_with('}')
}

#[op2(fast)]
fn op_meow_print(
    state: &mut OpState,
    #[string] msg: &str,
    is_err: bool,
) -> Result<(), std::io::Error> {
    write_output(state, msg, is_err)
}

pub(crate) fn write_output(state: &mut OpState, msg: &str, is_err: bool) -> Result<(), io::Error> {
    if is_internal_process_exit_payload(msg) {
        return Ok(());
    }
    if let Some(sink) = state.try_borrow::<PrintSink>() {
        (sink.0)(msg, is_err);
        return Ok(());
    }
    if is_err {
        let mut err = io::stderr().lock();
        err.write_all(msg.as_bytes())?;
        err.flush()?;
    } else {
        let mut out = io::stdout().lock();
        out.write_all(msg.as_bytes())?;
        out.flush()?;
    }
    Ok(())
}

#[op2(fast)]
fn op_current_thread_cpu_usage(#[buffer] out: &mut [f64]) {
    if out.len() >= 2 {
        out[0] = 0.0;
        out[1] = 0.0;
    }
}

#[op2(fast)]
fn op_bootstrap_color_depth() -> i32 {
    24
}

#[op2]
#[serde]
fn op_http_serve_address_override() -> (i32, String, u16, bool) {
    (0, String::new(), 0, false)
}

#[op2]
#[serde]
fn op_bootstrap_unstable_args() -> Vec<String> {
    Vec::new()
}

#[op2(fast)]
fn op_meow_record_process_exit(state: &mut OpState, code: i32) {
    if let Some(exit_code) = state.try_borrow_mut::<ProcessExitCode>() {
        exit_code.record(code);
    }
}

#[op2]
#[string]
fn op_meow_host_platform() -> String {
    match std::env::consts::OS {
        "macos" => "darwin".to_owned(),
        "windows" => "win32".to_owned(),
        other => other.to_owned(),
    }
}

#[op2]
#[string]
fn op_meow_host_arch() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_owned(),
        "x86_64" => "x64".to_owned(),
        other => other.to_owned(),
    }
}

deno_core::extension!(
    meow_runtime,
    ops = [
        op_meow_print,
        op_current_thread_cpu_usage,
        op_bootstrap_color_depth,
        op_bootstrap_unstable_args,
        op_meow_host_platform,
        op_meow_host_arch,
        op_meow_record_process_exit,
        op_http_serve_address_override,
        op_meow_fs_events_open,
        op_meow_fs_events_poll,
        op_meow_fs_events_close
    ],
    esm_entry_point = "ext:meow_runtime/bootstrap.js",
    esm = [dir "src/js", "bootstrap.js"],
);
pub use http::http_extension;
pub use test::test_extension;
pub use ui::ui_extension;

/// Build an [`Extension`](deno_core::Extension) that seeds a [`PrintSink`] into
/// `OpState`. Pass it through `RuntimeOptions.extensions` to redirect
/// `console.log`/`console.error` (used by tests to capture output).
pub fn print_sink_extension(sink: PrintSink) -> deno_core::Extension {
    deno_core::Extension {
        name: "meow_runtime_print_sink",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            state.put(sink);
        })),
        ..Default::default()
    }
}
