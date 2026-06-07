//! Op / extension registration seam (RT-001 · A4).
//!
//! The seam is the plain `deno_core` idiom — a `Vec<Extension>` on
//! [`crate::RuntimeOptions`] plus this crate's own extension built with
//! `extension!`/`#[op2]`. No bespoke `OpProvider` trait: `Extension` already IS
//! the registration unit (CRAFT — avoid speculative generality).
//!
//! At P0 the only host I/O op is [`op_print`] (the "hello op") backing
//! `console.log`/`console.error` so a trivial program is observable. The async
//! I/O layer is RT-002; Web globals are RT-004.

use std::io::Write;
use std::rc::Rc;

use deno_core::{op2, OpState};

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

/// The ONLY host I/O op at P0. Synchronous, infallible-by-contract write that
/// honors a [`PrintSink`] override when present, else writes to stdout/stderr.
#[op2(fast)]
fn op_meow_print(
    state: &mut OpState,
    #[string] msg: &str,
    is_err: bool,
) -> Result<(), std::io::Error> {
    if let Some(sink) = state.try_borrow::<PrintSink>() {
        (sink.0)(msg, is_err);
        return Ok(());
    }
    let mut out: Box<dyn Write> = if is_err {
        Box::new(std::io::stderr())
    } else {
        Box::new(std::io::stdout())
    };
    out.write_all(msg.as_bytes())?;
    out.flush()?;
    Ok(())
}

deno_core::extension!(
    meow_runtime,
    ops = [op_meow_print],
    esm_entry_point = "ext:meow_runtime/bootstrap.js",
    esm = [dir "src/js", "bootstrap.js"],
);

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
