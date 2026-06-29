//! Stage 1 — Lossless Syntax Tree (CST).
//!
//! Oxc's AST borrows from a bump [`Allocator`] and the source `&str`, so the CST
//! is a self-referential owner built with `self_cell` (no hand-written `unsafe`).
//!
//! "Lossless" here is Option A (CANON §7, GRAPH-001 operator notes): the verbatim
//! source `Arc<str>` is retained alongside the Oxc AST and full-fidelity spans +
//! comment trivia. Every byte is reconstructable — proven by the round-trip test —
//! without building a second green tree. A structural re-serializer (formatter) is
//! a separate, additive stage, not part of P0 losslessness.

use std::sync::Arc;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_ast::Comment;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::Parser;
use oxc_span::SourceType;

/// Owns the data the parsed AST borrows from: the arena and the source text.
struct CstOwner {
    allocator: Allocator,
    source: Arc<str>,
}

/// The dependent: the parsed program (which holds comment trivia + exact spans)
/// plus recovered diagnostics. Borrows from `CstOwner`.
struct Parsed<'a> {
    program: Program<'a>,
    errors: Vec<OxcDiagnostic>,
    panicked: bool,
}

self_cell::self_cell!(
    struct CstCell {
        owner: CstOwner,
        #[covariant]
        dependent: Parsed,
    }
);

/// A parsed file: the Oxc AST + retained verbatim source + recovered errors.
/// Always produced — Oxc recovers from syntax errors (`errors()` non-empty,
/// `panicked()` distinguishes the unrecoverable case).
pub struct Cst {
    cell: CstCell,
    source_type: SourceType,
}

impl Cst {
    /// The structurally-valid AST root (valid even when `errors()` is non-empty).
    pub fn program(&self) -> &Program<'_> {
        &self.cell.borrow_dependent().program
    }

    /// Comment trivia, span-keyed and source-ordered.
    pub fn comments(&self) -> &[Comment] {
        &self.cell.borrow_dependent().program.comments
    }

    /// The original source, retained verbatim.
    pub fn source(&self) -> &str {
        &self.cell.borrow_owner().source
    }

    /// Recovered parse diagnostics. Empty for a clean parse.
    pub fn errors(&self) -> &[OxcDiagnostic] {
        &self.cell.borrow_dependent().errors
    }

    /// `true` only when the parser could not recover (AST is empty).
    pub fn panicked(&self) -> bool {
        self.cell.borrow_dependent().panicked
    }

    pub fn source_type(&self) -> SourceType {
        self.source_type
    }

    /// Reproduce the input by emitting the retained source. Byte-for-byte equal to
    /// the original for any file (the lossless guarantee, proven by the round-trip
    /// test). This is verbatim retention, NOT AST re-serialization — structural
    /// re-emit is the formatter's job and is deliberately out of scope here.
    pub fn write_source(&self, out: &mut String) {
        out.push_str(self.source());
    }
}

/// Stage-1 query body: parse `source` into a self-owning [`Cst`].
///
/// This is the ONE `Parser::new` call site in the workspace (I-1 / P15). It never
/// panics on input — Oxc returns a structurally-valid AST plus recovered errors.
pub(crate) fn compute_cst(source: Arc<str>, source_type: SourceType) -> Cst {
    let owner = CstOwner {
        allocator: Allocator::default(),
        source,
    };
    let cell = CstCell::new(owner, |owner| {
        // === the ONLY Parser::new in the workspace (I-1 / P15) ===
        let ret = Parser::new(&owner.allocator, &owner.source, source_type).parse();
        Parsed {
            program: ret.program,
            errors: ret.diagnostics.to_vec(),
            panicked: ret.panicked,
        }
    });
    Cst { cell, source_type }
}
