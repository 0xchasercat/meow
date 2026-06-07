//! `meow-graph` — the Oxc parse pipeline as an incremental query system.
//!
//! This crate is the single parser entrypoint for the whole workspace (I-1, P15):
//! every other crate consumes only [`GraphDb`] and never constructs an Oxc parser
//! or allocator. It exposes pipeline stages as memoized, invalidatable queries:
//!
//! - stage 1 — [`Cst`]: lossless syntax tree (Oxc AST + retained verbatim source +
//!   spans + comment trivia); [`Cst::write_source`] round-trips byte-for-byte.
//! - stage 2 — [`SemanticGraph`]: scopes, bindings, symbols, references.
//! - stage 5 — [`RuntimeIr`]: the type-erased lowering, behind the RT-003 strip seam.
//!
//! Salsa, `self_cell`, and every lifetime-bearing Oxc type are private to this
//! crate; the public surface is exactly the items re-exported below.
//!
//! ## Mutation -> read cadence
//!
//! `set_file`/`remove_file` take `&mut self`; the stage queries take `&self` and
//! return references valid until the next `&mut self`. Batch edits, then read
//! (the rust-analyzer cadence). The incremental core is documented in [`db`].

mod cst;
mod db;
mod error;
mod ids;
mod ir;
mod semantic;
mod strip;

pub use crate::cst::Cst;
pub use crate::error::{GraphError, StripDiagnostic};
pub use crate::ids::FileId;
pub use crate::ir::RuntimeIr;
pub use crate::semantic::SemanticGraph;
pub use crate::strip::{ErasablePolicy, PermissivePolicy, StripPolicy};

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use oxc_span::SourceType;

use crate::db::{FileSlot, FileStore, RecomputeCounters};
use crate::ids::PathInterner;
use crate::ir::IrOutcome;

/// Snapshot of per-stage recompute counts — the incrementality probe. Each field
/// counts real (re)computations performed since the db was created; memo hits do
/// not advance it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recomputes {
    pub cst: u64,
    pub semantic: u64,
    pub runtime_ir: u64,
}

/// The incremental query facade over Oxc. The entire public contract.
pub struct GraphDb {
    interner: PathInterner,
    store: FileStore,
    policy: Arc<dyn StripPolicy>,
    recomputes: RecomputeCounters,
}

impl Default for GraphDb {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphDb {
    /// New db with the erasable-only strip policy installed — the policy `meow`
    /// ships (I-3). Use [`with_policy`](Self::with_policy) to install another.
    pub fn new() -> Self {
        Self::with_policy(Arc::new(ErasablePolicy))
    }

    /// New db with a caller-supplied [`StripPolicy`] (RT-003 installs the real one).
    pub fn with_policy(policy: Arc<dyn StripPolicy>) -> Self {
        Self {
            interner: PathInterner::default(),
            store: FileStore::default(),
            policy,
            recomputes: RecomputeCounters::default(),
        }
    }

    /// Insert or update a file's text. Invalidates ONLY this file's stage queries
    /// (its slot, and thus its memos, is replaced); every other file is untouched.
    /// Returns the file's stable id.
    pub fn set_file(&mut self, path: impl Into<PathBuf>, text: Arc<str>) -> FileId {
        let path = path.into();
        // Never panics on a bad/unknown extension — fall back to the default type.
        let source_type = SourceType::from_path(&path).unwrap_or_default();
        let id = self.interner.intern(&path);
        self.store
            .slots
            .insert(id, FileSlot::new(text, source_type));
        id
    }

    /// Remove a file, invalidating its memos. Returns whether it existed.
    pub fn remove_file(&mut self, path: &Path) -> bool {
        match self.interner.forget(path) {
            Some(id) => {
                self.store.slots.remove(&id);
                true
            }
            None => false,
        }
    }

    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.interner.id_of(path)
    }

    pub fn path(&self, file: FileId) -> Option<&Path> {
        self.interner.path(file)
    }

