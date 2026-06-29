//! Stage 2 — Semantic Graph (scopes / bindings / symbols / references).
//!
//! `oxc_semantic::Semantic<'a>` borrows from the AST, which lives in the `Cst`'s
//! arena. We keep an `Rc<Cst>` as the owner and wrap the borrowed `Semantic` in a
//! `self_cell`, so the semantic graph is a self-contained owned value.

use std::rc::Rc;

use oxc_diagnostics::OxcDiagnostic;
use oxc_semantic::{AstNodes, Scoping, Semantic, SemanticBuilder};

use crate::cst::Cst;

struct SemDep<'a> {
    semantic: Semantic<'a>,
    errors: Vec<OxcDiagnostic>,
}

self_cell::self_cell!(
    struct SemanticCell {
        owner: Rc<Cst>,
        #[covariant]
        dependent: SemDep,
    }
);

/// Scope tree, symbol table (bindings + references), and AST nodes for one file.
pub struct SemanticGraph {
    cell: SemanticCell,
}

impl SemanticGraph {
    /// Stage-2 query body: run `SemanticBuilder` over an owned `Rc<Cst>`.
    pub(crate) fn build(cst: Rc<Cst>) -> Self {
        let cell = SemanticCell::new(cst, |cst| {
            let ret = SemanticBuilder::new().build(cst.program());
            SemDep {
                semantic: ret.semantic,
                errors: ret.diagnostics,
            }
        });
        Self { cell }
    }

    /// Scopes + symbols + references (Oxc 0.134 unifies these in `Scoping`).
    pub fn scoping(&self) -> &Scoping {
        self.cell.borrow_dependent().semantic.scoping()
    }

    /// Nodes in the AST, indexable by the ids carried in `scoping()`.
    pub fn nodes(&self) -> &AstNodes<'_> {
        self.cell.borrow_dependent().semantic.nodes()
    }

    /// The retained verbatim source of the underlying file.
    pub fn source(&self) -> &str {
        self.cell.borrow_owner().source()
    }

    /// Semantic-analysis diagnostics (separate from parse errors).
    pub fn errors(&self) -> &[OxcDiagnostic] {
        &self.cell.borrow_dependent().errors
    }
}
