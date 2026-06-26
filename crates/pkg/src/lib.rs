//! `meow-pkg` — lockfile, cache, install resolution, and graph projections.
//!
//! The dependency subsystem has one runtime truth:
//!
//! - [`Lockfile`] / [`LockEntry`] — the `meow.lock.jsonl` format: strictly-sorted
//!   JSON-lines, one dependency per line, byte-stable across runs so git merges
//!   rarely conflict and reproducibility (I-6) holds (CANON §12.3).
//! - [`Cache`] — the global content-addressed store
//!   `~/.meow/cache/<algo>/<hash>`, which RECOMPUTES a blob's hash before any
//!   read and refuses to serve mismatched bytes (I-7, CANON §12.1).
//! - [`Installer`] / [`RegistrySource`] — package.json-declared dependencies are
//!   resolved against registry metadata, publisher sha512 integrity is verified,
//!   and verified tarballs are stored by content hash.
//! - [`ResolutionGraph`] — the validated resolved tree consumed by runtime,
//!   materialization, and editor tooling. [`UnpackedStore`] supplies stable real
//!   package directories; the standard `node_modules` projection is a strict
//!   symlink tree into that global unpacked store.
//!
//! All hashes/specifiers are newtypes ([`ContentHash`], [`PackageName`],
//! [`Version`], [`VersionReq`]) and every reachable failure is a typed
//! [`thiserror`] enum — no path here panics on malformed input (CRAFT Part B).

mod cache;
mod error;
mod hash;
mod install;
mod lockfile;
// === PKG-003 ===
mod pnp;
// === /PKG-003 ===
// === PKG-004 ===
mod archive;
mod materialize;
// === LSP-001 ===
mod unpacked;
// === /LSP-001 ===
// === /PKG-004 ===
mod registry;
use std::path::{Path, PathBuf};

pub use cache::Cache;
pub use error::{CacheError, LockError, ParseHashError, ParseVersionError};
pub use hash::{ContentHash, HashAlgo, PackageName, Version, VersionReq};
pub use install::{
    resolve_roots, InstallError, InstallProgress, Installer, ProgressPhase, RootResolveError,
};
// === PKG-003 ===
pub use pnp::{PnpError, ResolutionGraph, ResolvedPackage};
// === /PKG-003 ===
// === PKG-004 ===
pub use archive::{unpack_to, UnpackStats};
pub use materialize::{
    LinkStrategy, MaterializeError, MaterializeOptions, MaterializePlan, MaterializeReport,
    Materializer, Projection,
};
// === LSP-001 ===
pub use unpacked::UnpackedStore;
// === /LSP-001 ===
// === /PKG-004 ===
pub use lockfile::{CapabilityGrant, LockEntry, Lockfile, RegistryProvenance};
pub use registry::FixtureRegistry;
pub use registry::{
    DepSpec, DistInfo, PackageMetadata, RegistryError, RegistrySource, VersionMetadata,
};

/// Sibling temp path for an atomic write: write `<path>.tmp`, then `rename` it
/// onto `<path>`. Shared by the lockfile writer and the cache store.
pub(crate) fn tmp_path(path: &Path) -> PathBuf {
    let mut buf = path.as_os_str().to_owned();
    buf.push(".tmp");
    PathBuf::from(buf)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A fresh, unique temp directory for a test, created eagerly. Uniqueness
    /// comes from the process id plus a monotonic counter — no `rand`, no clock
    /// (the P16 grep bans both even in tests); `std::env::temp_dir` is permitted.
    pub(crate) fn unique_tmp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut dir = std::env::temp_dir();
        dir.push(format!("meow-pkg-test-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir for test");
        dir
    }
}
