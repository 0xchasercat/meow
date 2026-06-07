//! File identity: a stable, interned `FileId` and the path interner behind it.
//!
//! `FileId` is the canonical key on the hot path (typed, `Copy`, O(1)) — not a
//! re-hashed `PathBuf` per query. Path-keyed convenience lives on `GraphDb`.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

/// Stable, interned file identity. Public; never exposes an internal handle.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileId(u32);

impl FileId {
    pub(crate) fn from_index(index: usize) -> Self {
        FileId(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

/// Bidirectional path <-> `FileId` interner. Ids are never reused, so a `FileId`
/// minted by `set_file` stays meaningful even after the path is forgotten.
#[derive(Default)]
pub(crate) struct PathInterner {
    ids: FxHashMap<PathBuf, FileId>,
    paths: Vec<Option<PathBuf>>,
}

impl PathInterner {
    /// Return the existing id for `path`, or mint a fresh one.
    pub(crate) fn intern(&mut self, path: &Path) -> FileId {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        let id = FileId::from_index(self.paths.len());
        self.paths.push(Some(path.to_path_buf()));
        self.ids.insert(path.to_path_buf(), id);
        id
    }

    pub(crate) fn id_of(&self, path: &Path) -> Option<FileId> {
        self.ids.get(path).copied()
    }

    pub(crate) fn path(&self, id: FileId) -> Option<&Path> {
        self.paths.get(id.index()).and_then(|p| p.as_deref())
    }

    /// Forget the path -> id binding (used by `remove_file`). The id is retired,
    /// not recycled. Returns the forgotten id, if any.
    pub(crate) fn forget(&mut self, path: &Path) -> Option<FileId> {
        let id = self.ids.remove(path)?;
        if let Some(slot) = self.paths.get_mut(id.index()) {
            *slot = None;
        }
        Some(id)
    }
}