    // ---- stage queries: memoized; the borrow is valid until the next `&mut self` ----
    //
    // All keyed by `FileId`. An unknown/retired id (e.g. held across file-watch
    // churn after `remove_file`) yields `None` — never a panic. The path-keyed
    // `*_of` wrappers compose on top.

    /// Stage 1. `None` if the id is unknown/retired; otherwise always a `Cst`
    /// (Oxc is error-recovering).
    pub fn cst(&self, file: FileId) -> Option<&Cst> {
        Some(self.cst_in(self.slot(file)?))
    }

    /// Stage 2. `None` if the id is unknown/retired.
    pub fn semantic(&self, file: FileId) -> Option<&SemanticGraph> {
        Some(self.semantic_in(self.slot(file)?))
    }

    /// Stage 5. `None` = unknown/retired file; `Some(Ok)` = the runtime IR;
    /// `Some(Err)` = the lowering diagnostics (parse/semantic errors, a strip-policy
    /// rejection, or the not-yet-available type strip).
    pub fn runtime_ir(&self, file: FileId) -> Option<Result<&RuntimeIr, &[StripDiagnostic]>> {
        let outcome = self.ir_outcome(self.slot(file)?);
        Some(match &outcome.ir {
            Some(ir) => Ok(ir),
            None => Err(&outcome.diagnostics),
        })
    }

    // ---- path-keyed convenience (intern, then dispatch by FileId) ----

    pub fn cst_of(&self, path: &Path) -> Option<&Cst> {
        self.cst(self.file_id(path)?)
    }

    pub fn semantic_of(&self, path: &Path) -> Option<&SemanticGraph> {
        self.semantic(self.file_id(path)?)
    }

    pub fn runtime_ir_of(&self, path: &Path) -> Option<Result<&RuntimeIr, &[StripDiagnostic]>> {
        self.runtime_ir(self.file_id(path)?)
    }

    /// The incrementality probe (see [`Recomputes`]).
    pub fn recomputes(&self) -> Recomputes {
        Recomputes {
            cst: self.recomputes.cst.get(),
            semantic: self.recomputes.semantic.get(),
            runtime_ir: self.recomputes.runtime_ir.get(),
        }
    }

    // ---- internals ----

    /// Resolve a `FileId` to its slot. A retired/forged id returns `None` rather
    /// than panicking: ids outlive the files they name (`remove_file` retires them,
    /// `set_file` mints fresh ones), so a stale id is ordinary input, not a bug.
    fn slot(&self, file: FileId) -> Option<&FileSlot> {
        self.store.slots.get(&file)
    }

    /// Stage-1 memo, keyed off an already-resolved slot (no second lookup).
    fn cst_in<'a>(&self, slot: &'a FileSlot) -> &'a Cst {
        self.cst_handle(slot)
    }

    fn cst_handle<'a>(&self, slot: &'a FileSlot) -> &'a Rc<Cst> {
        slot.cst.get_or_init(|| {
            RecomputeCounters::bump(&self.recomputes.cst);
            Rc::new(cst::compute_cst(slot.text.clone(), slot.source_type))
        })
    }

    /// Stage-2 memo, keyed off the slot.
    fn semantic_in<'a>(&self, slot: &'a FileSlot) -> &'a SemanticGraph {
        slot.semantic.get_or_init(|| {
            RecomputeCounters::bump(&self.recomputes.semantic);
            // tracked read of stage 1 -> dependency edge
            let cst = self.cst_handle(slot).clone();
            SemanticGraph::build(cst)
        })
    }

    /// Stage-5 memo, keyed off the slot.
    fn ir_outcome<'a>(&self, slot: &'a FileSlot) -> &'a IrOutcome {
        slot.ir.get_or_init(|| {
            RecomputeCounters::bump(&self.recomputes.runtime_ir);
            // tracked reads of stages 1 + 2 -> dependency edges
            let cst = self.cst_handle(slot);
            let sem = self.semantic_in(slot);
            ir::compute_runtime_ir(self.policy.as_ref(), cst, sem, slot.text.clone())
        })
    }
}
