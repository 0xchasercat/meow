//! Host-I/O ops + their typed error (RT-002 / RT-007).
//!
//! RT-002's raw async file-read + TCP-connect seam remains, and RT-007 extends it
//! with the first common Node-style filesystem slice (`fs`, `fs/promises`, and
//! `process.cwd()`). Every host touch still consults the one [`CapabilityCheck`]
//! seam *before* touching the host (I-6), and every failure is typed (CRAFT).

use std::borrow::Cow;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::UNIX_EPOCH;

use deno_core::{op2, AsyncRefCell, JsBuffer, OpState, Resource, ResourceId};
use serde::Serialize;

use crate::io::backend;
use crate::io::capability::{CapDenied, CapRequest, CapabilityCheck};

#[derive(Debug, Clone)]
pub struct PropStr(pub String);

impl std::fmt::Display for PropStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&PropStr> for deno_error::PropertyValue {
    fn from(p: &PropStr) -> Self {
        deno_error::PropertyValue::String(Cow::Owned(p.0.clone()))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PropStaticStr(pub &'static str);

impl std::fmt::Display for PropStaticStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&PropStaticStr> for deno_error::PropertyValue {
    fn from(p: &PropStaticStr) -> Self {
        deno_error::PropertyValue::String(Cow::Borrowed(p.0))
    }
}

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
    /// Filesystem operation failure (`readFile`, `stat`, `mkdir`, ...).
    #[class(inherit)]
    #[error("{code}: {detail}, {syscall} '{path}'")]
    Fs {
        #[property]
        syscall: PropStaticStr,
        #[property]
        path: PropStr,
        #[property]
        code: PropStaticStr,
        #[property]
        errno: i32,
        detail: &'static str,
        #[source]
        #[inherit]
        source: std::io::Error,
    },
    /// Reading the current working directory failed.
    #[class(inherit)]
    #[error("{code}: {detail}, cwd")]
    Cwd {
        code: &'static str,
        detail: &'static str,
        #[source]
        #[inherit]
        source: std::io::Error,
    },
    /// The supplied address did not parse as a `SocketAddr`.
    #[class(type)]
    #[error("EINVAL: invalid socket address '{0}'")]
    BadAddr(String),
    /// The TCP connection attempt failed (refused, unreachable, ...).
    #[class(inherit)]
    #[error("{code}: {detail}, connect '{addr}'")]
    Connect {
        addr: String,
        code: &'static str,
        detail: &'static str,
        #[source]
        #[inherit]
        source: std::io::Error,
    },
}

impl RuntimeIoError {
    fn fs(op: &'static str, path: String, source: std::io::Error) -> Self {
        let (code, detail, errno) = io_code_detail_and_errno(source.kind());
        Self::Fs {
            syscall: PropStaticStr(op),
            path: PropStr(path),
            code: PropStaticStr(code),
            errno,
            detail,
            source,
        }
    }

    fn cwd(source: std::io::Error) -> Self {
        let (code, detail, _) = io_code_detail_and_errno(source.kind());
        Self::Cwd {
            code,
            detail,
            source,
        }
    }

    fn connect(addr: String, source: std::io::Error) -> Self {
        let (code, detail, _) = io_code_detail_and_errno(source.kind());
        Self::Connect {
            addr,
            code,
            detail,
            source,
        }
    }
}

fn io_code_detail_and_errno(kind: std::io::ErrorKind) -> (&'static str, &'static str, i32) {
    use std::io::ErrorKind;

    match kind {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory", -2),
        ErrorKind::PermissionDenied => ("EACCES", "permission denied", -13),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists", -17),
        ErrorKind::InvalidInput => ("EINVAL", "invalid argument", -22),
        ErrorKind::InvalidData => ("EINVAL", "invalid data", -22),
        ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory", -21),
        ErrorKind::NotADirectory => ("ENOTDIR", "not a directory", -20),
        ErrorKind::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty", -39),
        ErrorKind::ReadOnlyFilesystem => ("EROFS", "read-only file system", -30),
        ErrorKind::StorageFull => ("ENOSPC", "no space left on device", -28),
        ErrorKind::BrokenPipe => ("EPIPE", "broken pipe", -32),
        ErrorKind::ConnectionRefused => ("ECONNREFUSED", "connection refused", -111),
        ErrorKind::ConnectionAborted => ("ECONNABORTED", "connection aborted", -103),
        ErrorKind::ConnectionReset => ("ECONNRESET", "connection reset by peer", -104),
        ErrorKind::TimedOut => ("ETIMEDOUT", "operation timed out", -110),
        ErrorKind::Unsupported => ("ENOSYS", "operation not supported", -38),
        _ => ("EIO", "I/O error", -5),
    }
}

/// Clone the capability seam out of `OpState`. Seeded by the `meow_io` extension
/// (default [`AllowAll`](crate::io::AllowAll)); a caller-supplied extension may
/// overwrite it (see [`io_capability_extension`](crate::io::io_capability_extension)).
fn capabilities(state: &Rc<RefCell<OpState>>) -> Rc<dyn CapabilityCheck> {
    state.borrow().borrow::<Rc<dyn CapabilityCheck>>().clone()
}

fn capabilities_sync(state: &OpState) -> Rc<dyn CapabilityCheck> {
    state
        .try_borrow::<Rc<dyn CapabilityCheck>>()
        .cloned()
        .unwrap_or_else(|| Rc::new(crate::io::AllowAll))
}
#[derive(Clone, Copy)]
enum PathCap {
    ReadFile,
    WriteFile,
    ReadDir,
    Stat,
    Mkdir,
    Access,
    Remove,
}

impl PathCap {
    fn request<'a>(self, path: &'a Path) -> CapRequest<'a> {
        match self {
            PathCap::ReadFile => CapRequest::ReadFile(path),
            PathCap::WriteFile => CapRequest::WriteFile(path),
            PathCap::ReadDir => CapRequest::ReadDir(path),
            PathCap::Stat => CapRequest::Stat(path),
            PathCap::Mkdir => CapRequest::Mkdir(path),
            PathCap::Access => CapRequest::Access(path),
            PathCap::Remove => CapRequest::Remove(path),
        }
    }
}

