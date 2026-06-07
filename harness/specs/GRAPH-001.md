---
spec_id: GRAPH-001
title: "Oxc parse pipeline as an incremental query system (CST/semantic/IR)"
subsystem: crates/graph
status: drafting
blast_radius: high
plan_ref: "Initial spec backlog · Wave 1 (GRAPH-001); Build sequence · P0 Foundations"
constitution_ref: [I-1, I-3]
depends_on: [DIST-001]
estimate: large
backend: omp
---

## Motivation

The parse pipeline is the technical heart of `meow` (CANON §7): every tool — runtime,
linter, formatter, typecheck orchestrator, bundler, LSP — reads one shared, incremental
model of the project, and **no subsystem builds its own parser or graph** (I-1,
`graph-integrity`). This spec stands up that model: a `crates/graph` crate that wraps Oxc
and exposes stages 1 (Lossless Syntax Tree / CST), 2 (Semantic Graph), and 5 (Runtime IR)
as **memoized, invalidatable queries** (CANON §7.2 — salsa/`rust-analyzer`-style), so an
editor keystroke recomputes only the dependent slices instead of re-running the pipeline.
It is also the **single parser entrypoint** that `principles-check.sh` P15 enforces
(`Parser::new` may appear only under `/(graph|parse)/`). Stage 5 carries the transformer
seam that RT-003 fills with the erasable-only type-strip policy (I-3, `strip-fidelity`).
This is P0 Foundations — incrementality must be designed in now; retrofitting it is a
rewrite (CANON §7.2). Depends on DIST-001 for the Cargo workspace + pinned Oxc deps.

## Acceptance criteria

Concrete, checkable surface. All Oxc parser construction lives in **one file**
(`crates/graph/src/cst.rs`); every other crate consumes only `GraphDb`.

### Crate layout

```text
crates/graph/
  Cargo.toml          # oxc_parser, oxc_semantic, oxc_allocator, oxc_ast, oxc_span,
                      # oxc_transformer, oxc_codegen, salsa, self_cell, thiserror, rustc-hash
  src/lib.rs          # GraphDb facade + public re-exports (Cst, SemanticGraph, RuntimeIr, FileId, GraphError)
  src/db.rs           # salsa database, SourceFile input, Db trait — all pub(crate)
  src/ids.rs          # FileId newtype + path interner
  src/cst.rs          # stage-1 query + the ONLY `Parser::new` call site (P15)
  src/semantic.rs     # stage-2 query (oxc_semantic)
  src/ir.rs           # stage-5 query + transformer/codegen plumbing; RT-003 call-site fence
  src/strip.rs        # StripPolicy trait + default seam               // === RT-003 ===
  src/error.rs        # GraphError, StripDiagnostic
  tests/integrity.rs  # round-trip, incrementality, single-parser surface
```

### Identity & input (`src/ids.rs`, `src/db.rs`)

```rust
/// Stable, interned file identity. Typed, Copy, O(1) — not a re-hashed PathBuf
/// on the hot path. Public; never exposes a salsa handle.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FileId(u32);

#[salsa::input]                      // pub(crate) — salsa never crosses the facade
pub(crate) struct SourceFile {
    #[returns(ref)] pub text: std::sync::Arc<str>,
    pub source_type: oxc_span::SourceType,   // ts/tsx/mts/js… inferred from the path
}
```

### Facade (`src/lib.rs`) — the entire public contract

```rust
pub struct GraphDb { /* private: salsa db + path interner */ }

impl GraphDb {
    pub fn new() -> Self;                                  // installs PermissivePolicy seam
    pub fn with_policy(policy: std::sync::Arc<dyn StripPolicy>) -> Self; // RT-003 installs the real one

    /// Insert or update a file's text. Bumps the salsa revision and invalidates ONLY
    /// the queries that transitively read this file. Returns its stable id.
    pub fn set_file(&mut self, path: impl Into<std::path::PathBuf>, text: std::sync::Arc<str>) -> FileId;

    /// Remove a file; invalidates its dependents. Returns whether it existed.
    pub fn remove_file(&mut self, path: &std::path::Path) -> bool;

    pub fn file_id(&self, path: &std::path::Path) -> Option<FileId>;
    pub fn path(&self, file: FileId) -> Option<&std::path::Path>;

    // ---- stage queries: memoized; the borrow is valid until the next `&mut self` ----
    pub fn cst(&self, file: FileId) -> &Cst;               // stage 1 — always succeeds (error-recovering)
    pub fn semantic(&self, file: FileId) -> &SemanticGraph;// stage 2
    pub fn runtime_ir(&self, file: FileId) -> Result<&RuntimeIr, &[StripDiagnostic]>; // stage 5
}
```

