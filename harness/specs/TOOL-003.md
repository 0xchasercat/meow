---
spec_id: TOOL-003
title: "meow bundle — rolldown skeleton over shared graph"
subsystem: crates/tool
status: drafting
blast_radius: high
plan_ref: "Build sequence · P3 Parse-Once Toolchain; backlog later-phases (TOOL-003)"
constitution_ref: [I-1, I-5, I-6, I-10, I-11]
depends_on: [GRAPH-001]
estimate: large
backend: omp
---

## Motivation

`meow bundle` is required as a P3 command surface, but runtime-bridge wiring (PnP `Resolver` handoff from runtime/loader) is not yet finalized. This slice keeps bundling in `crates/tool` as a **skeleton**:

- fetch dependency pins and CLI scaffolding,
- create the `bundle` module/API,
- reject clearly when resolver integration is pending.

The bundle implementation is intentionally scoped so it does not block `meow run` or formatter/lint progress.

## Acceptance criteria

- Use one shared crate: `crates/tool` (`meow-tool`) for all tool work (fmt/lint/bundle), no standalone `crates/bundle`.
- Pull `rolldown` as a dependency in the `meow-tool` workspace path and wire it behind the bundle module surface.
- Add/adjust CLI command wiring so `meow bundle` is no longer an unconditional stub:
  - `meow bundle <ENTRY>... [--out <DIR>] [--outfile <FILE>] [--format <esm|cjs|iife>] [--minify] [--sourcemap[=external|inline]]`.
- `BundleArgs` / dispatch lives in `crates/cli` and calls into `meow_tool::bundle`.
- `meow-tool::bundle::bundle(...)` returns a typed `BundleError::ResolverUnavailable` when PnP runtime bridge inputs are missing; this error text must point to resolver runtime-finalization as the cause.
- No silent success claim for full bundling in this slice: it is an explicit skeleton contract (I-11).
- Keep dependency footprint claims honest: no `<60MB proven` statement until the `footprint` gate runs.

## Interface (`crates/tool/src/bundle.rs`)

```rust
use std::path::PathBuf;
use meow_graph::{FileId, GraphDb};

pub struct BundleRequest {
    pub entries: Vec<FileId>,
    pub out_dir: Option<PathBuf>,
    pub out_file: Option<PathBuf>,
    pub format: BundleFormat,
    pub minify: bool,
    pub sourcemap: Option<BundleSourcemap>,
}

#[derive(Debug, Clone, Copy)]
pub enum BundleFormat { ESM, CJS, IIFE }

#[derive(Debug, Clone, Copy)]
pub enum BundleSourcemap { External, Inline }

#[derive(Debug)]
pub enum BundleArtifact {
    /// Future-ready payload; not populated in this skeleton slice.
    Pending(Vec<u8>),
}

#[derive(Debug)]
pub enum BundleError {
    ResolverUnavailable(String),
    Format(String),
    Other(String),
}

pub async fn bundle(db: &mut GraphDb, req: BundleRequest) -> Result<BundleArtifact, BundleError>;
```

The resolver integration point is explicit (`GraphDb` + runtime/loader bridge input omitted for now). This spec does not require `oxc` re-parse avoidance proof yet because the full Rolldown path is not implemented.

## CLI integration (`crates/cli/src/cli.rs`)

- Add/confirm dedicated `BundleArgs` (or extend current args) with `entries`, `--out`, `--outfile`, `--format`, `--minify`, `--sourcemap`.
- Replace stub handling with a `cmd_bundle(&args)` dispatch.
- `cmd_bundle`:
  1. resolves entries to file paths for now,
  2. builds one `GraphDb` and `set_file` per entry,
  3. calls `meow_tool::bundle::bundle` with request metadata,
  4. on `ResolverUnavailable`, print a concise honest error and return non-zero.

No output files are guaranteed from this slice; the command boundary exists and fails honestly when runtime bridge handoff is missing.

## Required skeleton proof

- `cli` command parse coverage exists for `--out`, `--outfile`, `--format`, `--minify`, `--sourcemap`.
- `bundle` API compiles with the `BundleRequest` shapes above.
- `ResolverUnavailable` path is exercised and asserts non-zero exit in an integration test.

## Rollout / reversibility

- Add `rolldown` dependency pin and module skeleton under `crates/tool`.
- Keep all unimplemented parts feature-guardable or return explicit `BundleError` states so a later slice can fill execution with minimal churn.
- If this slice reverts, CLI falls back to the honest `EXIT_UNIMPLEMENTED` behavior for `bundle` and the `meow-tool` bundle module is removed.

## Notes for follow-up slice

- When runtime bridge finalizes `ResolutionGraph -> Resolver` handoff, replace `ResolverUnavailable` with Rolldown pre-pass + plugin path.
- Keep OXC generation consistency for any new `oxc_*` deps (`oxc_* = 0.134` today in-tree) unless publishing constraints force a compatible follow-up.
