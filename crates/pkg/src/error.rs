//! Typed, causal errors for the lockfile + cache (CRAFT Part B: errors are
//! `thiserror`, carry their cause, and point at the fix). No path in this crate
//! panics on malformed input — every reachable failure is one of these.

use std::path::PathBuf;

/// Failure parsing an SRI content-hash string (`<algo>-<base64>`).
#[derive(thiserror::Error, Debug)]
pub enum ParseHashError {
    /// The algorithm prefix is not one this build understands.
    #[error("unknown hash algorithm in SRI: {0:?}")]
    UnknownAlgo(String),
    /// The string is not shaped like `<algo>-<base64>`.
    #[error("malformed SRI (expected `<algo>-<base64>`): {0:?}")]
    Malformed(String),
    /// The digest body is not valid standard base64.
    #[error("bad base64 in SRI digest")]
    BadBase64,
    /// The decoded digest is the wrong length for the named algorithm.
    #[error("digest length {got} != expected {expected} for {algo}")]
    BadLength {
        algo: &'static str,
        expected: usize,
        got: usize,
    },
}

/// Failure parsing a semver version (`version`) or requirement (`meow`) field.
#[derive(thiserror::Error, Debug)]
pub enum ParseVersionError {
    /// An exact `version` field was not valid semver.
    #[error("invalid exact version {value:?}: {reason}")]
    Version { value: String, reason: String },
    /// A version-requirement field (`meow`) was not a valid semver range.
    #[error("invalid version requirement {value:?}: {reason}")]
    Req { value: String, reason: String },
}

/// Failure reading, parsing, or writing a `meow.lock.jsonl`.
#[derive(thiserror::Error, Debug)]
pub enum LockError {
    /// I/O against the lockfile path (read or atomic write).
    #[error("lockfile I/O at {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A line was not valid JSON for a [`crate::LockEntry`].
    #[error("lockfile line {line} is invalid JSON")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    /// A line is well-formed JSON but violates canonical form: not strictly
    /// ascending by `(name, version)`, a duplicate, or non-canonical key
    /// order / whitespace. A non-canonical lockfile is a defect, not silently
    /// re-sorted (I-7).
    #[error("lockfile line {line} is not canonical: {reason}")]
    NotCanonical { line: usize, reason: String },
    /// A line's `integrity` field is not a parseable SRI content-hash. Carries
    /// the line so the diagnostic points at the fix (vs a generic "invalid JSON").
    #[error("lockfile line {line}: invalid integrity SRI")]
    HashField {
        line: usize,
        #[source]
        source: ParseHashError,
    },
    /// A line's `version`, `meow`, or a dependency value is not valid semver.
    #[error("lockfile line {line}: invalid version field")]
    VersionField {
        line: usize,
        #[source]
        source: ParseVersionError,
    },
}

/// Failure storing to or reading from the content-addressed cache.
#[derive(thiserror::Error, Debug)]
pub enum CacheError {
    /// I/O against a blob path (read, atomic write, or `mkdir`).
    #[error("cache I/O at {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// No blob exists for the requested hash. Carries its SRI.
    #[error("blob not found in cache: {0}")]
    NotFound(String),
    /// The stored bytes do not hash to the requested hash — refuse, never
    /// serve tampered/corrupt content (I-7 / `lockfile-integrity` gate).
    #[error("integrity check FAILED: expected {expected}, recomputed {got}")]
    IntegrityMismatch { expected: String, got: String },
}