*Path-keyed convenience (literal `db.cst(path)` form from the backlog) is provided as
thin wrappers — `cst_of(&self, &Path) -> Option<&Cst>`, etc. — that intern via `file_id`.
`FileId` is canonical on the hot path (typed, no re-hash); see Operator notes.*

### Stage 1 — Lossless Syntax Tree (`src/cst.rs`)

Oxc's AST borrows from a bump `Allocator` + the source `&str`, so the CST is a
self-referential owner built with `self_cell` (no hand-written `unsafe`):

```rust
struct CstOwner { allocator: oxc_allocator::Allocator, source: std::sync::Arc<str> }

self_cell::self_cell!(
    struct CstCell { owner: CstOwner, #[covariant] dependent: Parsed }
);

struct Parsed<'a> {
    program: oxc_ast::ast::Program<'a>,
    comments: oxc_allocator::Vec<'a, oxc_ast::Comment>,   // trivia, span-keyed
    errors:   Vec<oxc_diagnostics::OxcDiagnostic>,        // recovered; CST still produced
}

pub struct Cst { cell: CstCell, source_type: oxc_span::SourceType }

impl Cst {
    pub fn program(&self) -> &oxc_ast::ast::Program<'_>;
    pub fn comments(&self) -> &[oxc_ast::Comment];
    pub fn source(&self) -> &str;                          // retained verbatim
    pub fn errors(&self) -> &[oxc_diagnostics::OxcDiagnostic];
    /// Reconstruct source from structure (node + trivia spans over the retained text),
    /// proving no source information was discarded. Used by the round-trip test.
    pub fn write_source(&self, out: &mut String);
}

#[salsa::tracked(returns(ref))]
pub(crate) fn cst(db: &dyn Db, f: SourceFile) -> Cst {
    let allocator = oxc_allocator::Allocator::default();
    // === the ONLY Parser::new in the workspace (I-1 / P15) ===
    // oxc_parser::Parser::new(&allocator, source, source_type).parse()
    // … build Parsed inside the self_cell, capturing program+comments+errors …
}
```

**Lossless guarantee:** `Cst` retains the original source `Arc<str>` plus exact byte spans
on every node and every comment; `write_source` walks the structure and reproduces the
input. For any well-formed file `cst(f).write_source(&mut s)` yields `s == original_text`.

### Stage 2 — Semantic Graph (`src/semantic.rs`)

```rust
pub struct SemanticGraph { cell: SemanticCell /* self_cell over an Arc<Cst> + Semantic<'a> */ }

impl SemanticGraph {
    pub fn scopes(&self)  -> &oxc_semantic::ScopeTree;
    pub fn symbols(&self) -> &oxc_semantic::SymbolTable;   // bindings + references
    pub fn nodes(&self)   -> &oxc_semantic::AstNodes;
}

#[salsa::tracked(returns(ref))]
pub(crate) fn semantic(db: &dyn Db, f: SourceFile) -> SemanticGraph {
    let cst = cst(db, f);                                  // tracked read → dependency edge
    // oxc_semantic::SemanticBuilder::new().build(cst.program()) → scopes/symbols/refs
}
```

### Stage 5 — Runtime IR + transformer seam (`src/ir.rs`, `src/strip.rs`)

```rust
/// Type-erased, V8-ready output. Type erasure preserves source positions
/// (CANON §9: annotations blanked, no downlevel emit, no sourcemap layer).
pub struct RuntimeIr { pub code: std::sync::Arc<str>, pub positions_preserved: bool }

pub(crate) struct IrOutcome { ir: Option<RuntimeIr>, diagnostics: Vec<StripDiagnostic> }

#[salsa::tracked(returns(ref))]
pub(crate) fn runtime_ir(db: &dyn Db, f: SourceFile) -> IrOutcome {
    let sem = semantic(db, f);                             // tracked read → dependency edge
    // === RT-003 ===  erasable-only policy + the actual erasure live behind this seam.
    match db.strip_policy().check(sem) {
        Ok(())   => /* oxc_transformer type-strip → oxc_codegen → RuntimeIr */,
        Err(d)   => IrOutcome { ir: None, diagnostics: d },
    }
    // === /RT-003 ===
}
```

