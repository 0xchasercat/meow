---
spec_id: DIST-001
title: "Cargo workspace + meow CLI skeleton; upstream V8 pins"
subsystem: crates/cli (+ workspace root)
status: merged
blast_radius: high
plan_ref: "Initial spec backlog · Wave 1 (DIST-001); Build sequence · P0 Foundations"
constitution_ref: [I-10]
depends_on: []
estimate: medium
backend: omp
---

## Motivation

This is the dependency root of the entire project — every other spec branches from the workspace and the `meow` binary created here (PLAN *Build sequence · P0 Foundations*; *Initial spec backlog · Wave 1 · DIST-001*). Nothing can be built, gated, or shipped until there is a Cargo workspace to add crates to and a `meow` entrypoint to hang commands off.

Two jobs, both load-bearing:

1. **Centralize the external pins.** `deno_core`/`rusty_v8` are consumed via *unmodified upstream* and **never forked** (I-10, CANON §26.2, ADR-1; Q13). Declaring them once in `[workspace.dependencies]` makes "we consume upstream, in lockstep" a single auditable fact instead of N drifting per-crate pins — exactly what the `footprint` gate (I-10) inspects.
2. **Stand up an honest CLI skeleton.** The §19 command surface exists from day one as *honest stubs* — each prints a structured "not yet implemented" line and exits non-zero. CRAFT ("the action must DO the work"): a command that prints nothing and exits `0` is a fake success path and a craft block. `--version`/`--help` work for real.

Footprint is tracked from here (I-10), but only *meaningfully* from RT-001 onward, when V8 is actually linked — this skeleton links none of the heavy externals, so its binary is a few MB, trivially under the ≤ 60 MB budget.

## Acceptance criteria

Concrete, checkable shapes. All Rust is real `clap`-derive / `fn` / `const`, not pseudocode.

### A1 — Root `Cargo.toml` (workspace manifest, created by this spec)

```toml
# Cargo.toml  (repo root) — owned by DIST-001
[workspace]
resolver = "2"
members  = ["crates/*"]

[workspace.package]
edition      = "2021"
rust-version = "1.85"           # MSRV; bumped only by a DIST spec
license      = "MIT OR Apache-2.0"
repository   = "https://github.com/<org>/meow"
version      = "0.0.0"          # pre-release; SemVer governs the public surface (CANON §26.3), not this skeleton

# Externals are DECLARED centrally so consuming crates inherit one pin each.
# A pin here costs nothing until a crate lists it under its own [dependencies];
# this skeleton consumes ONLY clap, so cargo builds nothing heavy.
[workspace.dependencies]
# === DIST-001 ===  (shared-edit surface: each consuming spec appends ITS pin under its own marker)
# V8 — unmodified upstream, never a git fork (I-10, CANON §26.2). Consumed by RT-* only; NOT here.
deno_core   = "0.340"          # illustrative; RT-001 confirms the exact deno_core↔v8 lockstep pair
v8          = "130"            # rusty_v8 publishes as the `v8` crate; deno_core pins it transitively — list explicitly only for raw bindings
# Parse / semantic / transform — the ONE parser (I-1). Consumed by GRAPH/RT/TOOL.
oxc         = "0.40"
# Bundler — declared when TOOL-003 lands (left commented so principles-check sees the intent without an unused pin):
# rolldown  = "..."            # added by TOOL-003
# Async I/O — consumed by RT-002 (tokio; io_uring on Linux).
tokio       = { version = "1", features = ["rt-multi-thread", "fs", "io-util", "net", "macros"] }
# Serde — config / lockfile / wire types. Consumed by CFG/PKG/LOAD.
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
# CLI — consumed HERE.
clap        = { version = "4", features = ["derive"] }
# Errors — typed + causal (CRAFT). thiserror for library crates, anyhow for the binary's top edge.
thiserror   = "2"
anyhow      = "1"
# Dev-only (integration tests of the binary).
assert_cmd  = "2"
predicates  = "3"
```

