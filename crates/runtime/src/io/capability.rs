//! Capability seam for host I/O (RT-002 · I-6).
//!
//! Every host-I/O op consults a [`CapabilityCheck`] *before* touching the host,
//! so there is exactly one governed entry from JS to host I/O. This is the
//! mechanism that later makes I-6 (determinism & hermeticity) enforceable: there
//! is no second, ungoverned path.
//!
//! IMPORTANT: this is the **seam only**, NOT a security boundary yet. The
//! default [`AllowAll`] permits everything so P0 is runnable. Real tiered
//! enforcement (I-8, gate `capability`) lands at P6.
//
// TODO(SEC-001, P6): replace `AllowAll` with real per-capability grant
// enforcement. RT-002 only stands up the seam + proves every op routes through
// it; it makes no security claim.

use std::path::Path;

/// A host-I/O request presented to the capability seam before the op acts.
/// Borrows its subject so the check allocates nothing on the hot path.
pub enum CapRequest<'a> {
    /// Read the file at this path.
    ReadFile(&'a Path),
    /// Open a TCP connection to this address string (as supplied by JS).
    NetConnect(&'a str),
    // === RT-005 ===
    /// Bind a listening socket (`meow:http` serve). The seam is on the path now;
    /// scoped/tiered enforcement still lands later (SEC-001 / RT-006).
    NetListen(&'a std::net::SocketAddr),
    // === /RT-005 ===
}

/// Denial returned by a [`CapabilityCheck`]. Carries a human-readable subject
/// so the rejected JS promise names what was refused (never panics).
#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
#[error("permission denied: {0}")]
pub struct CapDenied(pub String);

/// The seam hook stored in `OpState` as `Rc<dyn CapabilityCheck>`. Each host op
/// calls [`check`](CapabilityCheck::check) before performing I/O.
pub trait CapabilityCheck {
    /// Allow (`Ok`) or refuse (`Err`) a host-I/O request. MUST NOT perform I/O
    /// itself or read ambient host state (I-6).
    fn check(&self, req: &CapRequest<'_>) -> Result<(), CapDenied>;
}

/// Default seam implementation: allows everything. P0 is runnable; P6 swaps in
/// real enforcement (see module `TODO(SEC-001)`). This is NOT a security
/// boundary.
pub struct AllowAll;

impl CapabilityCheck for AllowAll {
    fn check(&self, _req: &CapRequest<'_>) -> Result<(), CapDenied> {
        Ok(())
    }
}