```rust
// src/strip.rs
pub trait StripPolicy: Send + Sync {
    /// Reject non-erasable constructs before transform; the diagnostic names the fix.
    /// RT-003 supplies the real erasable-only impl (enum/namespace/param-props/import=).
    fn check(&self, sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>>;
}

// === RT-003 ===  default permissive seam; RT-003 replaces with the erasable-only policy.
#[derive(Default)]
pub struct PermissivePolicy;
impl StripPolicy for PermissivePolicy {
    fn check(&self, _sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>> { Ok(()) }
}
// === /RT-003 ===
```

### Errors (`src/error.rs`)

```rust
#[derive(thiserror::Error, Debug)]
pub enum GraphError {
    #[error("unknown file: {0}")]
    UnknownFile(std::path::PathBuf),
}

/// A non-erasable-construct diagnostic: cause + remedy + span (PRODUCT.md bar).
pub struct StripDiagnostic {
    pub span: oxc_span::Span,
    pub message: String,   // e.g. "Enums emit runtime code and cannot be type-stripped."
    pub help: String,      // e.g. "Use a `const` object instead."
}
```

The facade never panics on user input: `cst`/`semantic` on an unset `FileId` is a caller
invariant (a `FileId` is only minted by `set_file`); the path-keyed wrappers return
`Option`. Parse errors never panic — Oxc recovers and the CST is still produced.

## Non-goals

- **No type information / typechecking.** Stage 3 is delegated to the `tsc`/`tsgo` daemon
  (ADR-5); it is *not* a native stage and this crate builds no type graph (CANON §7.1).
- **No module resolution.** Import/export *edges* and resolved specifiers (stage 4) belong
  to LOAD; this crate parses a single file's text, it does not resolve specifiers.
- **No strip *policy*.** The erasable-only rules (which constructs are banned, the exact
  fix messages) are owned by RT-003 (I-3) and land here only inside the `// === RT-003 ===`
  fences. The default `PermissivePolicy` is an explicit seam, not a shipped guarantee.
- **No separate green tree.** "Lossless" is delivered via Oxc AST + retained source +
  exact spans + comment trivia (Option A below), not a second rowan-style CST — Oxc is the
  one parser (ADR-2). See Operator notes.
- **No bundling / codegen output format decisions** beyond producing position-preserving
  erased source for the runtime; bundler output is TOOL/Rolldown.

## Interface

The contract is exactly the `GraphDb` facade above. Salsa, `self_cell`, and every Oxc type
that carries a lifetime are **private to `crates/graph`**; the public surface is
`GraphDb`, `FileId`, `Cst`, `SemanticGraph`, `RuntimeIr`, `StripPolicy`, `StripDiagnostic`,
`GraphError`. Consumers (RT, LOAD, TOOL, CHK, LSP) hold a `&GraphDb`/`&mut GraphDb` and
call the stage queries — they never see a salsa database or construct a parser.

Mutation→read protocol (salsa borrow rules): `set_file`/`remove_file` take `&mut self`;
stage queries take `&self` and return references valid until the next `&mut self`. This is
the rust-analyzer cadence: batch edits, then read.

## Implementation sketch

1. **DIST-001 first** pins unmodified upstream Oxc crates + `salsa` in the workspace
   `Cargo.toml`; this crate adds them as deps.
2. **salsa as the memoization engine, fully encapsulated.** `src/db.rs` defines the
   `#[salsa::db]` database and the `Db` trait (carrying `strip_policy()`); `SourceFile`
   is a `#[salsa::input]`. `GraphDb` owns the database + a `rustc_hash` path↔`FileId`↔
   `SourceFile` interner. Nothing salsa-typed leaves the crate. A future swap to a
   hand-built revision core is then a contained change behind the facade (see Operator
   notes for why salsa over hand-rolling).
3. **Stage 1** (`cst.rs`): the single `Parser::new` site; build the `self_cell` CST owning
   allocator + source; capture comments (trivia) and recovered errors. Implement
   `write_source` for the round-trip proof.
4. **Stage 2** (`semantic.rs`): `semantic` reads `cst` (tracked edge) and runs
   `SemanticBuilder`; wrap the lifetime-bound `Semantic` in a `self_cell` over `Arc<Cst>`.
5. **Stage 5** (`ir.rs`): `runtime_ir` reads `semantic` (tracked edge), calls the installed
   `StripPolicy` at the `// === RT-003 ===` fence, then runs `oxc_transformer` +
   `oxc_codegen` to produce position-preserving erased source. Until RT-003 lands, the seam
   is identity + `PermissivePolicy`; this is documented as a seam, never advertised as
   type-stripping.
6. **Incrementality falls out of salsa**: editing one file bumps only that `SourceFile`'s
   revision; salsa's dependency graph re-runs `cst`/`semantic`/`runtime_ir` for that file
   alone and serves memoized results for every other file.