- `resolver = "2"`, `members = ["crates/*"]` (glob — future crates self-register by existing on disk, no edit here).
- Version-policy contract: caret pins; `deno_core` and `v8` move only in lockstep and only via a DIST spec (CANON §26.3 — engine bumps are minor, never silent). Each version string is illustrative-at-draft; the implementer pins the exact set that resolves against the upstream Deno release matrix (see Operator notes — these are not invented working pairs).

### A2 — `crates/cli/Cargo.toml`

```toml
[package]
name         = "meow-cli"      # crate naming convention: meow-<subsystem>; dir is crates/<subsystem>
version.workspace      = true
edition.workspace      = true
rust-version.workspace = true
license.workspace      = true

[[bin]]
name = "meow"
path = "src/main.rs"

[dependencies]
clap = { workspace = true }    # the ONLY external this skeleton consumes

[dev-dependencies]
assert_cmd = { workspace = true }
predicates = { workspace = true }
```

### A3 — `crates/cli/src/main.rs`

```rust
use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    // clap handles --version/--help (exit 0) and usage errors (exit 2) before we get here.
    <cli::Cli as clap::Parser>::parse().run()
}
```

### A4 — `crates/cli/src/cli.rs` (the command tree — exact signatures)

```rust
use std::path::PathBuf;
use std::process::ExitCode;
use clap::{Args, Parser, Subcommand, ValueEnum};

/// Reserved exit code: command recognized, but its implementation has not landed yet.
/// Distinct from 0 (success), 1 (generic failure), and clap's 2 (usage error) so a
/// harness can tell "not built yet" from "you held it wrong".
pub const EXIT_UNIMPLEMENTED: u8 = 3;

/// meow — a standards-first JavaScript/TypeScript runtime + unified toolchain.
#[derive(Debug, Parser)]
#[command(name = "meow", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute a program (default mode = strict-web).
    Run(RunArgs),
    /// Watch-mode run.
    Dev(RunArgs),
    /// Install dependencies into the virtual store.
    Install(InstallArgs),
    /// Add a dependency + update the lockfile.
    Add(PkgArgs),
    /// Remove a dependency + update the lockfile.
    Remove(PkgArgs),
    /// Run a typed task from meow.tasks.ts.
    Task(TaskArgs),
    /// Isolate-backed test runner.
    Test(TestArgs),
    /// Typecheck — delegated to the tsc/tsgo daemon (ADR-5).
    Check(PathArgs),
    /// Lint over the shared pipeline.
    Lint(PathArgs),
    /// Format over the shared pipeline.
    Fmt(PathArgs),
    /// Bundle via Rolldown over the module graph.
    Bundle(BundleArgs),
    /// Observability: slowest imports / init / cold-start (the Module Load timeline).
    #[command(name = "why-slow")]
    WhySlow(PathArgs),
    /// Observability: largest modules / duplicate packages.
    #[command(name = "why-large")]
    WhyLarge(PathArgs),
    /// Dependency provenance / paths.
    #[command(name = "why-dep")]
    WhyDep(WhyDepArgs),
    /// Execution trace.
    Trace(RunArgs),
    /// Sampling/allocation profile.
    Profile(RunArgs),
    /// Environment / config / lockfile health.
    Doctor,
    /// Regenerate shadow configs (.meow/tsconfig.json, root package.json) — ADR-8.
    Sync,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Entry module to execute.
    pub entry: PathBuf,
    /// Arguments forwarded to the program (everything after `--`).
    #[arg(last = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Virtual-store install mode (CANON §18; PKG owns the final flag surface).
    #[arg(long, value_enum, default_value_t = InstallMode::Pnp)]
    pub mode: InstallMode,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum InstallMode { Pnp, Vfs, Materialize, Vendor }

#[derive(Debug, Args)]
pub struct PkgArgs {
    /// Package specifier(s), e.g. `lodash@^4`.
    #[arg(required = true)]
    pub packages: Vec<String>,
}

#[derive(Debug, Args)]
pub struct TaskArgs {
    /// Task name from meow.tasks.ts.
    pub name: String,
    #[arg(last = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct TestArgs {
    /// Optional test name/path filter.
    pub filter: Option<String>,
}

#[derive(Debug, Args)]
pub struct PathArgs {
    /// Target paths (default: workspace root).
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BundleArgs {
    /// Entry module(s) to bundle.
    #[arg(required = true)]
    pub entries: Vec<PathBuf>,
    /// Output directory.
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct WhyDepArgs {
    /// Package to explain.
    pub pkg: String,
}
```

