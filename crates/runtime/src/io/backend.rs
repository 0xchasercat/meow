//! Portable async I/O backend (RT-002 · ADR-9).
//!
//! The committed cross-platform layer is **`tokio`/`mio`** (ADR-9: the portability
//! commitment is the `tokio`/`mio` abstraction; `epoll`/`kqueue`/IOCP are reached
//! transparently through `mio`). This module is that committed layer and is the
//! wired default everywhere.
//!
//! ## `io_uring` is DEFERRED (not scaffolded here)
//!
//! `io_uring` is a Linux-only *accelerant* behind this same abstraction, NOT the
//! portability story (ADR-9) — and it carries NO performance claim (I-11): any
//! "faster I/O" assertion needs a named workload + reproducible benchmark, which
//! RT-002 ships none of. `tokio-uring` requires its own current-thread uring
//! driver, so marrying it to the deno_core event loop is a named follow-up
//! (`RT-002a`). Per CRAFT (no speculative code), RT-002 ships **no** untested
//! Linux-only `io_uring` code: when `RT-002a` lands it will introduce a
//! `cfg`-gated arm exposing this exact signature, leaving callers (ops) unchanged.

use std::net::SocketAddr;
use std::path::Path;

/// Read an entire file into memory off the V8 thread (tokio's blocking pool).
/// Returns the raw bytes; the caller maps the `io::Error` to a typed op error.
pub async fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

/// Open a TCP connection. Returns the connected stream; the caller stores it as
/// a resource and maps the `io::Error` to a typed op error.
pub async fn tcp_connect(addr: SocketAddr) -> std::io::Result<tokio::net::TcpStream> {
    tokio::net::TcpStream::connect(addr).await
}
