//! Async host-I/O layer (RT-002 · ADR-9, CANON §6.2).
//!
//! The single governed path from JS to host I/O: tokio-backed async ops behind
//! the [`meow_io`] extension, each routed through the [`CapabilityCheck`] seam
//! (I-6). The committed backend is `tokio`/`mio` ([`backend`]); `io_uring` is a
//! deferred Linux accelerant with NO performance claim (see [`backend`] docs and
//! `RT-002a`). This crate remains the only V8/deno_core edge (I-10).

mod backend;
pub mod capability;
pub mod ops;

use std::rc::Rc;

use deno_core::OpState;

pub use capability::{AllowAll, CapDenied, CapRequest, CapabilityCheck};
pub use ops::{op_read_file, op_tcp_connect, RuntimeIoError, TcpStreamResource};

// Raw host-I/O ops. **Unmediated internal plumbing — NOT a user-visible API.**
//
// This extension is deliberately *not* installed by `Runtime::new`'s default
// set: a default runtime (what `meow run` uses) must not expose `op_read_file`
// / `op_tcp_connect` to user JS, or any executed module would hold ambient host
// FS/network authority (I-6, I-8). It is opt-in — internal consumers install it
// via `RuntimeOptions.extensions` alongside `io_capability_extension` for a
// non-`AllowAll` policy. The ops only go live behind a mediated,
// capability-enforced surface in a later spec (meow:fs + SEC/P6); until then
// they are plumbing for tests + future internal callers.
deno_core::extension!(
    meow_io,
    ops = [crate::io::ops::op_read_file, crate::io::ops::op_tcp_connect],
    // Seed the capability seam so every op finds a checker in OpState. Default
    // is ALLOW (P0 runnable); a later extension may overwrite it. NOT a security
    // boundary — see capability.rs TODO(SEC-001, P6).
    state = |state: &mut OpState| {
        state.put::<Rc<dyn CapabilityCheck>>(Rc::new(AllowAll));
    },
);

/// Build an [`Extension`](deno_core::Extension) that installs a custom
/// [`CapabilityCheck`] into `OpState`, overwriting the default [`AllowAll`].
///
/// Pass it through `RuntimeOptions.extensions` *after* the crate's own
/// extensions (the normal caller position) so its `OpState` seed wins. Used by
/// tests to install a DENY policy and prove the seam is on the op path.
pub fn io_capability_extension(caps: Rc<dyn CapabilityCheck>) -> deno_core::Extension {
    deno_core::Extension {
        name: "meow_io_capability",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            state.put::<Rc<dyn CapabilityCheck>>(caps.clone());
        })),
        ..Default::default()
    }
}
