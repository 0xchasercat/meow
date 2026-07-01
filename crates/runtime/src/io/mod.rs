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

pub use capability::{
    sandbox_caps, AllowAll, CapDenied, CapRequest, CapabilityCheck, SandboxCaps, SandboxPolicy,
    SANDBOX_BYPASS_HINT,
};
pub use ops::{op_read_file, op_tcp_connect, RuntimeIoError, TcpStreamResource};

// Raw host-I/O ops. **Unmediated internal plumbing — NOT a user-visible API.**
//
// This extension is deliberately *not* installed by `Runtime::new`'s default
// set: a bare runtime stays host-pure until a caller opts into these ops via
// `RuntimeOptions.extensions`. RT-007's Node built-ins are that first mediated
// user-facing surface: they call these ops from committed shims and route every
// host touch through the same capability seam.
deno_core::extension!(
    meow_io,
    ops = [
        crate::io::ops::op_read_file,
        crate::io::ops::op_node_read_file_sync,
        crate::io::ops::op_node_write_file,
        crate::io::ops::op_node_write_file_sync,
        crate::io::ops::op_node_readdir,
        crate::io::ops::op_node_readdir_sync,
        crate::io::ops::op_node_stat,
        crate::io::ops::op_node_stat_sync,
        crate::io::ops::op_node_mkdir,
        crate::io::ops::op_node_mkdir_sync,
        crate::io::ops::op_node_access,
        crate::io::ops::op_node_access_sync,
        crate::io::ops::op_node_rm,
        crate::io::ops::op_node_rm_sync,
        crate::io::ops::op_node_exists_sync,
        crate::io::ops::op_node_cwd,
        crate::io::ops::op_tcp_connect,
    ],
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
