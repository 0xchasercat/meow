//! Host-I/O ops + their typed error (RT-002).
//!
//! Two async ops behind the `meow_io` extension: [`op_read_file`] and
//! [`op_tcp_connect`]. Both consult the [`CapabilityCheck`] seam *before*
//! touching the host (I-6) and resolve under the deno_core event loop driven by
//! the active (current-thread) tokio runtime. No `unwrap`/`expect`/`panic!` on
//! any user-reachable line: every failure is a typed [`RuntimeIoError`] surfaced
//! to JS as a rejected promise (CRAFT: the runtime does not panic on user input).

use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;

use deno_core::{op2, AsyncRefCell, OpState, Resource, ResourceId};

use crate::io::backend;
use crate::io::capability::{CapDenied, CapRequest, CapabilityCheck};

/// Typed failure surface for host-I/O ops. Implements `deno_error::JsError` so it
/// crosses into JS as a thrown error (rejected promise), never a Rust panic.
#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum RuntimeIoError {
    /// The capability seam refused the request (see `capability.rs`).
    #[class(inherit)]
    #[error(transparent)]
    Denied(
        #[from]
        #[inherit]
        CapDenied,
    ),
    /// The file could not be read (missing, not permitted by the OS, …).
    #[class(inherit)]
    #[error("read {path}: {source}")]
    Read {
        path: String,
        #[source]
        #[inherit]
        source: std::io::Error,
    },
    /// The supplied address did not parse as a `SocketAddr`.
    #[class(type)]
    #[error("invalid socket address {0:?}")]
    BadAddr(String),
    /// The TCP connection attempt failed (refused, unreachable, …).
    #[class(inherit)]
    #[error("connect {addr}: {source}")]
    Connect {
        addr: String,
        #[source]
        #[inherit]
        source: std::io::Error,
    },
}

/// Clone the capability seam out of `OpState`. Seeded by the `meow_io` extension
/// (default [`AllowAll`](crate::io::AllowAll)); a caller-supplied extension may
/// overwrite it (see [`io_capability_extension`](crate::io_capability_extension)).
fn capabilities(state: &Rc<RefCell<OpState>>) -> Rc<dyn CapabilityCheck> {
    state.borrow().borrow::<Rc<dyn CapabilityCheck>>().clone()
}

/// Read a whole file and resolve a `Promise<Uint8Array>` in JS with its bytes.
/// Consults the capability seam first; a denied/missing path rejects with a
/// typed error.
#[op2]
#[buffer]
pub async fn op_read_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<Vec<u8>, RuntimeIoError> {
    let p = PathBuf::from(&path);
    // seam, NOT enforcement (P6) — see capability.rs TODO(SEC-001).
    capabilities(&state).check(&CapRequest::ReadFile(&p))?;
    backend::read_file(&p)
        .await
        .map_err(|source| RuntimeIoError::Read { path, source })
}

/// Open a TCP connection and resolve a `ResourceId` into the deno_core
/// `resource_table`. Read/write on the resource are out of RT-002 scope; this
/// proves the async-net seam end-to-end. Consults the capability seam first.
#[op2]
#[smi]
pub async fn op_tcp_connect(
    state: Rc<RefCell<OpState>>,
    #[string] addr: String,
) -> Result<ResourceId, RuntimeIoError> {
    let sock: SocketAddr = addr
        .parse()
        .map_err(|_| RuntimeIoError::BadAddr(addr.clone()))?;
    capabilities(&state).check(&CapRequest::NetConnect(&addr))?;
    let stream = backend::tcp_connect(sock)
        .await
        .map_err(|source| RuntimeIoError::Connect { addr, source })?;
    let rid = state
        .borrow_mut()
        .resource_table
        .add(TcpStreamResource::new(stream));
    Ok(rid)
}

/// A connected TCP stream held in the resource table. The stream lives in an
/// [`AsyncRefCell`] so a later spec can add read/write ops without changing the
/// `connect` seam; RT-002 only needs the handle to exist and stay open.
pub struct TcpStreamResource {
    #[allow(dead_code, reason = "held open; read/write ops land in a later spec")]
    stream: AsyncRefCell<tokio::net::TcpStream>,
}

impl TcpStreamResource {
    fn new(stream: tokio::net::TcpStream) -> Self {
        Self {
            stream: AsyncRefCell::new(stream),
        }
    }
}

impl Resource for TcpStreamResource {
    fn name(&self) -> std::borrow::Cow<'_, str> {
        "tcpStream".into()
    }
}
