//! Global content-addressed blob cache (`~/.meow/cache/<algo>/<hash>`, CANON §12.1).
//!
//! The defining invariant is I-7: a read RECOMPUTES the stored blob's hash and
//! refuses to return bytes that do not match — corrupt or tampered content is an
//! error, never served. Stores are atomic (temp file + `rename`) so a crash
//! cannot leave a partial blob that a later read would accept.
//!
//! The root is injectable ([`Cache::with_root`]) so callers — and tests — never
//! touch the real `~/.meow`. The library never reads `$HOME` ambiently (P16 / no
//! ambient host reads); a caller that wants the conventional location passes the
//! home directory explicitly via [`Cache::in_home`].

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CacheError;
use crate::hash::ContentHash;
use crate::tmp_path;

/// A content-addressed blob store rooted at a single directory.
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Use an explicit cache root (tests, vendored caches, or a host-resolved
    /// path). The root is created lazily on first [`store`](Cache::store).
    pub fn with_root(root: impl Into<PathBuf>) -> Cache {
        Cache { root: root.into() }
    }

    /// Cache rooted at `<home>/.meow/cache` (CANON §12.1). The caller supplies
    /// the home directory explicitly — the library never reads `$HOME` itself
    /// (P16). The CLI edge is responsible for resolving the host home.
    pub fn in_home(home: impl AsRef<Path>) -> Cache {
        let mut root = home.as_ref().to_path_buf();
        root.push(".meow");
        root.push("cache");
        Cache { root }
    }

    /// The cache root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path a blob lives at: `<root>/<algo>/<lowerhex-digest>`.
    pub fn path_for(&self, hash: &ContentHash) -> PathBuf {
        self.root.join(hash.cache_subpath())
    }

    /// Whether a blob file exists at the expected path. Does NOT verify
    /// integrity — that is enforced by [`read`](Cache::read).
    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.path_for(hash).is_file()
    }

    /// Hash `bytes`, write the blob atomically, and return its [`ContentHash`].
    ///
    /// A valid existing blob is a no-op (its bytes are re-hashed + confirmed); a
    /// corrupt/tampered blob is REPAIRED by re-writing the known-good bytes (I-7,
    /// the cache self-heals). The write goes to a sibling temp file then `rename`s
    /// into place, so a crashed store never leaves a partial blob.
    pub fn store(&self, bytes: &[u8]) -> Result<ContentHash, CacheError> {
        let hash = ContentHash::of(bytes);
        let dest = self.path_for(&hash);
        // A hit ONLY if the existing bytes still hash to `hash`; a corrupt or
        // truncated blob falls through and is repaired by re-writing below (I-7).
        if let Ok(existing) = fs::read(&dest) {
            if ContentHash::of(&existing) == hash {
                return Ok(hash);
            }
        }

        // The blob path is always `<root>/<algo>/<hex>`, so a parent exists;
        // fall back to the root rather than unwrapping.
        let dir = dest.parent().unwrap_or(&self.root);
        fs::create_dir_all(dir).map_err(|source| CacheError::Io {
            path: dir.to_path_buf(),
            source,
        })?;

        let tmp = tmp_path(&dest);
        fs::write(&tmp, bytes).map_err(|source| CacheError::Io {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, &dest).map_err(|source| CacheError::Io {
            path: dest.clone(),
            source,
        })?;
        Ok(hash)
    }

    /// Read the blob for `hash`, RECOMPUTE its hash, and compare before
    /// returning (I-7).
    ///
    /// - missing blob → [`CacheError::NotFound`]
    /// - on-disk bytes hash to something else → [`CacheError::IntegrityMismatch`]
    ///   and the bytes are dropped — refuse, never serve.
    pub fn read(&self, hash: &ContentHash) -> Result<Vec<u8>, CacheError> {
        let path = self.path_for(hash);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(CacheError::NotFound(hash.to_sri()));
            }
            Err(source) => return Err(CacheError::Io { path, source }),
        };

        let got = ContentHash::of(&bytes);
        if &got != hash {
            return Err(CacheError::IntegrityMismatch {
                expected: hash.to_sri(),
                got: got.to_sri(),
            });
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp_dir;

    #[test]
    fn store_then_read_round_trips() {
        let dir = unique_tmp_dir("cache-rt");
        let cache = Cache::with_root(&dir);
        let blob = b"the quick brown fox";

        let h = cache.store(blob).expect("store");
        assert!(cache.contains(&h));
        assert_eq!(cache.read(&h).expect("read"), blob);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_is_idempotent() {
        let dir = unique_tmp_dir("cache-idem");
        let cache = Cache::with_root(&dir);
        let blob = b"idempotent payload";

        let h1 = cache.store(blob).expect("store 1");
        let path = cache.path_for(&h1);
        let bytes1 = fs::read(&path).expect("first on-disk");
        let h2 = cache.store(blob).expect("store 2");
        let bytes2 = fs::read(&path).expect("second on-disk");

        assert_eq!(h1, h2);
        assert_eq!(bytes1, bytes2);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_refuses_tampered_blob() {
        let dir = unique_tmp_dir("cache-tamper");
        let cache = Cache::with_root(&dir);
        let blob = b"trusted contents";

        let h = cache.store(blob).expect("store");
        // Tamper: overwrite the on-disk blob (same length) with other bytes.
        let path = cache.path_for(&h);
        fs::write(&path, b"evil contents!!!").expect("tamper");

        let err = cache.read(&h).unwrap_err();
        match err {
            CacheError::IntegrityMismatch { expected, got } => {
                assert_eq!(expected, h.to_sri());
                assert_ne!(got, h.to_sri());
            }
            other => panic!("expected IntegrityMismatch, got {other:?}"),
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_refuses_truncated_blob() {
        let dir = unique_tmp_dir("cache-trunc");
        let cache = Cache::with_root(&dir);
        let h = cache.store(b"some longer payload of bytes").expect("store");
        fs::write(cache.path_for(&h), b"short").expect("truncate");

        assert!(matches!(
            cache.read(&h).unwrap_err(),
            CacheError::IntegrityMismatch { .. }
        ));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_missing_blob_is_not_found() {
        let dir = unique_tmp_dir("cache-missing");
        let cache = Cache::with_root(&dir);
        let h = ContentHash::of(b"never stored");

        assert!(matches!(
            cache.read(&h).unwrap_err(),
            CacheError::NotFound(_)
        ));
        assert!(!cache.contains(&h));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn in_home_appends_meow_cache() {
        let cache = Cache::in_home("/home/u");
        assert!(cache.root().ends_with(".meow/cache"));
    }

    #[test]
    fn store_repairs_a_corrupt_blob() {
        let dir = unique_tmp_dir("cache-repair");
        let cache = Cache::with_root(&dir);
        let blob = b"trusted contents";

        let h = cache.store(blob).expect("store");
        // Corrupt the on-disk blob so a read refuses it.
        fs::write(cache.path_for(&h), b"evil contents!!!").expect("corrupt");
        assert!(matches!(
            cache.read(&h).unwrap_err(),
            CacheError::IntegrityMismatch { .. }
        ));

        // Re-storing the known-good bytes repairs the entry (I-7 self-heal).
        let h2 = cache.store(blob).expect("re-store");
        assert_eq!(h, h2);
        assert_eq!(cache.read(&h).expect("read after repair"), blob);

        fs::remove_dir_all(&dir).ok();
    }
}