fn checked_path_sync(
    state: &OpState,
    path: &str,
    request: PathCap,
) -> Result<PathBuf, RuntimeIoError> {
    let path_buf = PathBuf::from(path);
    capabilities_sync(state).check(&request.request(&path_buf))?;
    Ok(path_buf)
}

fn checked_path_async(
    state: &Rc<RefCell<OpState>>,
    path: &str,
    request: PathCap,
) -> Result<PathBuf, RuntimeIoError> {
    let path_buf = PathBuf::from(path);
    capabilities(state).check(&request.request(&path_buf))?;
    Ok(path_buf)
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeStat {
    size: u64,
    readonly: bool,
    is_file: bool,
    is_directory: bool,
    is_symlink: bool,
    atime_ms: f64,
    mtime_ms: f64,
    ctime_ms: f64,
    birthtime_ms: f64,
}

fn time_ms(value: Option<std::time::SystemTime>) -> f64 {
    value
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs_f64() * 1_000.0)
        .unwrap_or(0.0)
}

fn stat_from(meta: std::fs::Metadata) -> NodeStat {
    let modified = meta.modified().ok();
    let created = meta.created().ok();
    NodeStat {
        size: meta.len(),
        readonly: meta.permissions().readonly(),
        is_file: meta.is_file(),
        is_directory: meta.is_dir(),
        is_symlink: meta.file_type().is_symlink(),
        atime_ms: time_ms(meta.accessed().ok()),
        mtime_ms: time_ms(modified),
        // Cross-platform `ctime` is not exposed by `std`; keep it aligned with
        // `mtime` for now instead of introducing platform-specific branches here.
        ctime_ms: time_ms(modified),
        birthtime_ms: time_ms(created.or(modified)),
    }
}

fn check_access_mode(meta: &std::fs::Metadata, mode: u32) -> Result<(), std::io::Error> {
    if mode & !0x7 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported access mode",
        ));
    }
    if mode == 0 {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let bits = meta.permissions().mode();
        if mode & 0x4 != 0 && bits & 0o444 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "read access denied",
            ));
        }
        if mode & 0x2 != 0 && bits & 0o222 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "write access denied",
            ));
        }
        if mode & 0x1 != 0 && bits & 0o111 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "execute access denied",
            ));
        }
    }

    #[cfg(not(unix))]
    {
        if mode & 0x2 != 0 && meta.permissions().readonly() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "write access denied",
            ));
        }
    }

    Ok(())
}

fn remove_sync(path: &Path, recursive: bool, force: bool) -> Result<(), std::io::Error> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            if meta.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_dir(path)
                }
            } else {
                std::fs::remove_file(path)
            }
        }
        Err(source) if force && source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(source),
    }
}

async fn remove_async(path: &Path, recursive: bool, force: bool) -> Result<(), std::io::Error> {
    match backend::stat(path).await {
        Ok(meta) => {
            if meta.is_dir() {
                if recursive {
                    tokio::fs::remove_dir_all(path).await
                } else {
                    tokio::fs::remove_dir(path).await
                }
            } else {
                tokio::fs::remove_file(path).await
            }
        }
        Err(source) if force && source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(source),
    }
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
    let path_buf = checked_path_async(&state, &path, PathCap::ReadFile)?;
    backend::read_file(&path_buf)
        .await
        .map_err(|source| RuntimeIoError::fs("open", path, source))
}

#[op2]
#[buffer]
pub fn op_node_read_file_sync(
    state: &mut OpState,
    #[string] path: String,
) -> Result<Vec<u8>, RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::ReadFile)?;
    std::fs::read(&path_buf).map_err(|source| RuntimeIoError::fs("open", path, source))
}

