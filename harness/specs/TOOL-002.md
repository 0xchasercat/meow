---
spec_id: TOOL-002
title: "meow lint — Oxc-native lint over the shared parse surface"
subsystem: crates/tool
status: drafting
blast_radius: high
plan_ref: "Build sequence · P3 Parse-Once Toolchain; backlog P3 (TOOL-002)"
constitution_ref: [I-1, I-2, I-3, I-11]
depends_on: [GRAPH-001]
estimate: medium
backend: omp
---

## Motivation

The operator has converged on a single-tool crate for P3 primitives. `meow lint` therefore ships from `crates/tool` (`meow-tool`), not a separate `crates/lint` crate. This keeps parse/DIAG reuse tight and avoids a second graph contract in this slice.

Linting for this slice is **Oxc-native**: run Oxc’s existing recommended rule set (the equivalent of `OXC_RECOMMENDED`) over the already-parsed, shared CST from `meow-graph`.

UI output must be user-visible and styled through `meow-ui`:

- errors are reported via `meow_ui::Ui::hiss` (Bad Kitty path),
- source snippets use `meow_ui::diagnostic::render` (Mochi Pink highlight).

## Acceptance criteria

- Implement linting in `crates/tool` (`meow_tool::lint`) and reuse `meow_graph` as the only parse surface for this slice.
- No per-tool parser (`oxc_parser`) in `meow-tool` lint path.
- `meow lint` runs OXC recommended rules over each file loaded into a single `GraphDb`.
- No per-file config support yet; this slice is “recommended defaults + stable output + shared diagnostics” (I-11).
- Output formats:
  - default: pretty `meow-ui` diagnostics with Bad Kitty/Mochi Pink snippets,
  - `--json`: machine-readable JSON array.
- Exit code:
  - `0` when only warnings (if we choose to downgrade) or clean,
  - `1` when any lint error is produced,
  - `1` for malformed Oxc diagnostics, parse-graph failures, or I/O failures.
- Robustness: syntactic errors in input never panic.

## Interface (`crates/tool/src/lint.rs`)

```rust
use std::path::PathBuf;

use meow_graph::{GraphDb, FileId};
use meow_tool::diag::{Diagnostic, Severity, Tool};

pub struct LintOptions {
    /// Use OXC `recommended` ruleset; extension point for future custom profiles.
    pub recommended: bool,
}

/// Single-invocation lint over files already in GraphDb.
pub fn lint(db: &GraphDb, files: &[FileId], opts: &LintOptions) -> Vec<Diagnostic>;

/// Convenience helper for direct CLI integration over discovered file ids.
pub fn lint_paths(db: &GraphDb, files: &[FileId]) -> Vec<Diagnostic> {
    lint(db, files, &LintOptions { recommended: true })
}
```

Diagnostics are `meow_tool`-owned, not in a separate `meow-diag` crate this slice.

## CLI integration (`crates/cli/src/cli.rs`)

- `Command::Lint(PathArgs)` is replaced with or extends to `LintArgs { paths: Vec<PathBuf>, json: bool }`.
- `Command::Lint(args)` dispatches to `cmd_lint(&args)` instead of the honest stub.
- `cmd_lint` flow:
  1. Walk default paths (same include behavior as fmt) and load each path text once into a new `GraphDb`.
  2. Call `meow_tool::lint_paths` once (single parse per file).
  3. Render diagnostics:
     - `json`: serialize to stdout;
     - default: use `meow_ui::diagnostic::SourceDiagnostic` + `meow_ui::diagnostic::render` and emit through `Ui::hiss` for non-empty output.
  4. Return non-zero if any `Diagnostic.severity == Severity::Error`.

## Dependency pinning (workspace)

Under a `TOOL-002` fence:

- add `oxc_linter` + `oxc_diagnostics` (and shared Oxc generation compatibility checks) in `[workspace.dependencies]`.
- ensure pinned generation matches existing `GRAPH-001` `oxc_*` version family (same AST/span types).
- `meow-tool` consumes those deps plus `meow-ui`, `meow-graph`, `meow-config` where needed.

## Non-goals

- custom rule registry,
- `--fix`/autofix path in this slice,
- type-aware diagnostics (kept for `CHK-001/CHK-002`).

## Required proof in this slice

- OXC recommended rule baseline test: fixture triggers at least one canonical Oxc lint rule from the default set and emits a stable code/severity/span.
- Parse-once test: collect diagnostics for the same file twice in one GraphDb and confirm parse count is not increased.
- Output test: pretty rendering path contains `Bad Kitty`/`Mochi Pink` style markers (or equivalent render assertions).
- Determinism test: linting output is byte-stable across host env/locale permutations.
- Robustness test: malformed JS/TS input returns diagnostics, no panic, non-zero exit.

## Operator notes

- This spec intentionally narrows to a single `crates/tool` path for all tool surfaces in the P3 slice.
- `GraphDb` parse provenance (CST + source bytes) is the authoritative input to avoid divergence with `meow run` and future `meow check`/`meow bundle`.
- I-10 posture: keep dependency bumps explicit and aligned to Oxc published generations; do not emit a binary-size claim until `footprint` gate passes.
