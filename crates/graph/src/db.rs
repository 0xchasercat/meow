//! The incremental store behind `GraphDb`.
//!
//! ## Why hand-built and not salsa
//!
//! GRAPH-001's sketch reaches for salsa, and that remains the right *eventual*
//! engine. But salsa stores memoized values inside a `Send + Sync` database and
//! requires every tracked value to be `Send + Sync + salsa::Update`. Oxc's stage
//! artifacts are the opposite: `Program`/`Semantic` are arena-allocated, borrow
//! from a bump `Allocator`, and are `!Send`/`!Sync`/`!Clone`/`!Update`. Storing
//! them in salsa means either fighting those bounds indefinitely (the warned-about
//! churn) or not memoizing the heavy artifacts at all.
//!
//! So the incremental core is a minimal, explicit memo layer — per-file slots whose
//! stage results are filled lazily ([`OnceCell`]) and evicted wholesale when the
//! file's input changes (`set_file`/`remove_file` take `&mut self`). Because the
//! P0 dependency shape is single-file (no module resolution — that is LOAD's stage
//! 4), per-file invalidation is exact: editing one file recomputes only its stages
//! and serves every other file from memo. Everything here is `pub(crate)` and lives
//! behind `GraphDb`, so swapping in salsa later is a contained change, not a
//! cross-crate break.

use std::cell::{Cell, OnceCell};
use std::rc::Rc;
use std::sync::Arc;

use oxc_span::SourceType;
use rustc_hash::FxHashMap;

use crate::cst::Cst;
use crate::ids::FileId;
use crate::ir::IrOutcome;
use crate::semantic::SemanticGraph;

/// One file's input + lazily-memoized stage results. Replacing the slot (on
/// `set_file`) drops the `OnceCell`s, which is exactly the invalidation step.
pub(crate) struct FileSlot {
    pub(crate) text: Arc<str>,
    pub(crate) source_type: SourceType,
    pub(crate) cst: OnceCell<Rc<Cst>>,
    pub(crate) semantic: OnceCell<SemanticGraph>,
    pub(crate) ir: OnceCell<IrOutcome>,
}

impl FileSlot {
    pub(crate) fn new(text: Arc<str>, source_type: SourceType) -> Self {
        Self {
            text,
            source_type,
            cst: OnceCell::new(),
            semantic: OnceCell::new(),
            ir: OnceCell::new(),
        }
    }
}

#[derive(Default)]
pub(crate) struct FileStore {
    pub(crate) slots: FxHashMap<FileId, FileSlot>,
}

/// Per-stage recompute counters — the incrementality probe. Each counter is bumped
/// exactly once per real (re)computation, since the `OnceCell` init closure runs
/// only on a memo miss.
#[derive(Default)]
pub(crate) struct RecomputeCounters {
    pub(crate) cst: Cell<u64>,
    pub(crate) semantic: Cell<u64>,
    pub(crate) runtime_ir: Cell<u64>,
}

impl RecomputeCounters {
    pub(crate) fn bump(counter: &Cell<u64>) {
        counter.set(counter.get() + 1);
    }
}
