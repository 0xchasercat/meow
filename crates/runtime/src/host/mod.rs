//! The runtime's host-access edge (RT-005).
//!
//! The ambient host reads needed to locate the delegated TypeScript compiler
//! (`meow types`, ADR-5/I-4) live here — the one sanctioned home for env reads in
//! this crate outside `hermetic/` (`principles-check.sh` P16 + the `runtime.rs`
//! floor test allowlist `/host/`). Determinism (I-6) is unaffected: these feed a
//! dev/CI build tool (declaration emit), never the runtime's execution of user JS.

use std::ffi::OsString;
use std::path::PathBuf;

/// Host home directory — used only to locate the npx TypeScript cache for typegen.
/// Falls back to `.` when `HOME` is unset (the locator then simply finds no cache).
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The `MEOW_TSC` override (an explicit `tsc` path), if set — the highest-priority
/// way to point `meow types` at a TypeScript compiler.
pub fn meow_tsc() -> Option<OsString> {
    std::env::var_os("MEOW_TSC")
}
