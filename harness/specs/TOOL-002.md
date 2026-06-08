---
spec_id: TOOL-002
title: "meow lint — the linter over the shared semantic graph (rule engine + the shared Diagnostic model)"
subsystem: crates/lint (+ new crates/diag; crates/cli)
status: drafting
blast_radius: high
plan_ref: "Build sequence · P3 Parse-Once Toolchain; backlog P3 (TOOL-002)"
constitution_ref: [I-1, I-2, I-3, I-11]
depends_on: [GRAPH-001]
estimate: large
backend: omp
---

## Motivation

P3 consolidates lint/format/check/bundle on the **one shared graph** (PLAN §54-55; CANON §7.3:
*"the linter and the typechecker never disagree … a file is parsed once and reused across many
operations in a single invocation"*). The linter is the first tool to read **stage 2 — the Semantic
Graph (scopes/bindings/symbols/references)** that GRAPH-001 already produces. It must reuse that
parse, never start a second one (I-1, P15, `graph-integrity`); a linter that re-parses is the exact
re-derivation CANON §3.1 / CRAFT Part B call a defect.

TOOL-002 ships three things:

1. **`meow lint`** — `meow lint [paths] [--fix] [--format json|pretty]`, the §19 verb (CLI stub flips
   to real).
2. **The rule engine** — a `Rule` trait, a registry, per-rule severity from the one config
   (`meow_config::Lint`), and an autofix seam — running rules over each file's already-parsed
   `Cst` + `SemanticGraph`. The two **required** built-ins from PLAN line 104 are `no-first-party-cjs`
   (I-2 / ADR-3 — CJS is dependency-only, never first-party authored) and `erasable-only` (I-3 —
   no `enum`/`namespace`/parameter-properties/`import =`/`export =` that the type-strip cannot erase),
   plus a small core-correctness set.
3. **The shared `Diagnostic` model** — TOOL-002 is the **canonical diagnostic producer** for P3, so it
   introduces the neutral `meow_diag::Diagnostic` that lint **and** fmt/check/bundle all emit, in a new
   dependency-free `crates/diag` crate (NOT inside the linter — that would force fmt/check/bundle to
   depend on the linter to share a type). This is the model the P3 exit ("lint/format/check/bundle
   agree") rests on, and it maps cleanly to LSP/editor diagnostics (CANON §20).

Traceability: `plan_ref` → PLAN *Build sequence · P3* + *backlog P3* (`TOOL-002`, listing
`no-first-party-cjs` + `erasable-only`); `constitution_ref` → **I-1** [`graph-integrity`] (one parse,
read not rebuilt), **I-2** (no-first-party-cjs), **I-3** (erasable-only), **I-11** (diagnostics name
cause + fix; the linter is honestly *not* type-aware — that is CHK-002).

## Acceptance criteria

Concrete, checkable Rust. Two new crates (`meow-diag`, `meow-lint`) + the `crates/cli` dispatch flip.
Every user-triggerable path returns `Result`; **no rule ever panics** on parsed input (CRAFT Part B —
the runtime/toolchain does not panic on user code; a malformed file is linted on Oxc's recovered AST,
never a crash). All host I/O (read sources, write `--fix` output) lives at the **CLI edge**, never in
`meow-lint`/`meow-diag` (P16 / I-6 — tool crates take a `&GraphDb` + caller-supplied config).

### `crates/diag` — `meow-diag`, the neutral shared diagnostic model (zero heavy deps)

The home all P3 tools share. Depends only on `serde` + `thiserror` — **never** on `meow-graph`/Oxc, so
it sits *below* every tool and the parse pipeline; no crate that emits a diagnostic risks a cycle.

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One diagnostic, identical in shape across lint/fmt/check/bundle. Maps 1:1 to an LSP
/// `Diagnostic` (CANON §20): `file`+`span` → `uri`+`range`, `severity`, `code`, `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The file the diagnostic is about. Path-based (NOT a `meow_graph::FileId`) so this crate
    /// stays graph-free; producers map `FileId → path` via `GraphDb::path()` (one O(1) lookup).
    pub file: Arc<Path>,
    /// Byte range into that file's source. Same field layout as `oxc_span::Span`, so a producer
    /// converts with `Span { start: s.start, end: s.end }` — no re-computation.
    pub span: Span,
    pub severity: Severity,
    /// Stable, namespaced, machine-greppable id: `"lint/no-first-party-cjs"`, `"lint/erasable-only"`,
    /// `"check/TS2345"`, `"fmt/format"`, `"bundle/unresolved-import"`. `'static` — codes are a closed set.
    pub code: DiagCode,
    /// The cause (PRODUCT.md diagnostic bar: *what* is wrong).
    pub message: String,
    /// The remedy (*how* to fix it), e.g. "Use a `const` object instead." `None` when there is no
    /// single obvious fix. The exemplar (CANON §9, PRODUCT.md) is cause + remedy together.
    pub help: Option<String>,
    /// An optional machine-applicable fix (`--fix`). `Some` only when the edit is semantics-preserving.
    pub fix: Option<Fix>,
    /// Which tool produced it — for grouping in output and routing to the right LSP source.
    pub tool: Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span { pub start: u32, pub end: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity { Error, Warning, Info }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct DiagCode(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool { Lint, Fmt, Check, Bundle }

/// A semantics-preserving edit set. `apply` is pure text surgery — no host, no graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix { pub edits: Vec<Edit> }

/// Replace `source[span.start..span.end]` with `replacement`. An empty `replacement` is a deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit { pub span: Span, pub replacement: String }

#[derive(thiserror::Error, Debug)]
pub enum FixError {
    #[error("overlapping fixes at bytes {a:?} and {b:?} cannot be applied in one pass")]
    Overlap { a: (u32, u32), b: (u32, u32) },
}

/// Apply non-overlapping edits to `source`, back-to-front by descending `span.start` so earlier
/// offsets stay valid without recomputation. Overlap with an already-applied edit → `FixError::Overlap`
/// (the CLI defers the loser to the next `--fix` pass, see below). Pure; the CLI owns the disk write.
pub fn apply_fixes(source: &str, fixes: &[Fix]) -> Result<String, FixError>;

/// Render for humans (file:line:col, severity, code, message, the `help` remedy line, a source caret).
/// `source_of` lets the renderer fetch each file's text for the caret without `meow-diag` reading disk.
pub fn render_pretty(diags: &[Diagnostic], source_of: &dyn Fn(&Path) -> Option<&str>) -> String;

/// Machine-readable: a JSON array of `{ file, span:{start,end}, severity, code, message, help, tool }`.
/// `Diagnostic` derives `Serialize` for this (the `fix` is summarized as `fixable: bool`, not the edits).
pub fn render_json(diags: &[Diagnostic]) -> String;
```

### `crates/lint` — `meow-lint`, the engine (reads the shared graph; never parses)

```rust
use std::collections::BTreeMap;
use std::path::Path;
use meow_graph::{Cst, FileId, GraphDb, SemanticGraph};
use meow_config::{Lint, Mode, Severity as ConfigSeverity};
use meow_diag::{Diagnostic, DiagCode, Fix, Severity, Span, Tool};

/// The whole-invocation entrypoint, and the **parse-once seam** the P3 driver plugs into: it is handed
/// a `&GraphDb` whose files are already loaded (one `set_file` per file) and returns diagnostics —
/// it NEVER constructs a parser/allocator and NEVER calls `set_file` (I-1, P15). Diagnostics come back
/// sorted by `(file, span.start, code)` for stable output.
pub fn lint(db: &GraphDb, files: &[FileId], config: &LintConfig) -> Vec<Diagnostic>;

/// One file. Reads `db.cst(file)` + `db.semantic(file)` (memoized — reused, not recomputed), builds a
/// `LintContext`, runs each enabled rule. An unknown/retired `FileId` yields no diagnostics (never panics).
pub fn lint_file(db: &GraphDb, file: FileId, config: &LintConfig) -> Vec<Diagnostic>;

/// Resolved configuration: which rules run, at what severity, in which project mode.
pub struct LintConfig {
    /// rule-id → effective severity. Built from the registry's defaults overlaid with `meow_config::Lint`.
    severities: BTreeMap<&'static str, ConfigSeverity>,
    /// Gates mode-sensitive rules (e.g. `no-ambient-node-api` is off under `node-compat`/`legacy`).
    mode: Mode,
}

impl LintConfig {
    /// Defaults from `RuleRegistry::builtin()`, overlaid with the user's `meow.config.ts` `lint.rules`.
    /// An override naming a rule the registry does not know is an **honest error** (I-11 — silent
    /// ignore would let a typo'd rule id quietly do nothing), listing the unknown id + the valid ids.
    pub fn new(lint: &Lint, mode: Mode) -> Result<LintConfig, LintConfigError>;
    pub fn with_registry(reg: &RuleRegistry, lint: &Lint, mode: Mode) -> Result<LintConfig, LintConfigError>;
}

#[derive(thiserror::Error, Debug)]
pub enum LintConfigError {
    #[error("unknown lint rule {id:?} in meow.config.ts (known rules: {known})")]
    UnknownRule { id: String, known: String },
}

/// What a rule emits; the engine stamps `file`, the configured `severity`, `code`, and `Tool::Lint`.
pub struct RuleDiagnostic {
    pub span: Span,
    pub message: String,
    pub help: Option<String>,
    pub fix: Option<Fix>,
}

/// Static metadata; `id` is the stable rule id (== the `DiagCode` suffix after `lint/`).
pub struct RuleMeta {
    pub id: &'static str,
    pub default: ConfigSeverity,
    pub fixable: bool,
    /// `Some(mode)` ⇒ the rule only runs in that mode (e.g. `Some(Mode::StrictWeb)`); `None` ⇒ all modes.
    pub only_in: Option<Mode>,
}

/// A lint rule. Read-only over one file's already-parsed graph; `emit` is the sink the engine owns.
pub trait Rule: Send + Sync {
    fn meta(&self) -> RuleMeta;
    fn check(&self, ctx: &LintContext<'_>, emit: &mut dyn FnMut(RuleDiagnostic));
}

/// The per-file read context handed to every rule. Borrows the shared graph — owns nothing, parses nothing.
pub struct LintContext<'a> {
    pub path: &'a Path,
    pub cst: &'a Cst,                 // stage 1 — AST + retained source + trivia (GRAPH-001)
    pub semantic: &'a SemanticGraph,  // stage 2 — scopes/bindings/symbols/references
    pub mode: Mode,
}

impl LintContext<'_> {
    pub fn source(&self) -> &str { self.cst.source() }
}

/// The set of available rules. `builtin()` is the shipped set; an empty/custom registry is for tests.
pub struct RuleRegistry { rules: Vec<Box<dyn Rule>> }
impl RuleRegistry {
    pub fn builtin() -> RuleRegistry;                       // the five rules below
    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_;
    pub fn get(&self, id: &str) -> Option<&dyn Rule>;
}
```

**The engine loop (parse-once, the I-1 heart):** `lint` iterates files; per file it pulls `db.cst(f)` and
`db.semantic(f)` **once** (memoized in `GraphDb` — `Recomputes` does not advance on the 2nd…Nth read),
then runs every enabled rule against that single `LintContext`. N rules over one file ⇒ **one parse, one
semantic build** (the `graph-integrity` test asserts exactly this via `db.recomputes()`). A rule whose
`meta().default`/config severity resolves to `Off` is **skipped before `check`** (never run); a rule with
`only_in: Some(m)` where `ctx.mode != m` is skipped. Each `RuleDiagnostic` becomes a `Diagnostic` with
`code: DiagCode("lint/<id>")`, `severity` = the configured level mapped (`Warn → Warning`, `Error →
Error`), `tool: Tool::Lint`, `file: Arc::from(ctx.path)`.

**Parse-error surfacing:** when `cst.panicked()` (Oxc could not recover) the file yields a single
`Error` diagnostic from the first `cst.errors()` entry and rules are skipped; when `cst.errors()` /
`semantic.errors()` are non-empty but recoverable, those are emitted as `Error`s **and** rules still run
on the recovered AST. Parse/semantic diagnostics carry `code: DiagCode("lint/syntax-error")` — honest,
never swallowed (I-11).

### The built-in rules (exact ids; the required two + a small core)

Deliberately small — the value is the shared model, not rule breadth (CANON §24.10: *"do not build
every tool surface before the shared graph works"*). Deeper type-aware rules are CHK-002's fast-preview
linter (ADR-5) and the post-1.0 semantic-graph track.

| id (`lint/<id>`) | default | mode | fixable | reads | invariant |
|---|---|---|---|---|---|
| `no-first-party-cjs` | error | all | no | CST + unresolved refs | I-2 / ADR-3 |
| `erasable-only` | error | all | no | `SemanticGraph` via `ErasablePolicy` | I-3 |
| `no-ambient-node-api` | error | strict-web only | no | CST imports + unresolved refs | I-2 (§8/§11) |
| `no-unused-vars` | warn | all | imports only | `SemanticGraph::scoping()` | core |
| `no-debugger` | warn | all | yes | CST | core |

- **`no-first-party-cjs`** (I-2, ADR-3). Flags CJS *authoring* in first-party source: a `require(<string>)`
  call, a `module.exports = …` / `exports.foo = …` assignment, and references to the CJS ambients
  `module` / `require` / `exports` / `__dirname` / `__filename`. **Scope-aware** to avoid false positives:
  it reports a name only when that name is in `scoping().root_unresolved_references()` (i.e. genuinely the
  CJS ambient, not a user's locally-bound `const require = …`). Message: *"CommonJS `require()` is not
  allowed in first-party code — meow is ESM-only."* help: *"Use an ESM `import` instead (CJS is supported
  only for dependencies)."* No autofix (require→import is a semantic rewrite). This is the **static/editor
  surface** of I-2; the runtime refusal is LOAD-002 and the CI floor is `principles-check.sh` P12 — three
  surfaces of one invariant, not a second enforcement path.
- **`erasable-only`** (I-3). **Delegates to `meow_graph::ErasablePolicy`** (the *exact* policy the RT-003
  type-strip consults), NOT a second AST walk: `ErasablePolicy.check(ctx.semantic)` returns
  `Err(Vec<StripDiagnostic>)`; each `StripDiagnostic` maps to a `RuleDiagnostic` (`span/message/help` copied
  verbatim). Result: `meow lint` and `meow run`'s strip error use **identical words** for `enum`/`namespace`/
  parameter-properties/`import =`/`export =` (CANON §9 catalog) — agreement *by construction* (I-1), the
  whole point of the shared graph. No autofix (e.g. enum→const-object is non-mechanical).
- **`no-ambient-node-api`** (I-2; CANON §18 ships this in the `defineMeow` `lint.rules` example, §8/§11 —
  Node APIs are never ambient). Strict-web only (`only_in: Some(Mode::StrictWeb)`). Flags `import`/`export …
  from "node:*"` (CST import-source check) and references to Node-only globals (`process`, `Buffer`,
  `global`, `__dirname`) that resolve to nothing (unresolved refs ∩ the Node-global set). help points at
  `node-compat` mode. To avoid double-reporting, the CJS-ambient names owned by `no-first-party-cjs`
  (`__dirname`, `require`, …) are reported by whichever rule is enabled at higher severity, else by
  `no-first-party-cjs` (documented precedence).
- **`no-unused-vars`** (core; the semantic-graph exemplar). Iterates `scoping().symbol_ids()`, flags each
  where `scoping().symbol_is_unused(id)` and the symbol is not exported (`symbol_flags(id)` lacks `Export`)
  and not a deliberately-ignored `_`-prefixed name; reports at `scoping().symbol_span(id)`. Severity `warn`.
  **Autofix only for unused *imports*** (delete the specifier, or the whole `import` statement when it has
  no remaining bindings) — safe because an unused import has no side-effect we remove. Plain unused locals
  are warn-only (their initializer may have a side effect; removing it is not semantics-preserving).
- **`no-debugger`** (core; the clean autofix exemplar). Flags each `debugger;` statement (CST), severity
  `warn`, **fixable**: `Fix { edits: [Edit { span: <statement>, replacement: "" }] }`. Demonstrates the
  end-to-end `--fix` path and the round-trip property (the rest of the file is byte-identical).

### `crates/cli` — the `meow lint` surface (stub → real)

The existing `Command::Lint(PathArgs)` is replaced (`PathArgs` lacks `--fix`/`--format`) and the dispatch
stub flips to `cmd_lint`. Fenced `// === TOOL-002 ===`.

```rust
// === TOOL-002 ===  (in cli.rs, replacing `Lint(PathArgs)`)
#[derive(Debug, Args)]
pub struct LintArgs {
    /// Target paths (files or dirs). Default: the project root (first-party sources).
    pub paths: Vec<PathBuf>,
    /// Apply machine-applicable fixes in place, then report what remains.
    #[arg(long)]
    pub fix: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat { Pretty, Json }   // clap stays out of meow-diag; maps to its renderers
// === /TOOL-002 ===
```

`cmd_lint(args)` (the standalone, single-tool parse-once driver):
1. `find_project_root(cwd)` (reuse the LOAD-001 helper); `MeowConfig::load(root)` → `cfg.lint` + `cfg.mode`
   (a missing/TS-only config falls back to defaults, honestly — same boundary as the other `cmd_*`).
2. `LintConfig::new(&cfg.lint, cfg.mode)?` — an unknown rule id aborts with a clear stderr line + exit 1.
3. Discover targets: walk `args.paths` (default `[root]`) for first-party `*.ts|tsx|mts|js|mjs|jsx`,
   skipping `node_modules`/`.meow`/`target`/`.git`. Host reads live here.
4. `let mut db = GraphDb::new();` (installs `ErasablePolicy` by default); `db.set_file(path, Arc::from(text))`
   **once** per file; collect `FileId`s. `let diags = meow_lint::lint(&db, &ids, &config);`
5. `--fix`: filter diagnostics with `fix`, group by file, `meow_diag::apply_fixes`, write changed files,
   `db.set_file` the new text (GraphDb invalidates only that file), re-`lint`; loop until a pass makes no
   change or `MAX_FIX_PASSES` (10) is hit; report `N fixed`. Then render the remaining diagnostics.
6. Render: `OutputFormat::Pretty → render_pretty(&diags, &|p| sources.get(p))` to stderr; `Json →
   render_json` to stdout.
7. Exit: `1` if any remaining `Severity::Error`; else `0`. (clap usage error stays clap's `2`.)

```rust
#[derive(thiserror::Error, Debug)]
pub enum LintError {            // CLI-edge errors; rules/engine never panic
    #[error("config: {0}")] Config(#[from] meow_lint::LintConfigError),
    #[error("reading {path}: {source}")] Read { path: PathBuf, source: std::io::Error },
    #[error("writing fix to {path}: {source}")] Write { path: PathBuf, source: std::io::Error },
    #[error(transparent)] Fix(#[from] meow_diag::FixError),
}
```

## Non-goals

- **Type-aware lint / typecheck.** No type information — `meow check` (CHK-001, the delegated `tsc`/`tsgo`
  daemon) and the **fast-preview type-aware linter** (CHK-002) own that (ADR-5). TOOL-002 is purely
  syntactic + scope-based; docs/help must say so (I-11) — it is *not* labelled authoritative type analysis.
- **The formatter.** `meow fmt` is TOOL-001; it reads the same `Cst` and emits `Tool::Fmt` diagnostics
  through this same `meow-diag` model, but its rewrite logic is its own spec. `no-unused-vars` removing an
  import is a lint fix, not formatting.
- **The cross-tool parse-once driver.** `cmd_lint` is the *single-tool* driver. The shared driver that runs
  lint+fmt+check+bundle over **one** `GraphDb` in one invocation is P3 shared infra (see Operator notes —
  flagged for Main to assign a home, e.g. `meow-driver` or in `meow-cli`); TOOL-002 delivers the lint
  engine that conforms to its `(&GraphDb, &[FileId]) -> Vec<Diagnostic>` seam.
- **Plugin / user-authored rules.** The registry is the fixed built-in set; no dynamic rule loading.
- **LSP wiring.** `meow-diag` maps cleanly to LSP `Diagnostic` (CANON §20) by design, but the editor
  integration is LSP/CHK territory, not this spec.
- **`--max-warnings`, severity-promotes-to-error, per-file disable comments (`// meow-lint-disable`).**
  Out of scope for the P3 slice; the `code`/`span` model is sufficient to add them later without a reshape.

## Interface

- **New crate `meow-diag`** (`crates/diag`): `Diagnostic`, `Span`, `Severity`, `DiagCode`, `Tool`, `Fix`,
  `Edit`, `FixError`, `apply_fixes`, `render_pretty`, `render_json`. Deps: `serde` (+ `serde_json` for
  `render_json`), `thiserror`. **No** `meow-graph`/Oxc dep. This is the P3-wide contract every tool emits.
- **New crate `meow-lint`** (`crates/lint`): `lint`, `lint_file`, `LintConfig`, `LintConfigError`, `Rule`,
  `RuleMeta`, `RuleDiagnostic`, `RuleRegistry`, `LintContext`. Deps: `meow-graph`, `meow-diag`,
  `meow-config`, Oxc AST/span types *only as re-exported through `meow-graph`* (it reads `Cst`/`SemanticGraph`
  — it does **not** depend on `oxc_parser` and constructs no parser, P15).
- **Consumes (does not redefine):** `meow_graph::{GraphDb, FileId, Cst, SemanticGraph, ErasablePolicy,
  StripPolicy, StripDiagnostic, Recomputes}`, `meow_config::{Lint, Mode, Severity}`.
- **`crates/cli`**: `Command::Lint(LintArgs)` + `OutputFormat` + the `cmd_lint` dispatch arm (fenced).
- **Workspace** (`Cargo.toml`): two new `[workspace.members]` + `[workspace.dependencies]` pins
  (`meow-diag`, `meow-lint`), fenced `# === TOOL-002 ===`. `meow-cli/Cargo.toml` gains both as deps.
- No new heavy externals (I-10): `serde`/`serde_json`/`thiserror` are already in-tree; the linter adds none.

## Implementation sketch

Two new crates + a fenced CLI flip; no existing crate's logic changes.

```text
crates/diag/
  Cargo.toml          # serde, serde_json, thiserror
  src/lib.rs          # Diagnostic/Span/Severity/DiagCode/Tool + re-exports
  src/fix.rs          # Fix/Edit/FixError + apply_fixes (back-to-front, overlap-detecting)
  src/render.rs       # render_pretty (caret via source_of) + render_json
crates/lint/
  Cargo.toml          # meow-graph, meow-diag, meow-config
  src/lib.rs          # lint/lint_file, LintConfig, the engine loop, RuleDiagnostic→Diagnostic stamping
  src/rule.rs         # Rule trait, RuleMeta, LintContext, RuleRegistry::builtin()
  src/rules/no_first_party_cjs.rs
  src/rules/erasable_only.rs       # delegates to meow_graph::ErasablePolicy — NO second walk
  src/rules/no_ambient_node_api.rs
  src/rules/no_unused_vars.rs
  src/rules/no_debugger.rs
  tests/lint_corpus.rs             # behavior + the parse-once (graph-integrity) probe
```

Shared-file fences (`// === TOOL-002 ===`):
- `crates/cli/src/cli.rs` — the `Command::Lint` variant (now `LintArgs`), the `OutputFormat` enum, the
  `Command::Lint(args) => cmd_lint(&args)` dispatch arm, and `cmd_lint` + `LintError`. (`Command::landing`
  keeps `("lint","P3")` — harmless; the real arm shadows the stub.)
- `Cargo.toml` (workspace) — `# === TOOL-002 ===` members + dep pins.
- `crates/cli/Cargo.toml` — `# === TOOL-002 ===` dep on `meow-lint` + `meow-diag`.

Flow: `cmd_lint` reads sources → `GraphDb::set_file` (once each) → `meow_lint::lint(&db, &ids, &cfg)`
→ engine pulls memoized `cst`/`semantic` and runs rules → `Vec<Diagnostic>` → (optionally) `apply_fixes`
+ disk write at the edge → `render_*`. One graph, one parse, many rules.

## Tests required

`crates/lint/tests/lint_corpus.rs` (integration) + unit tests in each `rules/*.rs` and in `diag/src/fix.rs`.
Proves **I-1** [`graph-integrity`] (the phase gate), with cross-checks against **I-2**/**I-3**/**I-11**.

- **Parse-once (I-1, `graph-integrity`) — the load-bearing test.** `set_file` 3 files once, `lint` with the
  full `RuleRegistry::builtin()`, then assert `db.recomputes() == Recomputes { cst: 3, semantic: 3,
  runtime_ir: 0 }`: every file parsed/analyzed **exactly once** despite five rules reading it, and the
  linter never touches stage 5. *Invariant: a file is parsed once and reused across operations (CANON §7.3).*
- **Single-producer surface (I-1, P15).** Assert `meow-lint` references no `oxc_parser`/`Parser::new`/
  `Allocator` (structural) and the only route to an AST is `GraphDb`. Every emitted diagnostic is a
  `meow_diag::Diagnostic` with `tool == Tool::Lint`.
- **`no-first-party-cjs` (I-2).** `require("x")`, `module.exports = {}`, `exports.f = 1` each flagged at
  the right span with the ESM-only message; a file that *locally* binds `const require = …` and calls it is
  **not** flagged (scope-aware via unresolved refs); pure-ESM is clean.
- **`erasable-only` parity with RT-003 (I-3).** For the `enum`/`namespace`/parameter-property/`import =`/
  `export =` fixtures, the lint diagnostic's `message`+`help` are **byte-identical** to
  `ErasablePolicy.check()`'s `StripDiagnostic` (assert by calling both). An ambient `declare namespace`
  is clean (matches the strip policy). *Invariant: lint ≡ the type-strip's verdict, by construction.*
- **`no-ambient-node-api` mode gate (I-2).** `import "node:fs"` / bare `process` flagged under
  `Mode::StrictWeb`; the same file under `Mode::NodeCompat` produces **zero** `no-ambient-node-api`
  diagnostics (rule skipped). Confirms `only_in` gating.
- **`no-unused-vars` (semantic graph).** An unused `const x = 1` warns at its declaration span; a used
  binding is clean; an exported binding is never flagged; an unused `import { a }` gets a `Fix` and `--fix`
  removes it.
- **`no-debugger` autofix round-trip.** `debugger;` warns; `apply_fixes` deletes exactly that statement;
  the rest of the source is **byte-identical**; a re-lint of the fixed text is clean.
- **Autofix conflict + multi-pass.** Two overlapping fixes → `apply_fixes` returns `FixError::Overlap`
  (or applies the earliest-start and the CLI defers the other to the next pass); a synthetic two-pass case
  reaches a fixpoint within `MAX_FIX_PASSES`. *Invariant: `--fix` is convergent, never corrupts source.*
- **Config (I-11).** A rule set to `"off"` emits nothing (skipped before `check`); `"warn"` →
  `Severity::Warning`; an unknown rule id in `lint.rules` → `LintConfigError::UnknownRule` naming the id +
  the known ids (no silent no-op).
- **Robustness / no panic (CRAFT Part B).** A syntactically broken file → an `Error` "syntax-error"
  diagnostic and no rule panic; an empty file → no diagnostics; an unknown/retired `FileId` → empty.
- **`render_json` shape.** Output parses as a JSON array; each element has `file`/`span`/`severity`/`code`/
  `message`/`tool`; `render_pretty` includes the `help` remedy line.

GATES: **`graph-integrity`** (I-1 — the parse-once + single-producer tests are this gate's fixture for the
lint surface; the P3 exit's "parsed once and reused" clause). Cross-cutting evidence feeds `compat`
(no-first-party-cjs ≡ I-2), `strip-fidelity` (erasable-only ≡ RT-003 messages), and `honesty` (the
"fast/syntactic, not type-aware" labelling, I-11). Floor: `cargo test/clippy/fmt`, `principles-check.sh`.

## Rollout

Two **new, additive** crates (`meow-diag`, `meow-lint`) consumed initially only by `meow lint`; the one
breaking edit is internal — `Command::Lint(PathArgs) → Command::Lint(LintArgs)` in the CLI, a single fenced
change with no external contract (the verb was an unimplemented stub). No persisted state, no migration, no
data. **Reversibility:** revert the `// === TOOL-002 ===` fences in `cli.rs` + the two `Cargo.toml`s and
delete `crates/diag` + `crates/lint`; `meow lint` returns to its honest `EXIT_UNIMPLEMENTED` stub.

`meow-diag` is the **shared dependency** for the rest of P3: TOOL-001 (fmt), TOOL-003 (bundle), CHK-001/002
(check) consume the same `Diagnostic`. Landing order: `meow-diag` first (this spec creates it), siblings
build on it — so this spec should merge before the other P3 tool surfaces wire their diagnostics, or they
fence against the agreed names (coordinated over `irc`; see Operator notes).

Forward-compatible: per-file `// meow-lint-disable` comments, `--max-warnings`, and severity promotion are
additive on top of the `code`/`severity` model; plugin rules would extend `RuleRegistry` without reshaping
the `Rule` trait.

## Operator notes

**Shared `Diagnostic` contract — broadcast to the 5 P3 drafters over `irc` (TOOL-002 is the canonical
producer).** Surface for Main to consolidate into `decisions.json`:

- `FORK[TOOL-002:diag-home]` **new dependency-free `crates/diag` (`meow-diag`)** over extending
  `meow-graph`, because rendering (pretty/json) and a tool-neutral diagnostic vocabulary have no place in
  the parse-pipeline crate, and a crate that depends on nothing heavier than `serde` sits *below* every
  tool — zero cycle risk when lint/fmt/check/bundle all emit it. `meow-graph` keeps its own
  `StripDiagnostic` (stage-5 lowering); the linter maps `StripDiagnostic → Diagnostic` at its edge. Alt:
  extend `meow-graph` (no new crate, but couples the shared diagnostic to Oxc and grows graph's surface).
- `FORK[TOOL-002:diag-file-id]` `Diagnostic.file: Arc<Path>` (not `meow_graph::FileId`) so `meow-diag`
  never depends on `meow-graph`; producers do one `GraphDb::path(file)` lookup to stamp it. Alt: `FileId`
  (forces `meow-diag → meow-graph`, dragging Oxc into the diagnostic crate).
- `FORK[TOOL-002:diag-shape]` pinned fields `{ file, span:{start,end}, severity(Error|Warning|Info),
  code:DiagCode("tool/id"), message, help?, fix?, tool(Lint|Fmt|Check|Bundle) }`. `Span` mirrors
  `oxc_span::Span`'s `{start,end}` for a free conversion; namespaced `code` keeps producers collision-free.
- `FORK[TOOL-002:driver]` parse-once driver seam = **every tool entrypoint takes `&GraphDb` + `&[FileId]`
  and returns `Vec<Diagnostic>`; the driver `set_file`s each file once, then fans the tools over that one
  `GraphDb`.** The cross-tool driver crate itself is unowned shared infra — **flagged for Main** to place
  (`meow-driver`, or a function in `meow-cli`); `cmd_lint` is the single-tool instance. Pin this signature
  so fmt/check/bundle conform.
- `FORK[TOOL-002:rule-set]` built-ins = `{no-first-party-cjs, erasable-only, no-ambient-node-api,
  no-unused-vars, no-debugger}` — the two PLAN-required + `no-ambient-node-api` (shipped in CANON §18's
  config example) + two core exemplars (one semantic, one autofixable). Kept small on purpose (CANON
  §24.10); type-aware rules are CHK-002 / post-1.0.
- `FORK[TOOL-002:fix-conflict]` `--fix` applies **non-overlapping edits per pass** (back-to-front by
  descending `span.start`), defers overlaps to a bounded multi-pass loop (≤10 passes) until a fixpoint —
  the ESLint/oxlint convention; never applies overlapping edits in one pass (would corrupt source). Alt:
  single-pass apply-non-overlapping + report-the-rest.
- `FORK[TOOL-002:config]` reuse the existing `meow_config::Lint { rules: BTreeMap<String, Severity> }` and
  `meow_config::Severity {Off,Warn,Error}` verbatim (the config knob), distinct from `meow_diag::Severity
  {Error,Warning,Info}` (the emitted level); an unknown rule id in config is an **honest error**, not a
  silent no-op (I-11). `no-ambient-node-api` reads `MeowConfig.mode`.

**Integration points (named):**
- **GRAPH-001:** consumes `GraphDb::{set_file, cst, semantic, path, recomputes}`, `Cst::{source, program,
  comments, errors, panicked}`, `SemanticGraph::{scoping, nodes, errors}`, `Recomputes`. No change to graph.
- **RT-003 (merged):** `erasable-only` calls the public re-export `meow_graph::ErasablePolicy` +
  `StripPolicy::check` + `StripDiagnostic` — **the policy is already in-tree** (`crates/graph/src/strip.rs`),
  so this is a consume, not a blocking dep; that is why `depends_on` is just `[GRAPH-001]`.
- **meow-config (CFG-001/002):** `MeowConfig::{load}`, `cfg.lint`, `cfg.mode`. The lint config schema
  already exists — TOOL-002 adds no config keys.
- **LOAD-002 / `principles-check.sh` P12:** `no-first-party-cjs` is the *static* surface of I-2; the loader
  refusal (runtime) and the P12 grep (CI floor) are the other two — complementary, not a second resolver/
  enforcement path (CRAFT Part B: "one graph, one resolver").

**Assumptions (low-confidence, flagged for correction):**
- `Diagnostic.severity` is the *emitted* level; rule `Off` is enforced by **not running** the rule, so an
  `Off` rule costs nothing. If a future "report-but-don't-fail" tier is wanted, add `Severity::Info`
  routing rather than a fourth config level.
- First-party file globs `*.{ts,tsx,mts,js,mjs,jsx}`; `.cts`/`.cjs` are first-party-CJS (banned) and are
  flagged by `no-first-party-cjs` if present rather than silently skipped.
- `no-unused-vars` only autofixes unused **imports**; unused locals stay warn-only (initializer side
  effects make deletion non-semantics-preserving). Revisit if a "remove unused local" fix is requested.