### A5 — Stub behavior contract (`crates/cli/src/cli.rs`, cont.)

```rust
impl Command {
    /// (canonical verb as the user typed it, PLAN phase where the real impl lands).
    /// Single source of truth for the stub message AND the per-command test table.
    pub const fn landing(&self) -> (&'static str, &'static str) {
        match self {
            Command::Run(_)      => ("run",       "P1"),
            Command::Dev(_)      => ("dev",       "P1"),
            Command::Install(_)  => ("install",   "P2"),
            Command::Add(_)      => ("add",       "P2"),
            Command::Remove(_)   => ("remove",    "P2"),
            Command::Task(_)     => ("task",      "P4"),
            Command::Test(_)     => ("test",      "P6"),
            Command::Check(_)    => ("check",     "P3"),
            Command::Lint(_)     => ("lint",      "P3"),
            Command::Fmt(_)      => ("fmt",       "P3"),
            Command::Bundle(_)   => ("bundle",    "P3"),
            Command::WhySlow(_)  => ("why-slow",  "P6"),
            Command::WhyLarge(_) => ("why-large", "P6"),
            Command::WhyDep(_)   => ("why-dep",   "P2"),
            Command::Trace(_)    => ("trace",     "P6"),
            Command::Profile(_)  => ("profile",   "P6"),
            Command::Doctor      => ("doctor",    "P6"),
            Command::Sync        => ("sync",      "P1"),
        }
    }
}

impl Cli {
    pub fn run(self) -> ExitCode {
        let (verb, phase) = self.command.landing();
        // HONEST stub (CRAFT — "the action must DO the work"): no fake success path.
        // Structured, single-line, machine-greppable; goes to stderr; exit is non-zero.
        eprintln!("meow: not yet implemented — `{verb}` lands in PLAN {phase}");
        ExitCode::from(EXIT_UNIMPLEMENTED)
    }
}
```

Contract, exhaustively:
- Every one of the 18 subcommands resolves to a non-empty `(verb, phase)`; the `match` is exhaustive (compiler-enforced — adding a `Command` variant without a landing is a compile error, so the surface can never silently regress).
- Stub output is exactly `meow: not yet implemented — \`<verb>\` lands in PLAN <phase>\n` on **stderr**, nothing on stdout, exit `EXIT_UNIMPLEMENTED` (3).
- `meow --version` → prints the workspace version, exit 0 (clap). `meow --help` and `meow <cmd> --help` → usage, exit 0 (clap). `meow` with no subcommand / a bad flag → clap usage error, exit 2.

### A6 — Size probe (`scripts/footprint.sh`, interface)

A thin, deterministic probe the `footprint` gate runs (GATES.md → I-10). Not Rust; a POSIX script so CI and the gate share one number.

```sh
# scripts/footprint.sh — owned by DIST-001
#   Builds the release binary, reports on-disk + gzip-compressed size, checks the budget.
#   Budget is the COMPRESSED download (CANON §26.1: "< 60 MB refers to the compressed download;
#   on-disk is larger"). Exit non-zero iff the compressed size exceeds the budget.
#
#   env:  FOOTPRINT_BUDGET_BYTES   default 62914560 (60 * 1024 * 1024)
#   out (one line, stable/greppable):
#         footprint: meow on-disk=<bytes> gzip=<bytes> budget=<bytes> status=<ok|over>
#   exit: 0 ok, 1 over budget, 2 binary missing
```

