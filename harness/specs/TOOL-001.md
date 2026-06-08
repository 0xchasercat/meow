---
spec_id: TOOL-001
title: "meow fmt — deterministic, idempotent formatter over the shared CST"
subsystem: crates/tool
status: drafting
blast_radius: medium
plan_ref: "Build sequence · P3 Parse-Once Toolchain; backlog P3 (TOOL-001)"
constitution_ref: [I-1, I-6, I-10, I-11]
depends_on: [GRAPH-001]
estimate: medium
backend: omp
---

## Motivation

P3 keeps one parse pipeline (`meow-graph`) for all tool operations. `meow fmt` must consume that shared parse surface and never construct a second parser. This slice keeps formatting in `crates/tool` (`meow-tool`) with no dedicated `crates/fmt` crate.

`meow fmt` uses Oxc’s formatter/code-printer intent (`oxc_formatter`) with **standard OXC options** and no custom formatting policy. For this slice there is **no dedicated fmt config surface** yet; output formatting is canonical, reproducible, and stable on host/locale.

## Acceptance criteria

### Scope / architecture

- Implement formatting in `crates/tool` only, under a `fmt` module.
- Consume `meow_graph::GraphDb` and `Cst`; formatting is over `cst.program()` and does not call `oxc_parser::Parser::new` (I-1).
- `meow fmt` is parse-once friendly: each file is loaded once into GraphDb, formatted from the already-parsed CST.
- No config file knobs yet for fmt options in this slice (I-11: no silent defaulting to unknown schema); use Oxc defaults from the formatter intent.

### Command behavior

- CLI shape: `meow fmt [PATHS]... [--check] [--stdin] [--stdin-filepath <PATH>]`.
- Default target set: workspace walk for JS/TS-like first-party extensions when `PATHS` is empty.
- `--check` computes change requirements and exits **1** when any formatted output differs; it must not write files.
- In non-`--check` mode, write in place only when output differs.
- `--stdin` reads source from stdin and writes formatted output to stdout.
- Exit is `0` when no hard failures; otherwise `1` with at least one human+machine-readable diagnostic.

### Formatter contract

- Use Oxc native formatter intent and printer with default options:
  - no second parse,
  - default Oxc line/indent policy,
  - deterministic line ending normalization.
- Reuse `GraphDb` source text for diagnostics and for idempotence checks.
- Refuse to rewrite files with parse/print failures; emit typed tool diagnostics instead (I-11).
- Preserve source semantics except formatting.

## Interface (`crates/tool/src/fmt.rs`)

```rust
use std::path::Path;

use meow_graph::{GraphDb, FileId, Cst};
use meow_tool::diag::{Diagnostic, Tool};

pub struct FmtOutcome {
    pub changed: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub code: Option<String>,
}

/// Host-pure formatter entrypoint over the shared CST.
pub fn format_cst(db: &GraphDb, file: FileId) -> FmtOutcome;

/// Convenience path for loaded bytes (single file API used by CLI edge).
pub fn format_text(cst: &Cst) -> FmtOutcome;

impl FmtOutcome {
    pub const fn from_noop(file: &str) -> Self;
}
```

`Diagnostic` here is the in-`meow-tool` shared diagnostic surface used by lint/check/bundle once those specs converge.

## CLI integration (`crates/cli/src/cli.rs`)

- `Command::Fmt(PathArgs)` may remain as the parse surface today, but command internals must call `meow_tool::fmt`.
- Replace the generic `EXIT_UNIMPLEMENTED` branch with `cmd_fmt`.
- `cmd_fmt`:
  1. Build one `GraphDb` and `set_file` each target once.
  2. For each file, call `meow_tool::fmt::format_cst`.
  3. In-place write unless `--check`, then check-only.
  4. Render diagnostics through `meow-ui` in the existing CI/tty-friendly style.

## Dependency pinning (workspace)

Add/confirm in `Cargo.toml` workspace deps under a `TOOL-001` fence:

- `oxc_formatter` and `oxc_formatter_core` pinned to the same OXC generation as `oxc_ast/oxc_parser` currently in `GRAPH-001`.
- `oxc_allocator` for formatter IR scratch.
- `meow-tool` consumes these with `{ workspace = true }`.

## Rollout & reversibility

- One crate addition under `crates/tool` plus CLI wiring.
- If this slice is rolled back, delete the `TOOL-001` CLI edit and keep behavior at the honest `EXIT_UNIMPLEMENTED` stub.

## Required proof in this slice

- Idempotence test: `fmt(fmt(x)) == fmt(x)` over TS/JS/TSX corpus fixtures.
- Parse-once test: `format_cst` reuses `GraphDb` parse and does not trigger `oxc_parser` within `meow-tool`.
- `--check` test: formatted target returns exit `1` and does not modify bytes.
- `--stdin` test for formatted output on success and unchanged file when `--check` is passed.
- Determinism test: same fixture formatted on different hosts yields same bytes.

## Operator notes

- This spec deliberately stays single-crate: lint (`TOOL-002`) and bundle skeleton (`TOOL-003`) reuse `crates/tool`.
- `meow_graph` parse-option compatibility that benefits formatting (`Formatter`-compatible options) remains a shared-formatting precondition, not a `meow-fmt`-only concern.
- I-10 claim posture: do **not** add a `<60MB proven` statement in this spec; footprint is proven by the `footprint` gate after implementation.
