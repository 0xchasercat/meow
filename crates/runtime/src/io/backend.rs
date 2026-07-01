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
use std::sync::OnceLock;

use tokio::sync::Semaphore;

const FS_PERMITS: usize = 256;

fn fs_semaphore() -> &'static Semaphore {
    static SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();
    SEMAPHORE.get_or_init(|| Semaphore::new(FS_PERMITS))
}

/// Read an entire file into memory off the V8 thread (tokio's blocking pool).
/// Returns the raw bytes; the caller maps the `io::Error` to a typed op error.
pub async fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let _permit = fs_semaphore()
        .acquire()
        .await
        .expect("filesystem semaphore closed");
    tokio::fs::read(path).await
}

/// Write an entire file off the V8 thread.
pub async fn write_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _permit = fs_semaphore()
        .acquire()
        .await
        .expect("filesystem semaphore closed");
    tokio::fs::write(path, bytes).await
}

/// Read directory entry names. Sorted for deterministic tests.
pub async fn read_dir(path: &Path) -> std::io::Result<Vec<String>> {
    let _permit = fs_semaphore()
        .acquire()
        .await
        .expect("filesystem semaphore closed");
    let mut dir = tokio::fs::read_dir(path).await?;
    let mut entries = Vec::new();
    while let Some(entry) = dir.next_entry().await? {
        entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    entries.sort();
    Ok(entries)
}

/// Metadata for a path.
pub async fn stat(path: &Path) -> std::io::Result<std::fs::Metadata> {
    let _permit = fs_semaphore()
        .acquire()
        .await
        .expect("filesystem semaphore closed");
    tokio::fs::metadata(path).await
}

/// Create a directory (or directory tree when `recursive`).
pub async fn mkdir(path: &Path, recursive: bool) -> std::io::Result<()> {
    if recursive {
        tokio::fs::create_dir_all(path).await
    } else {
        tokio::fs::create_dir(path).await
    }
}

/// Open a TCP connection. Returns the connected stream; the caller stores it as
/// a resource and maps the `io::Error` to a typed op error.
pub async fn tcp_connect(addr: SocketAddr) -> std::io::Result<tokio::net::TcpStream> {
    tokio::net::TcpStream::connect(addr).await
}