Behavior: `cargo build --release -p meow-cli`; stat `target/release/meow`; `gzip -c | wc -c` for the compressed figure; compare gzip size to `FOOTPRINT_BUDGET_BYTES`; print the one-line report; exit accordingly. For this skeleton (no V8) gzip is a few MB → `status=ok`. The number is *recorded* now and becomes the real watch point at RT-001 when V8 links in.

### A7 — Floor is green
`cargo build --release` succeeds at the workspace root (floor `build` gate, GATES.md Tier 1). `cargo fmt --all -- --check` clean (floor `format` gate). `harness/principles-check.sh` green (non-empty `plan_ref`/`constitution_ref`, balanced comment-marker fences).

## Non-goals

- **No V8.** No isolate, event loop, or op layer — RT-001 owns embedding; this skeleton links *none* of the heavy externals (they are declared, not consumed).
- **No real command logic.** Every subcommand is a stub; zero runtime/resolver/toolchain behavior.
- **No per-subsystem crates.** Only `crates/cli` is created. Each later spec creates its own crate (the `members = ["crates/*"]` glob already admits it).
- **No `meow <file.ts>` run-shorthand.** The §19 shorthand (`meow foo.ts` ⇒ `meow run foo.ts`) is deferred to RT/run work; the skeleton requires an explicit subcommand.
- **No install-flag finalization.** `--mode <pnp|vfs|materialize|vendor>` is a placeholder surface; PKG-002/PKG-004 own the final spelling (`--vfs|--materialize|--vendor` per §19).
- **No release channels / packaging.** `curl|sh`, Homebrew, Winget (CANON §26.1) are a later DIST spec.
- **No CI wiring.** `scripts/footprint.sh` exists and is runnable; hooking it into CI is an `H` chore.

## Interface

The contract this spec publishes to the rest of the repo:

1. **Workspace root** — `[workspace]` (resolver 2, `crates/*`), `[workspace.package]` (edition/MSRV/license/version), and `[workspace.dependencies]` as the *single* place externals are pinned. Consuming specs add a crate under `crates/`, list `<dep> = { workspace = true }` in their own `Cargo.toml`, and (if introducing a new external) append the pin under a `# === <SPEC-ID> ===` marker inside `[workspace.dependencies]`.
2. **The `meow` binary** — `crates/cli` produces `target/{debug,release}/meow`. `--version`/`--help` real; 18 subcommands present as honest stubs.
3. **`EXIT_UNIMPLEMENTED = 3`** — the stable "recognized but not built" exit code, distinct from clap's 0/2.
4. **`Command::landing()`** — the one place that maps a subcommand to its PLAN phase; later specs flip a stub to real by replacing that arm's dispatch (the `landing` table shrinks as phases land).
5. **`scripts/footprint.sh`** — the `footprint` gate's measurement tool, stable one-line output.

## Implementation sketch

