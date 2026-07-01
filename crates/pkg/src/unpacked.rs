use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{archive, tmp_path, Cache, ContentHash, MaterializeError};

const MARKER_NAME: &str = ".unpacked";

/// Global content-addressed unpacked package store.
///
/// Archives still live in [`Cache`]; this store projects a verified blob onto disk
/// once at `<root>/<algo>-<hex>/` so editor tooling can traverse real files
/// without a project-local `node_modules` tree.
#[derive(Clone)]
pub struct UnpackedStore {
    root: PathBuf,
    cache: Arc<Cache>,
}

impl UnpackedStore {
    pub fn new(root: PathBuf, cache: Arc<Cache>) -> UnpackedStore {
        UnpackedStore { root, cache }
    }

    pub fn in_home(home: impl AsRef<Path>, cache: Arc<Cache>) -> UnpackedStore {
        UnpackedStore::new(
            home.as_ref().join(".meow").join("cache").join("unpacked"),
            cache,
        )
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir_for(&self, integrity: &ContentHash) -> PathBuf {
        self.root.join(integrity.to_url_host())
    }

    pub async fn ensure_async(&self, integrity: &ContentHash) -> Result<PathBuf, MaterializeError> {
        let dir = self.dir_for(integrity);
        if marker_path(&dir).is_file() {
            return Ok(dir);
        }

        fs::create_dir_all(&self.root)
            .map_err(|source| MaterializeError::io(&self.root, source))?;
        if dir.exists() {
            remove_best_effort(&dir);
        }

        let bytes = self
            .cache
            .read(integrity)
            .map_err(|source| MaterializeError::CacheBlob {
                hash: integrity.to_sri(),
                source,
            })?;

        let tmp = tmp_path(&dir);
        remove_best_effort(&tmp);
        let tmp_for_unpack = tmp.clone();
        let extracted = tokio::task::spawn_blocking(move || {
            archive::unpack_to(&bytes, &tmp_for_unpack)?;
            let marker = marker_path(&tmp_for_unpack);
            fs::write(&marker, []).map_err(|source| MaterializeError::io(&marker, source))?;
            Ok::<(), MaterializeError>(())
        })
        .await
        .map_err(|err| MaterializeError::blocking_task("unpack package", err))?;
        if let Err(err) = extracted {
            remove_best_effort(&tmp);
            return Err(err);
        }

        match fs::rename(&tmp, &dir) {
            Ok(()) => Ok(dir),
            Err(_source) if marker_path(&dir).is_file() => {
                remove_best_effort(&tmp);
                Ok(dir)
            }
            Err(source) => {
                remove_best_effort(&tmp);
                Err(MaterializeError::io(&dir, source))
            }
        }
    }

    pub fn ensure(&self, integrity: &ContentHash) -> Result<PathBuf, MaterializeError> {
        let dir = self.dir_for(integrity);
        if marker_path(&dir).is_file() {
            return Ok(dir);
        }

        fs::create_dir_all(&self.root)
            .map_err(|source| MaterializeError::io(&self.root, source))?;
        if dir.exists() {
            remove_best_effort(&dir);
        }

        let bytes = self
            .cache
            .read(integrity)
            .map_err(|source| MaterializeError::CacheBlob {
                hash: integrity.to_sri(),
                source,
            })?;

        let tmp = tmp_path(&dir);
        remove_best_effort(&tmp);
        let extracted = (|| {
            archive::unpack_to(&bytes, &tmp)?;
            let marker = marker_path(&tmp);
            fs::write(&marker, []).map_err(|source| MaterializeError::io(&marker, source))?;
            Ok::<(), MaterializeError>(())
        })();
        if let Err(err) = extracted {
            remove_best_effort(&tmp);
            return Err(err);
        }

        match fs::rename(&tmp, &dir) {
            Ok(()) => Ok(dir),
            Err(_source) if marker_path(&dir).is_file() => {
                remove_best_effort(&tmp);
                Ok(dir)
            }
            Err(source) => {
                remove_best_effort(&tmp);
                Err(MaterializeError::io(&dir, source))
            }
        }
    }
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER_NAME)
}

fn remove_best_effort(path: &Path) {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp_dir;
    use crate::{Cache, CacheError, ContentHash};

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("package/{path}"), *bytes)
                .expect("append tar member");
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    #[test]
    fn dir_for_is_stable() {
        let root = unique_tmp_dir("unpacked-dir-for");
        let cache = Arc::new(Cache::with_root(root.join("cache")));
        let store = UnpackedStore::new(root.join("unpacked"), cache);
        let hash = ContentHash::of(b"stable dir");

        assert_eq!(store.dir_for(&hash), store.root().join(hash.to_url_host()));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ensure_extracts_once_and_reuses_marker_dir() {
        let root = unique_tmp_dir("unpacked-once");
        let cache = Arc::new(Cache::with_root(root.join("cache")));
        let store = UnpackedStore::new(root.join("unpacked"), cache.clone());
        let hash = cache
            .store(&archive(&[
                (
                    "package.json",
                    br#"{"name":"dep","version":"1.0.0","exports":"./index.js","type":"module"}"#,
                ),
                ("index.js", b"export const value = 1;\n"),
            ]))
            .expect("store dep");

        let first = store.ensure(&hash).expect("first ensure");
        assert!(first.join("package.json").is_file());
        let sentinel = first.join("sentinel.txt");
        fs::write(&sentinel, b"keep me").expect("write sentinel");

        let second = store.ensure(&hash).expect("second ensure");
        assert_eq!(first, second);
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"keep me");
        assert!(second.join(MARKER_NAME).is_file());

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tampered_blob_surfaces_cache_blob_and_leaves_dir_absent() {
        let root = unique_tmp_dir("unpacked-tampered");
        let cache = Arc::new(Cache::with_root(root.join("cache")));
        let store = UnpackedStore::new(root.join("unpacked"), cache.clone());
        let hash = cache
            .store(&archive(&[(
                "package.json",
                br#"{"name":"dep","version":"1.0.0"}"#,
            )]))
            .expect("store dep");
        fs::write(cache.path_for(&hash), b"tampered").expect("tamper blob");

        let err = store.ensure(&hash).expect_err("tampered blob errors");
        assert!(matches!(
            err,
            MaterializeError::CacheBlob {
                source: CacheError::IntegrityMismatch { .. },
                ..
            }
        ));
        assert!(!store.dir_for(&hash).exists());

        fs::remove_dir_all(root).ok();
    }
}