#[op2]
pub async fn op_node_write_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[buffer] data: JsBuffer,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_async(&state, &path, PathCap::WriteFile)?;
    backend::write_file(&path_buf, &data)
        .await
        .map_err(|source| RuntimeIoError::fs("open", path, source))
}

#[op2]
pub fn op_node_write_file_sync(
    state: &mut OpState,
    #[string] path: String,
    #[buffer] data: JsBuffer,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::WriteFile)?;
    std::fs::write(&path_buf, &data).map_err(|source| RuntimeIoError::fs("open", path, source))
}

#[op2]
#[serde]
pub async fn op_node_readdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<Vec<String>, RuntimeIoError> {
    let path_buf = checked_path_async(&state, &path, PathCap::ReadDir)?;
    backend::read_dir(&path_buf)
        .await
        .map_err(|source| RuntimeIoError::fs("scandir", path, source))
}

#[op2]
#[serde]
pub fn op_node_readdir_sync(
    state: &mut OpState,
    #[string] path: String,
) -> Result<Vec<String>, RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::ReadDir)?;
    let mut entries = std::fs::read_dir(&path_buf)
        .map_err(|source| RuntimeIoError::fs("scandir", path.clone(), source))?
        .map(|entry| entry.map(|dir| dir.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| RuntimeIoError::fs("scandir", path, source))?;
    entries.sort();
    Ok(entries)
}

#[op2]
#[serde]
pub async fn op_node_stat(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<NodeStat, RuntimeIoError> {
    let path_buf = checked_path_async(&state, &path, PathCap::Stat)?;
    backend::stat(&path_buf)
        .await
        .map(stat_from)
        .map_err(|source| RuntimeIoError::fs("stat", path, source))
}

#[op2]
#[serde]
pub fn op_node_stat_sync(
    state: &mut OpState,
    #[string] path: String,
) -> Result<NodeStat, RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::Stat)?;
    std::fs::metadata(&path_buf)
        .map(stat_from)
        .map_err(|source| RuntimeIoError::fs("stat", path, source))
}

#[op2]
pub async fn op_node_mkdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    recursive: bool,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_async(&state, &path, PathCap::Mkdir)?;
    backend::mkdir(&path_buf, recursive)
        .await
        .map_err(|source| RuntimeIoError::fs("mkdir", path, source))
}

#[op2(fast)]
pub fn op_node_mkdir_sync(
    state: &mut OpState,
    #[string] path: String,
    recursive: bool,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::Mkdir)?;
    if recursive {
        std::fs::create_dir_all(&path_buf)
    } else {
        std::fs::create_dir(&path_buf)
    }
    .map_err(|source| RuntimeIoError::fs("mkdir", path, source))
}

#[op2]
pub async fn op_node_access(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] mode: u32,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_async(&state, &path, PathCap::Access)?;
    let meta = backend::stat(&path_buf)
        .await
        .map_err(|source| RuntimeIoError::fs("access", path.clone(), source))?;
    check_access_mode(&meta, mode).map_err(|source| RuntimeIoError::fs("access", path, source))
}

#[op2(fast)]
pub fn op_node_access_sync(
    state: &mut OpState,
    #[string] path: String,
    #[smi] mode: u32,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::Access)?;
    let meta = std::fs::metadata(&path_buf)
        .map_err(|source| RuntimeIoError::fs("access", path.clone(), source))?;
    check_access_mode(&meta, mode).map_err(|source| RuntimeIoError::fs("access", path, source))
}

#[op2]
pub async fn op_node_rm(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    recursive: bool,
    force: bool,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_async(&state, &path, PathCap::Remove)?;
    remove_async(&path_buf, recursive, force)
        .await
        .map_err(|source| RuntimeIoError::fs("rm", path, source))
}

#[op2(fast)]
pub fn op_node_rm_sync(
    state: &mut OpState,
    #[string] path: String,
    recursive: bool,
    force: bool,
) -> Result<(), RuntimeIoError> {
    let path_buf = checked_path_sync(state, &path, PathCap::Remove)?;
    remove_sync(&path_buf, recursive, force)
        .map_err(|source| RuntimeIoError::fs("rm", path, source))
}

#[op2(fast)]
pub fn op_node_exists_sync(state: &mut OpState, #[string] path: &str) -> bool {
    let path_buf = match checked_path_sync(state, path, PathCap::Stat) {
        Ok(path_buf) => path_buf,
        Err(_) => return false,
    };
    std::fs::metadata(path_buf).is_ok()
}

#[op2]
#[string]
pub fn op_node_cwd(state: &mut OpState) -> Result<String, RuntimeIoError> {
    capabilities_sync(state).check(&CapRequest::CurrentDir)?;
    std::env::current_dir()
        .map(|cwd| cwd.to_string_lossy().into_owned())
        .map_err(RuntimeIoError::cwd)
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
        .map_err(|source| RuntimeIoError::connect(addr, source))?;
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
    fn name(&self) -> Cow<'_, str> {
        "tcpStream".into()
    }
}