- Create `Cargo.toml` (root), `crates/cli/Cargo.toml`, `crates/cli/src/main.rs`, `crates/cli/src/cli.rs`, `scripts/footprint.sh`, `crates/cli/tests/cli.rs`.
- `[workspace.dependencies]` is a **shared-edit surface**: the whole block is fenced `# === DIST-001 ===`; every later spec that introduces an external appends its pin under its own `# === <SPEC-ID> ===` marker (keeps `principles-check.sh`'s fence-balance check meaningful and makes the `footprint` audit a single read).
- The binary edge stays tiny: `main` only parses and dispatches; clap owns `--version`/`--help`/usage. `landing()` is `const fn` so the surface is compile-time data — the dispatch and the test table read the same source of truth.
- Errors: the skeleton has no fallible runtime path, so no `thiserror`/`anyhow` use yet (declared centrally for RT/LOAD/PKG). No `unwrap`/`expect`/`panic!` on any user-reachable path (CRAFT; vacuously true here, established as the pattern).
- `footprint.sh`: pure POSIX, no Rust deps; `gzip`+`wc` for the compressed figure to match §26.1 semantics; honest comment that on-disk ≠ budgeted-compressed and that the number only bites once V8 links (RT-001).

## Tests required

`crates/cli/tests/cli.rs` (integration, `assert_cmd` + `predicates` — exercises the built binary, the real surface):

- **`version_and_help_succeed`** — `meow --version` exits 0 and stdout contains the version; `meow --help` exits 0 and lists the subcommands. *(Proves the real paths work.)*
- **`every_subcommand_stub_is_honest`** — table-driven over all 18 `(verb, phase)` pairs: invoking each (with minimal valid args, e.g. `run x.ts`, `add p`, `why-dep p`) exits **non-zero and equal to 3**, prints nothing to **stdout**, and stderr matches `^meow: not yet implemented — \`<verb>\` lands in PLAN <phase>$`. *(Invariant: no command silently succeeds — the CRAFT "must DO the work" line, mechanically enforced; and the exit code is the stable contract.)*
- **`bad_usage_is_distinct`** — `meow` (no subcommand) and `meow run` (missing required `entry`) exit **2** (clap), not 3. *(Invariant: "not built" and "used wrong" are distinguishable.)*
- **`footprint_probe_runs_and_is_under_budget`** — invoke `scripts/footprint.sh`; assert exit 0 and stdout matches `status=ok`, parse `gzip=<n>` and assert `n < 62914560`. *(I-10 initial measurement is recorded and green for the V8-less skeleton.)*

**Gates touched:** `footprint` (I-10) — initial measurement recorded via the probe (no `rustc`/`cargo`/clang bundled; `deno_core`/`v8` declared as unmodified upstream, non-fork — auditor-verifiable from the manifest). Floor Tier 1: `build` (`cargo build --release` green), `format`, `principles`.

## Rollout

Bootstrap spec — there is nothing to migrate. Fully reversible: deleting the root `Cargo.toml`, `crates/cli/`, and `scripts/footprint.sh` returns the repo to pre-workspace state (no other crate exists yet to depend on it). No flags, no data, no deploy target (PLAN: *Prod target: none yet*). SemVer (CANON §26.3) does not bind a `0.0.0` pre-release skeleton; the public-surface promise begins when commands become real.

## Operator notes

- **`deno_core`/`v8` versions are illustrative-at-draft, not invented working pairs.** deno_core pins a specific `v8` (rusty_v8) build; the implementer MUST select the exact lockstep pair from the current upstream Deno release matrix at build time and let `cargo` resolve it — do not hand-fabricate a pair. This skeleton compiles neither (it consumes only `clap`), so a wrong pin here cannot break the floor build; RT-001 is where the pair is exercised and frozen.
- **`rusty_v8` is published as the crate named `v8`.** Listed explicitly in `[workspace.dependencies]` only for crates needing raw bindings; most consume it transitively through `deno_core`.
- **`rolldown` is declared as a comment, not a live pin** — it lands with TOOL-003 (P3). Adding an unused heavy pin now would slow `cargo` for no benefit and muddy the `footprint` audit.
- **Footprint budget semantics:** the ≤ 60 MB target is the *compressed download* (CANON §26.1); on-disk is larger. The probe measures both and gates on the compressed figure. The number is meaningful from RT-001 (V8 linked); for this skeleton it is trivially green.
- **Assumptions taken (genuine forks; CANON resolved the strategy, these are local mechanics — report to Main, do not edit decisions.json):**
  1. Crate naming convention `meow-<subsystem>` (package) with directory `crates/<subsystem>` and bin `meow`. Reasonable default; CANON fixes neither the package name nor the bin/crate split.
  2. `EXIT_UNIMPLEMENTED = 3` for stubs (distinct from clap's 0/2). Any non-{0,2} value satisfies the contract; 3 chosen for clarity.
  3. MSRV `1.85` / `edition = "2021"`. Pick the edition that `deno_core`'s current release requires; bump only via a DIST spec.
  4. `install --mode <enum>` placeholder vs §19's `--vfs|--materialize|--vendor` boolean flags — modeled as an enum for the stub; PKG owns the final flag surface.
