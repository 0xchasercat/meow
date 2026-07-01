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
//! SEC-001: [`AllowAll`] is still the default for trusted runs (`meow run`),
//! preserving the pre-SEC-001 behaviour byte-for-byte. Sandboxed runs (`meow x`
//! by default, `meow run --sandbox`) install [`SandboxCaps`] instead, which is
//! real enforcement: reads everywhere, writes confined to policy roots, network
//! denied. This governs meow's own seam; the Node stack (deno_fs/deno_net/
//! deno_process) is gated by a matching restrictive `PermissionsContainer` built
//! from the same [`SandboxPolicy`] in `node.rs`.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Strip the Windows extended-length (verbatim) prefix from a path so it
/// matches the non-verbatim form the OS/Node present at I/O time. This is the
/// classic `std::fs::canonicalize` `\\?\` mismatch fix. Pure/lexical (I-6):
/// touches no filesystem and reads no ambient state. No-op on non-Windows.
#[cfg(windows)]
pub fn strip_windows_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    let s = path.as_os_str().to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return std::path::PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return std::path::PathBuf::from(rest);
    }
    path
}
#[cfg(not(windows))]
pub fn strip_windows_verbatim_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    path
}

/// A host-I/O request presented to the capability seam before the op acts.
/// Borrows its subject so the check allocates nothing on the hot path.
pub enum CapRequest<'a> {
    /// Read the file at this path.
    ReadFile(&'a Path),
    // === RT-007 ===
    /// Write the file at this path.
    WriteFile(&'a Path),
    /// Enumerate directory entries at this path.
    ReadDir(&'a Path),
    /// Stat this path.
    Stat(&'a Path),
    /// Create this directory.
    Mkdir(&'a Path),
    /// Check access to this path.
    Access(&'a Path),
    /// Remove this path.
    Remove(&'a Path),
    /// Read the current working directory.
    CurrentDir,
    // === /RT-007 ===
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

/// The single bypass one-liner every sandbox denial teaches, so friction always
/// arrives with its remedy (kept identical across messages for muscle memory).
pub const SANDBOX_BYPASS_HINT: &str =
    "re-run with --trust, or set MEOW_TRUST_ALL=1 to trust everything permanently";

/// Data describing a sandboxed run's host-access grants. Drives BOTH enforcement
/// seams: meow's [`CapabilityCheck`] (via [`SandboxCaps`], for the fetch gate and
/// meow-native io ops) and the deno_permissions `PermissionsContainer` the Node
/// stack consults (built from this in `node.rs`). Holds only owned `Send + Sync`
/// data so it can cross onto worker OS threads.
#[derive(Clone, Debug)]
pub struct SandboxPolicy {
    /// Absolute roots under which writes (create/write/remove/mkdir) are allowed.
    /// A write outside every root is denied. Should be canonicalized by the caller.
    pub write_roots: Vec<PathBuf>,
    /// Whether the network is permitted. `false` denies all connect/listen.
    pub allow_net: bool,
}

/// Real [`CapabilityCheck`] for sandboxed runs: reads are always allowed (exfil
/// requires the separately-gated network); writes are confined to the policy
/// roots; network is denied unless granted. Every denial names the subject and
/// the exact bypass ([`SANDBOX_BYPASS_HINT`]).
pub struct SandboxCaps {
    policy: SandboxPolicy,
}

impl SandboxCaps {
    pub fn new(policy: SandboxPolicy) -> Self {
        Self { policy }
    }

    /// Is a write to `path` inside the sandbox's writable roots? Pure/lexical —
    /// the seam must not touch the filesystem or read ambient host state (I-6).
    fn write_allowed(&self, path: &Path) -> bool {
        let normalized = lexically_normalize(path);
        if normalized.is_absolute() {
            let normalized = strip_windows_verbatim_prefix(normalized);
            self.policy.write_roots.iter().any(|root| {
                let root = strip_windows_verbatim_prefix(root.clone());
                normalized.starts_with(root)
            })
        } else {
            // Relative paths resolve against the process cwd, which is always a
            // write root for a sandboxed run; allow unless the normalized form
            // climbs above cwd (a leading `..`).
            !matches!(normalized.components().next(), Some(Component::ParentDir))
        }
    }
}

impl CapabilityCheck for SandboxCaps {
    fn check(&self, req: &CapRequest<'_>) -> Result<(), CapDenied> {
        match req {
            // Reads + metadata always allowed (exfiltration needs the network).
            CapRequest::ReadFile(_)
            | CapRequest::ReadDir(_)
            | CapRequest::Stat(_)
            | CapRequest::Access(_)
            | CapRequest::CurrentDir => Ok(()),
            CapRequest::WriteFile(path) | CapRequest::Mkdir(path) | CapRequest::Remove(path) => {
                if self.write_allowed(path) {
                    Ok(())
                } else {
                    Err(CapDenied(format!(
                        "write to {} denied by the meow sandbox ({SANDBOX_BYPASS_HINT})",
                        path.display()
                    )))
                }
            }
            CapRequest::NetConnect(target) => {
                if self.policy.allow_net {
                    Ok(())
                } else {
                    Err(CapDenied(format!(
                        "network access to {target} denied by the meow sandbox ({SANDBOX_BYPASS_HINT})"
                    )))
                }
            }
            CapRequest::NetListen(addr) => {
                if self.policy.allow_net {
                    Ok(())
                } else {
                    Err(CapDenied(format!(
                        "listening on {addr} denied by the meow sandbox ({SANDBOX_BYPASS_HINT})"
                    )))
                }
            }
        }
    }
}

/// Build the shared [`CapabilityCheck`] enforcing `policy`, as the `Arc` the
/// fetch gate ([`NetCaps`](crate::web::NetCaps)) and meow-native io ops consult.
pub fn sandbox_caps(policy: SandboxPolicy) -> Arc<dyn CapabilityCheck + Send + Sync> {
    Arc::new(SandboxCaps::new(policy))
}

/// Lexically normalize a path (drop `.`, resolve `..` against prior *normal*
/// components) WITHOUT touching the filesystem — the capability seam must not
/// read ambient host state (I-6). A leading `..` on a relative path is kept; a
/// `..` at a root/prefix is dropped (cannot climb above root).
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    use super::*;

    #[test]
    fn strips_windows_drive_verbatim_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\C:\a\b")),
            PathBuf::from(r"C:\a\b")
        );
    }

    #[test]
    fn strips_windows_unc_verbatim_prefix() {
        assert_eq!(
            strip_windows_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\x")),
            PathBuf::from(r"\\server\share\x")
        );
    }

    #[test]
    fn leaves_non_verbatim_windows_path_unchanged() {
        let path = PathBuf::from(r"C:\a\b");
        assert_eq!(strip_windows_verbatim_prefix(path.clone()), path);
    }

    #[test]
    fn sandbox_caps_accepts_verbatim_child_under_non_verbatim_root() {
        let caps = SandboxCaps::new(SandboxPolicy {
            write_roots: vec![PathBuf::from(r"C:\proj")],
            allow_net: false,
        });

        assert!(caps
            .check(&CapRequest::WriteFile(Path::new(r"\\?\C:\proj\inside.txt")))
            .is_ok());
    }
}