Shared-file fences for downstream specs: `// === RT-003 ===` in `src/strip.rs`
(policy impl) and `src/ir.rs` (call site) are RT-003's edit region.

## Tests required

`tests/integrity.rs` — proves the I-1 / I-3 foundations and sets up the
`graph-integrity` and `strip-fidelity` gates:

- **Parse a TS file (smoke).** `set_file("a.ts", …)`; `cst`, `semantic`, `runtime_ir`
  return without panic; `semantic().symbols()` contains the expected bindings.
- **CST round-trips losslessly** (`graph-integrity`). For a corpus incl. comments,
  template literals, regex, and trailing whitespace: `cst(f).write_source(&mut s)` ⇒
  `s == original_text`, byte-for-byte. *Invariant: no source information is discarded.*
- **Incrementality** (`graph-integrity`). `set_file(A)`, `set_file(B)`; read
  `semantic(A)`/`semantic(B)`. Edit only A. Re-read both; assert via a salsa event probe
  that `semantic`/`cst` re-executed for A and were **served from memo** for B. *Invariant:
  editing one file recomputes only dependent queries (CANON §7.2).*
- **Single-parser surface** (`graph-integrity`, backs P15). Assert the public API exposes
  no parser/allocator constructor (`Parser::new`, `Allocator`) — the only path to a parsed
  artifact is `GraphDb`. P15's grep (`Parser::new` confined to `/(graph|parse)/`) is the
  cheap floor; this test is the structural proof. *Invariant: one parse entrypoint (I-1).*
- **Strip seam wired** (`strip-fidelity` setup). With a `StripPolicy` test double that
  rejects a marked node, `runtime_ir` returns `Err(&[StripDiagnostic])` carrying message +
  help + span; with `PermissivePolicy` it returns `Ok`. This is the seam RT-003's
  property test (`strip → reparse` positions stable, fix-message fixtures) plugs into; the
  erasable-only rules themselves are RT-003's tests, not this spec's.

GATES: `graph-integrity` (I-1), `strip-fidelity` (I-3, seam only here).

## Rollout

P0 Foundations; no user-facing surface, no flags. Landing the crate satisfies part of the
P0 exit ("stand up the Oxc pipeline as an incremental query system (stages 1/2/5)") and
the `graph-integrity` gate's single-pipeline assertion. **Reversibility:** the crate is
additive and consumed only by later specs (RT-003, LOAD-001, …) that are not yet built;
reverting is deleting `crates/graph` + its workspace member entry — no migration, no data,
no persisted artifacts. The salsa-vs-hand-built choice is reversible behind the `GraphDb`
facade without touching any consumer.

## Operator notes

Assumptions made (Main consolidates into `decisions.json`; flagged here for correction):

- **`salsa` crate chosen over a hand-built memo layer.** Correct, invalidatable
  incrementality (dependency back-edges, durability, cycle handling) is a genuinely hard,
  *solved* problem and the load-bearing seam of the whole product (I-1); hand-rolling it at
  P0 is the "hand-roll of a solved problem" craft symptom and risks subtle invalidation
  bugs across every tool. `rust-analyzer`'s production use is the proof. The one real risk
  — salsa API churn — is contained by **isolating every salsa type inside `crates/graph`
  behind `GraphDb`**, so a later swap to a hand-built revision core is a contained change,
  not a cross-crate break. Pin salsa to the generation `rust-analyzer` tracks.
- **"Lossless" = Oxc AST + retained source + exact spans + comment trivia (Option A)**, not
  a separate rowan-style green tree. CANON §7 says "trivia, comments, whitespace
  preserved"; Oxc is an AST (not green-tree) parser and is the one parser we bet on
  (ADR-2). Retaining the verbatim source plus full-fidelity spans makes every byte
  reconstructable (the round-trip test proves it) and serves the formatter/codemods via
  span-anchored edits — without building a second tree. If a true green tree is later
  required, it is an additive stage behind the same query, not a re-parse.
- **`FileId` (interned `u32` newtype) is canonical on the hot path**, with path-keyed
  wrappers for the literal `db.cst(path)` ergonomics in the backlog. Typed + O(1) +
  no per-call path re-hash (the "don't allocate gratuitously" bar); `set_file` returns it.
- **`runtime_ir` is a wired seam, not a shipped strip** until RT-003. The default
  `PermissivePolicy` + identity transform are documented as a seam; no doc/CLI string
  claims type-stripping works before RT-003 lands (I-11 / craft "prose must match code").
