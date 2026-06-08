//! The CLI's sanctioned **host-boundary seam** (P16 / CONSTITUTION I-6).
//!
//! The binary edge — and ONLY here — may read ambient host configuration. This is
//! the host equivalent of the runtime's single print op: one named place where the
//! process touches the environment. Library/subsystem crates never read the
//! environment; they receive already-resolved paths from this edge.
//!
//! Reading `$HOME` here locates the global content-addressed cache
//! (`~/.meow/cache`, CANON §12.1). It does **not** affect execution determinism
//! (I-6): the cache is content-addressed, so the *path* only changes where bytes
//! are read from, never *which* (hash-pinned) bytes are loaded.

use std::ffi::OsString;
use std::path::PathBuf;

/// The host home directory (to locate `~/.meow`), falling back to the cwd.
pub(crate) fn host_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Optional override for the reference TypeScript compiler path (`meow types`).
pub(crate) fn host_meow_tsc() -> Option<OsString> {
    std::env::var_os("MEOW_TSC")
}

/// Whether the user requested no ANSI color. Host-boundary read for UI-001.
pub(crate) fn host_no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}
