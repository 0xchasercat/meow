---
spec_id: TOOL-003
title: "meow bundle — Rolldown bundler over the shared graph + the one resolver"
subsystem: crates/bundle
status: drafting
blast_radius: high
plan_ref: "Build sequence · P3 Parse-Once Toolchain; backlog later-phases (TOOL-003)"
constitution_ref: [I-1, I-5, I-6, I-10, I-11]
depends_on: [GRAPH-001, LOAD-003]
estimate: large
backend: omp
---

## Motivation

`meow bundle` is the bundler verb of P3's Parse-Once Toolchain (PLAN.md *Build sequence · P3*:
"consolidate lint/format/check/bundle on the shared graph; `meow lint`/`fmt`/`bundle`"). It is the
stage-4 Module Graph consumer (CANON §7.1 table: stage 4 = "Rolldown + native resolver"; §6.1 / ADR-2:
*"Direct reuse of Oxc and Rolldown as libraries, not subprocesses"*). The whole point of the phase is that
a file is **parsed once** into the shared `GraphDb` and resolved through the **one** `meow_loader::Resolver`,
then *reused* across lint/format/check/bundle in a single invocation — so the bundler can never disagree
with the runtime or the LSP about what a specifier means (CANON §7.3).

Two constitutional invariants are load-bearing here and are the reason this spec is non-trivial:

- **I-1 (one parse, one graph)** [`graph-integrity`]: meow constructs **no** Oxc parser of its own for
  bundling; every module is parsed once via `GraphDb` (GRAPH-001) and the resulting type-erased IR is what
  the bundler consumes.
- **I-5 (one resolver)** [`resolver-parity`]: the bundler resolves *identically* to the runtime and the LSP —
  it routes every specifier through `meow_loader::Resolver::locate` (LOAD-003), never a second resolver and
  never `node_modules`.

The remaining two refs are the honest-engineering constraints Rolldown forces on us:

- **I-10 (small footprint; upstream)** [`footprint`]: Rolldown is a large Rust crate tree; integrating it
  must be a deliberate, quantified footprint decision (feature-gated), not an accidental dependency blob.
- **I-11 (honest claims)** [`honesty`]: Rolldown performs its **own** internal parse of the source we hand
  it (the public plugin `load` hook takes source text, not an Oxc AST). meow's "parse once" is true for
  **meow's** toolchain pipeline; it is not "every file is parsed exactly once inside the entire binary," and
  the docs/output must say so. The toolchain "10×" claim (CANON §24.1) never attaches to Rolldown's
  bundling compute.

This is also the I-6 [`determinism`] surface for build artifacts: same `source + lockfile + meow version +
options` ⇒ **byte-identical bundle** (the core bet, CANON §1).

Traceability: `plan_ref` → PLAN.md *Build sequence · P3* + *backlog later-phases* (`TOOL-003`, PLAN line 104).
`constitution_ref` → **I-1, I-5, I-6, I-10, I-11** (gates `graph-integrity`, `resolver-parity`,
`determinism`, `footprint`, `honesty`; phase gates for TOOL-003 are `graph-integrity` + `determinism`).

## Acceptance criteria

Concrete, checkable Rust. A new workspace crate `crates/bundle` (`meow-bundle`), gated behind a default-on
Cargo feature, plus the CLI flip of the existing honest `bundle` stub. Every user-triggerable path returns
`Result`/`Diagnostic` — the bundler never panics on user input, a malformed module, an unresolved specifier,
or a Rolldown build error (CRAFT Part B). The host-write (emitting files) and all ambient reads live at the
CLI edge; the library is host-pure (I-6 / P16).

### The architecture: meow resolves + parses; Rolldown bundles (I-1, I-5)

Rolldown's `rolldown_plugin::Plugin` is `Send + Sync` and its `resolve_id`/`load` hooks return
`impl Future + Send`. `GraphDb` (Rc/RefCell/`self_cell` over an Oxc `Allocator`) and `Resolver`
(`RefCell<HashMap<…, Arc<PackageFs>>>`) are **`!Send`/`!Sync`** — a plugin holding the live graph or resolver
**will not compile**. The resolution of this is also the *correct* expression of I-1/I-5:

meow does a **graph-walk pre-pass** on the main thread — resolving every edge once through the one
`Resolver` and parsing every module once through the shared `GraphDb` — producing a `Send + Sync`
`BundleInputs` snapshot. Rolldown then bundles with a plugin that is a **pure lookup adapter** over that
snapshot: `resolve_id` returns the already-resolved id, `load` returns the already-parsed, type-erased code.
Rolldown's built-in `rolldown_resolver::Resolver` is **structurally never reached** (every `resolve_id`
returns `Ok(Some(_))`), and meow's own pipeline parses each file exactly once.

```text
crates/bundle/
  Cargo.toml          # rolldown (feature "bundle"), meow-graph, meow-loader, meow-diag,
                      # meow-driver, oxc_ast (read import edges off the shared CST — no parser),
                      # deno_core (Url), tokio, thiserror
  src/lib.rs          # Bundler facade + BundleOptions/BundleArtifact + public re-exports
  src/walk.rs         # graph-walk pre-pass: resolve-once + parse-once -> BundleInputs (Send + Sync)
  src/plugin.rs       # MeowInputsPlugin: rolldown Plugin adapter over Arc<BundleInputs>
  src/options.rs      # BundleOptions -> rolldown BundlerOptions mapping
  src/error.rs        # BundleError (thiserror) + BundleError -> meow_diag::Diagnostic
  tests/bundle.rs     # integrity, parity, determinism, CLI surface, robustness
```

### `crates/bundle/src/lib.rs` — the facade

```rust
use std::path::PathBuf;
use std::sync::Arc;
use deno_core::url::Url;
use meow_diag::Diagnostic;
use meow_driver::ToolHost;            // shared parse-once driver (owns ONE GraphDb + ONE Resolver)

/// One bundling request. Pure inputs — no ambient host state (I-6/P16). `entries` are already
/// canonicalized to `file:` URLs by the CLI edge; the library never touches the cwd.
pub struct BundleOptions {
    pub entries: Vec<Url>,
    pub output: OutputTarget,
    pub format: OutputFormat,
    pub minify: bool,
    pub sourcemap: SourceMap,
}

/// Single-file vs. chunk-directory output. Names only; the library returns bytes, the CLI writes them.
pub enum OutputTarget {
    /// `--outfile <path>`: one self-contained file. Errors if the graph requires code-splitting
    /// (a dynamic `import()` that cannot be inlined) — see `BundleError::SplitRequiresOutDir`.
    File(PathBuf),
    /// `--out <dir>`: one entry chunk per entry + shared/dynamic chunks (Rolldown's automatic splitting).
    Dir(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat { Esm, Cjs, Iife }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SourceMap { None, External, Inline }

/// In-memory result of `Bundler::run`. The CLI edge writes `files` to disk and renders `warnings`.
pub struct BundleArtifact {
    /// Emitted chunks + assets, in a deterministic, sorted order (I-6).
    pub files: Vec<EmittedFile>,
    /// Non-fatal diagnostics (e.g. an unused-import note Rolldown surfaced) in the shared model.
    pub warnings: Vec<Diagnostic>,
}

pub struct EmittedFile {
    /// Output-relative path (the chunk/asset filename Rolldown computed); never an absolute host path.
    pub path: PathBuf,
    pub bytes: Arc<[u8]>,
}

pub struct Bundler<'a> {
    host: &'a mut ToolHost,
    options: BundleOptions,
}

impl<'a> Bundler<'a> {
    pub fn new(host: &'a mut ToolHost, options: BundleOptions) -> Bundler<'a>;

    /// Resolve-once + parse-once pre-pass over the shared graph, then bundle with Rolldown.
    /// Async because Rolldown's `generate()` is async; the CLI drives it on a current-thread runtime.
    /// Returns IN-MEMORY bytes — does NOT write to disk (I-6/P16: the CLI edge owns host writes).
    pub async fn run(self) -> Result<BundleArtifact, BundleError>;
}

pub use crate::error::BundleError;
```

`ToolHost` is the P3 shared parse-once driver (proposed over IRC; see Operator notes). Its load-bearing
contract for this spec is exactly the pair `MeowModuleLoader::new` already takes —
`Rc<RefCell<meow_graph::GraphDb>>` + `meow_loader::Resolver` — so the runtime, lint, format, check, and
bundle all share **one** `GraphDb` instance and **one** `Resolver` per invocation. If the group keeps the
driver minimal, `Bundler::new` instead takes `(graph: Rc<RefCell<GraphDb>>, resolver: &Resolver)` directly;
the algorithm below is identical either way.

### `crates/bundle/src/walk.rs` — resolve-once + parse-once pre-pass (the I-1/I-5 core)

```rust
use std::collections::BTreeMap;
use std::sync::Arc;
use deno_core::url::Url;

/// The `Send + Sync` snapshot the Rolldown plugin reads. Built entirely on the main thread via the
/// shared GraphDb (parse-once) + the one Resolver (resolve-once); contains no Rc/RefCell/Oxc handle.
pub struct BundleInputs {
    /// resolved module id (URL string) -> its already-parsed, type-erased source (stage-5 IR).
    pub modules: BTreeMap<String, ModuleInput>,
    /// the resolved ids of the entry modules, in input order.
    pub entries: Vec<String>,
}

pub struct ModuleInput {
    /// Type-erased, V8-/bundler-ready source from `GraphDb::runtime_ir` (annotations blanked, positions
    /// preserved). This is what Rolldown re-parses internally for tree-shaking + codegen.
    pub code: Arc<str>,
    /// (raw specifier as written -> resolved id) for every *value-level* static import/export and dynamic
    /// `import()` in this module. Precomputed via `Resolver::locate`, so the plugin never re-resolves.
    /// Type-only imports (`import type`, `import_kind == Type`) are excluded — they are already gone from
    /// `code`.
    pub edges: BTreeMap<String, EdgeTarget>,
}

/// What an import edge points at after resolution through the one Resolver.
pub enum EdgeTarget {
    /// A first-party file or a cached package member — bundled in.
    Module(String),
    /// A `meow:*` native module — runtime-provided; kept as an external import in the output (see FORK).
    External(String),
}

/// BFS from `entries`: for each module, read its bytes via `Resolver::resolve`, feed `GraphDb::set_file`
/// (parse-once) -> `runtime_ir` (erased), extract its value-level import specifiers off the shared `Cst`,
/// resolve each via `Resolver::locate`, enqueue unseen module targets. Deterministic order (BTreeMap +
/// FIFO queue). Never constructs an Oxc parser (I-1) and never a second resolver (I-5).
pub(crate) fn collect_modules(
    host: &mut ToolHost,
    entries: &[Url],
) -> Result<BundleInputs, BundleError>;
```

Edge extraction reads `GraphDb::cst(fid).program()` and walks `oxc_ast::ast::{ImportDeclaration,
ExportNamedDeclaration, ExportAllDeclaration, ImportExpression}` for their `source` string, skipping any
with `import_kind`/`export_kind == ImportOrExportKind::Type` (those are erased). This is reading the shared
parse, not a new one — `Parser::new` never appears in `crates/bundle` (P15). A `ModuleKind::Cjs` dependency
target is a hard `BundleError::CjsDependencyUnsupported` (the LOAD-004 boundary, honest I-11); a non-erasable
TS construct in any module surfaces the `GraphDb` strip `Diagnostic` (the `runtime_ir(fid)` `Err` branch),
never a panic.

### `crates/bundle/src/plugin.rs` — the Rolldown adapter (I-5 seam)

```rust
use std::sync::Arc;
use std::borrow::Cow;
use rolldown::plugin::{
    HookLoadArgs, HookLoadOutput, HookLoadReturn, HookResolveIdArgs, HookResolveIdReturn,
    HookResolveIdOutput, Plugin, PluginContext, SharedLoadPluginContext,
};
use crate::walk::{BundleInputs, EdgeTarget};

/// Pure lookup over the precomputed `BundleInputs`. `Send + Sync` (only `Arc` + owned data). Because it
/// answers EVERY `resolve_id`, Rolldown's built-in `rolldown_resolver::Resolver` is never consulted — the
/// single-resolver invariant (I-5) holds by construction.
#[derive(Debug)]
pub(crate) struct MeowInputsPlugin {
    inputs: Arc<BundleInputs>,
}

impl Plugin for MeowInputsPlugin {
    fn name(&self) -> Cow<'static, str> { Cow::Borrowed("meow:inputs") }

    async fn resolve_id(&self, _ctx: &PluginContext, args: &HookResolveIdArgs<'_>)
        -> HookResolveIdReturn
    {
        // entry (no importer) -> the specifier is already a resolved entry id.
        // otherwise -> the importer's precomputed edge for this raw specifier.
        // `EdgeTarget::External` => HookResolveIdOutput { external: Some(true), .. } (kept as-is).
        // a specifier with no precomputed edge is an internal invariant break, returned as an error
        // (never Ok(None), which would fall through to Rolldown's own resolver).
    }

    async fn load(&self, _ctx: SharedLoadPluginContext, args: &HookLoadArgs<'_>)
        -> HookLoadReturn
    {
        // return Ok(Some(HookLoadOutput { code: inputs.modules[args.id].code.clone().into(), .. }))
        // -> Rolldown bundles the already-parsed, type-erased source; it never reads disk for a meow module.
    }
}
```

### `crates/bundle/src/options.rs` — meow → Rolldown options

`BundleOptions` maps onto `rolldown::BundlerOptions` (consumed, not redefined):

- `entries` → `input: Some(vec![InputItem { import: <entry id>, .. }, …])`.
- `OutputTarget::File(p)` → `file: Some(p)`; `OutputTarget::Dir(d)` → `dir: Some(d)`.
- `OutputFormat::{Esm,Cjs,Iife}` → `format: Some(rolldown OutputFormat::{Esm,Cjs,Iife})`.
- `minify` → `minify: Some(RawMinifyOptions::Bool(true|false))` (Rolldown drives the Oxc minifier).
- `SourceMap::{None,External,Inline}` → `sourcemap: None | Some(SourceMapType::File) | Some(SourceMapType::Inline)`.
- Determinism pins (I-6): `platform: Some(Platform::Neutral)` (no host-platform branching), a fixed
  `entry_filenames`/`chunk_filenames` template (content-hash, stable `hash_characters`), and
  `sourcemap_path_transform` that rewrites any absolute path to a project-relative or `meow-cache://`
  stable id so no host path/timestamp leaks into output.

Then: `BundlerBuilder::default().with_options(opts).with_plugins(vec![plugin.into()]).build()?` and
`bundler.generate().await?` → `rolldown::BundleOutput` → `BundleArtifact` (map each `Output::{Chunk,Asset}`
via `.filename()` + `.content_as_bytes()` into an `EmittedFile`, sorted by filename for determinism).
`generate()` (in-memory) is used, **not** `write()` — the library stays host-pure; the CLI writes the bytes.

### `crates/bundle/src/error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("resolving {specifier:?} from {referrer}: {source}")]
    Resolve { specifier: String, referrer: Url, #[source] source: meow_loader::ResolveError },
    #[error("entry {0} does not exist or is not readable")]
    EntryNotFound(PathBuf),
    #[error("module {member:?} of {package:?} is CommonJS; CJS dependencies cannot be bundled until LOAD-004")]
    CjsDependencyUnsupported { package: String, member: String },
    #[error("{count} module(s) could not be lowered (type-strip/parse errors); see diagnostics")]
    Lowering { count: usize, diagnostics: Vec<Diagnostic> },
    #[error("--outfile cannot emit a code-split graph (a dynamic import forced splitting); use --out <dir>")]
    SplitRequiresOutDir,
    #[error("rolldown: {0}")]
    Rolldown(String),   // rolldown_error::BuildError flattened to a message + a Diagnostic per error
}

impl BundleError {
    /// Lower to the shared model so the CLI/LSP render bundle errors identically to lint/format/check.
    pub fn to_diagnostics(&self) -> Vec<Diagnostic>;
}
```

### CLI surface (`crates/cli/src/cli.rs`, fenced `// === TOOL-003 ===`)

Extend the existing `BundleArgs` (today: `entries: Vec<PathBuf>` + `out: Option<PathBuf>`) and flip the
stub. Exact surface:

```
meow bundle <ENTRY>...                # one or more entry modules
            [-o, --out <DIR>]         # chunk-directory output (existing flag; required if splitting)
            [--outfile <FILE>]        # single-file output (conflicts with --out)
            [--format <esm|cjs|iife>] # default: esm
            [--minify]                # default: off
            [--sourcemap[=<external|inline>]]  # bare => external; default (absent): none
```

```rust
// === TOOL-003 ===
#[derive(Debug, Args)]
pub struct BundleArgs {
    #[arg(required = true)]
    pub entries: Vec<PathBuf>,
    #[arg(long, short)]
    pub out: Option<PathBuf>,
    #[arg(long, conflicts_with = "out")]
    pub outfile: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = BundleFormat::Esm)]
    pub format: BundleFormat,
    #[arg(long)]
    pub minify: bool,
    /// `--sourcemap` (bare) = external `.map`; `--sourcemap=inline`; absent = none.
    #[arg(long, value_name = "MODE", num_args = 0..=1, require_equals = true,
          default_missing_value = "external")]
    pub sourcemap: Option<SourceMapArg>,
}

#[derive(Debug, Clone, ValueEnum)] pub enum BundleFormat { Esm, Cjs, Iife }
#[derive(Debug, Clone, ValueEnum)] pub enum SourceMapArg { External, Inline }
// === /TOOL-003 ===
```

Dispatch: a `Command::Bundle(args) => cmd_bundle(&args)` arm added before the catch-all stub (mirroring
the RT-001/CFG-001/PKG-002 flips); `Command::landing()` keeps the `("bundle", "P3")` table entry (the
per-command test table) but `bundle` is no longer routed to the stub. `cmd_bundle` is the host edge: it
canonicalizes each entry to a `file:` URL, builds the `ToolHost` (the same `ResolutionGraph` →
`Resolver::from_resolution` + shared `GraphDb` wiring `cmd_run` uses), runs `Bundler::run` on a
current-thread tokio runtime, then **writes** `BundleArtifact.files` to disk and renders `warnings` /
typed errors to stderr (exit 0 on success; non-zero on `BundleError`).

### Behaviors (the acceptance bar)

- `meow bundle src/main.ts --outfile dist/app.js` type-strips `main.ts` and its transitive first-party +
  cached-dependency ESM through the one pipeline and emits a single ESM file at `dist/app.js`; no
  `node_modules` is read or created; the output contains the bundled, type-erased code.
- `--format cjs` / `--format iife` change only the wrapper; `--minify` runs Rolldown's Oxc minifier;
  `--sourcemap` emits `dist/app.js.map` (external) or an inline `//# sourceMappingURL=data:` comment.
- A bare-specifier import of a **cached dependency with conditional exports** resolves to the *same* member
  URL `meow run` resolves (the one `Resolver`), and the dependency is bundled in.
- A `meow:*` import is **kept external** in the output (the bundle runs on the meow runtime that provides
  it) — not inlined, not an error (FORK below).
- A `.cjs` / `"type":"commonjs"` dependency in the graph → `BundleError::CjsDependencyUnsupported` (the
  honest LOAD-004 boundary), not a silent skip.
- Bundling the same project twice → **byte-identical** `files` + `.map` (I-6); no absolute host path,
  cwd, or timestamp appears in any emitted byte; `sources` entries are stable project-relative /
  `meow-cache://` ids.
- An unresolved bare specifier → `BundleError::Resolve` carrying the underlying `ResolveError`
  (fix: `meow install`); a non-erasable TS construct → the strip `Diagnostic`; a missing entry →
  `EntryNotFound`. Every path is a typed error, never a panic.

## Non-goals

- **Lint / format / typecheck** — TOOL-001 (formatter), TOOL-002 (linter), CHK-001/002 (check). `meow bundle`
  does **not** gate on type errors or lint findings; it only requires the source to be *strippable*
  (erasable-only, I-3, enforced by the shared `GraphDb`).
- **CJS dependency bundling** — classified + boundary-errored here; synthetic-ESM wrapping is LOAD-004 (P5).
  First-party CJS stays refused upstream (I-2).
- **Manual chunking / `manualChunks`, multi-format-in-one-run, library `dts` bundling, CSS/asset/binary
  loaders, plugin ecosystem** — out of P3. P3 ships JS/TS/JSON modules, automatic dynamic-import splitting
  only (see FORK).
- **Watch-mode bundling** (`meow dev`-driven rebundle) — P3 is batch; Rolldown's watch/HMR hooks are unused.
- **Injecting the shared Oxc AST into Rolldown** (avoiding Rolldown's internal re-parse) — the public plugin
  `load` hook takes source text; the experimental `transform_ast` hook could carry an AST but crosses the
  allocator/lifetime boundary. Deferred (FORK); P3 accepts the second parse honestly (I-11).
- **A native bundler** — ADR-2 is explicit: reuse Rolldown, do not reimplement it.

## Interface

Public surface of `meow_bundle`:
- `Bundler::new(&mut ToolHost, BundleOptions) -> Bundler` + `Bundler::run(self) -> Result<BundleArtifact, BundleError>` (async).
- `meow_bundle::{BundleOptions, OutputTarget, OutputFormat, SourceMap, BundleArtifact, EmittedFile, BundleError}`.
- `BundleInputs`/`ModuleInput`/`EdgeTarget`/`MeowInputsPlugin` stay `pub(crate)` (no external consumer).

Consumes (does not redefine):
- `meow_graph::{GraphDb, FileId, Cst, RuntimeIr, StripDiagnostic}` (parse-once; GRAPH-001).
- `meow_loader::{Resolver, ResolveError, ModuleKind, ModuleLocator}` (resolve-once; LOAD-003).
- `meow_diag::{Diagnostic, Severity, DiagCode, Related}` (the P3 shared diagnostics model — IRC contract).
- `meow_driver::ToolHost` (the P3 shared parse-once driver — IRC contract; underlying pair is
  `Rc<RefCell<GraphDb>>` + `Resolver`).
- `rolldown::{BundlerBuilder, Bundler as RolldownBundler, BundlerOptions, BundleOutput, Output, OutputFormat,
  SourceMapType, RawMinifyOptions, Platform, plugin::*}`, `oxc_ast::ast::{ImportDeclaration,
  ExportAllDeclaration, ExportNamedDeclaration, ImportExpression}`.

CLI: extend `BundleArgs`, add `cmd_bundle`, flip the `Bundle` dispatch arm (all fenced `// === TOOL-003 ===`).

New crate deps flagged for I-10: `rolldown` (+ its tree: `rolldown_common`, `rolldown_plugin`,
`rolldown_ecmascript`, `rolldown_sourcemap`, `rolldown_utils`, and notably `rolldown_resolver` which we
**do not call**). Oxc (`oxc_parser`/`oxc_semantic`/`oxc_transformer`/`oxc_codegen`) is already a workspace
dep via `meow-graph`, so Rolldown shares it; the **marginal** footprint is Rolldown's own crates + their
non-Oxc transitive deps — Rust code only, **no new C++/V8** (the ≤ 60 MB budget hog is V8, ADR-1). Behind a
Cargo feature `bundle` (default-on for the shipped binary; excludable for minimal/footprint builds — FORK).

## Implementation sketch

New workspace member `crates/bundle`. No edits to `meow-graph`/`meow-loader` (they already expose
everything needed: `Cst::program()`, `runtime_ir`, `Resolver::{locate,resolve}`). Shared-file edits are
confined to the CLI, fenced `// === TOOL-003 ===`:

- `crates/cli/Cargo.toml` — add `meow-bundle` under a `bundle` feature (default-on); the feature also gates
  the dispatch arm so a `--no-default-features` build keeps the honest stub.
- `crates/cli/src/cli.rs` — fence the extended `BundleArgs` + `BundleFormat`/`SourceMapArg`, the
  `Command::Bundle(args) => cmd_bundle(&args)` arm (under `#[cfg(feature = "bundle")]`; the `#[cfg(not)]`
  path stays in the catch-all stub), and the new `cmd_bundle` host edge (canonicalize entries, build the
  `ToolHost`, `tokio` current-thread `block_on(Bundler::run)`, write `files`, render `warnings`/errors).
- `crates/cli/src/cli.rs` `mod tests` — fence: the `bundle` command's stub-table assertion becomes a
  real-run assertion (a fixture entry bundles; with `--no-default-features` it still stubs).

Flow (one invocation, one graph, one resolver):
`cmd_bundle` → `ToolHost{ GraphDb, Resolver }` → `Bundler::run` →
`walk::collect_modules` (BFS: `Resolver::resolve` bytes → `GraphDb::set_file`/`runtime_ir` erased →
read import edges off `Cst` → `Resolver::locate` each → `BundleInputs`) →
`BundlerBuilder::with_options(map(opts)).with_plugins([MeowInputsPlugin{inputs}]).build()` →
`generate().await` → `BundleArtifact` → CLI writes bytes. Rolldown sees only precomputed ids + erased
source; its resolver/disk-loader are never reached.

## Tests required

`crates/bundle/tests/bundle.rs` (integration) + unit tests in `walk.rs`/`options.rs`/`error.rs`. Builds
fixtures in a temp dir + an in-memory cache/lockfile (reusing LOAD-003's corpus helpers). Proves **I-1**
[`graph-integrity`], **I-5** [`resolver-parity`], **I-6** [`determinism`] and backs **I-10** [`footprint`].

- **Parse-once / graph-integrity (I-1).** Bundle a TS entry that imports a relative TS module; assert both
  files are present in the shared `GraphDb` and each was parsed exactly once (`db.recomputes()` shows one
  `cst`/`runtime_ir` per file); assert the emitted bundle is type-erased (no TS annotations; a marker
  identifier survives at its original position). Structural: `crates/bundle` contains no `Parser::new`
  (P15 floor) and the only resolution call is `meow_loader::Resolver`.
- **One resolver / resolver-parity (I-5).** Over a corpus (relative + bare cached dep with conditional
  exports + self-reference + `#imports`), assert every edge resolves to the **same** URL `meow run`'s loader
  yields for the same corpus (shared fixture with LOAD-003's `resolver-parity` corpus). Assert no path
  containing `node_modules` is opened, and that the `MeowInputsPlugin` answers every `resolve_id`
  (Rolldown's built-in resolver is never invoked — a test plugin counter / or assert no disk read for a
  resolved id).
- **Determinism (I-6).** Bundle the same project twice from different cwds / scrambled `$HOME` → identical
  `files` bytes + identical `.map`; grep the output bytes for an absolute host path / the cwd / a
  4-digit-year timestamp → none; `sources` are stable ids. (This is the `determinism` gate fixture.)
- **CLI surface.** `--outfile` → one file; `--out <dir>` with a dynamic `import()` → multiple chunks;
  `--outfile` with a forced split → `SplitRequiresOutDir`; `--format cjs|iife` switch the wrapper;
  `--minify` shrinks output; `--sourcemap` / `--sourcemap=inline` emit external/inline maps; defaults
  (esm, no-minify, no-sourcemap) hold.
- **Robustness (no panic).** Unresolved bare specifier → `BundleError::Resolve`; `.cjs` dep →
  `CjsDependencyUnsupported`; non-erasable TS (`enum`) in a module → `Lowering` carrying the strip
  `Diagnostic`; missing entry → `EntryNotFound`; a Rolldown build error → `BundleError::Rolldown` +
  diagnostics. Each is a typed error, never a panic.
- **Footprint (I-10).** With `--no-default-features` the `meow bundle` arm is the honest stub and the binary
  excludes the Rolldown crate tree; the release-binary delta (feature on vs off) is measured by the
  `footprint` gate at integration via `scripts/footprint.sh`.

GATES: `graph-integrity` (I-1) + `determinism` (I-6) are the TOOL-003 phase gates; `resolver-parity` (I-5),
`footprint` (I-10), and `honesty` (I-11) also apply. Floor: `cargo test/clippy/fmt`, `principles-check.sh`.

## Rollout

New workspace crate behind a default-on `bundle` Cargo feature + a CLI stub→real flip. No persisted state,
no migration; output files are user artifacts (never committed). The cache + lockfile stay read-only to this
crate (PKG owns writes). The library is host-pure (`generate()` in memory; the CLI edge writes), so it adds
no new ambient-read surface (I-6/P16).

**Reversibility:** drop the `meow-bundle` dep + `bundle` feature and revert the `// === TOOL-003 ===` CLI
fences — the honest `bundle` stub returns, no data to roll back. The Rolldown version is pinned exactly (like
the Oxc/V8 pinning discipline, CANON §24.8); a bad bump is a one-line revert.

## Operator notes

Integration points (named):
- **GRAPH-001:** `GraphDb::{set_file, cst, runtime_ir}` + `Cst::program()` — consumed unchanged; bundling
  parses through this, never `oxc_parser` (I-1).
- **LOAD-003:** `Resolver::{locate, resolve}`, `ModuleKind`, `ModuleLocator`, `ResolveError` — the one
  resolver; the bundle walk shares LOAD-003's `resolver-parity` corpus as its parity fixture.
- **CLI (DIST-001 / PKG-003):** reuses `cmd_run`'s `ResolutionGraph::assemble` → `Resolver::from_resolution`
  + shared `GraphDb` wiring to build the `ToolHost`; flips the `Bundle` stub.

Forks (local defaults — flagged for Main to consolidate into `decisions.json`; NOT written here):
- **FORK[TOOL-003:rolldown-integration]** link the `rolldown` crate (published on crates.io) and drive it via
  `BundlerBuilder` + a meow `Plugin` (virtual-modules pattern: `resolve_id` → the one Resolver, `load` → the
  shared GraphDb's erased IR) — chosen over a subprocess because ADR-2 mandates *library* reuse and a
  subprocess breaks the single-binary distribution (CANON §26.1).
- **FORK[TOOL-003:send-sync-prepass]** resolve+parse in a main-thread pre-pass into a `Send + Sync`
  `BundleInputs`, with the plugin a pure lookup adapter — chosen because Rolldown's `Plugin` is `Send + Sync`
  while `GraphDb`/`Resolver` are `!Send`; this is also the cleanest expression of I-1/I-5 (meow resolves +
  parses once; Rolldown bundles). The alternative (a `!Send` plugin) does not compile.
- **FORK[TOOL-003:footprint-gate]** gate `meow-bundle` behind a default-on Cargo feature `bundle` — chosen so
  footprint-sensitive/minimal builds can drop Rolldown's crate tree. Marginal footprint is Rust-only (Oxc
  already shared, no new V8/C++); the exact delta is quantified at integration by the `footprint` gate.
- **FORK[TOOL-003:double-parse]** accept Rolldown's internal re-parse of the erased source — chosen because
  the public `load` hook returns source text, not an Oxc AST; AST injection via the experimental
  `transform_ast` hook is deferred (allocator/lifetime boundary). Honest claim (I-11): "parsed once for
  meow's toolchain; the bundler engine parses again." Never advertise "every file parsed exactly once."
- **FORK[TOOL-003:defaults]** format=esm, minify=off, sourcemap=off by default — chosen because ESM is the
  first-party authoring + output target (ADR-3) and unminified/no-map is the least-surprising dev default
  (matches esbuild/Rolldown).
- **FORK[TOOL-003:output-layout]** `--outfile <file>` = single-file bundle (default when one entry needs no
  split); `--out <dir>` = chunk directory — chosen to mirror esbuild's `--outfile`/`--outdir` mental model.
- **FORK[TOOL-003:code-splitting]** P3 scope = automatic chunking on dynamic `import()` only (Rolldown
  default) when `--out <dir>`; manual chunks deferred — chosen to ship the common path; `--outfile` errors
  (`SplitRequiresOutDir`) rather than silently producing multiple files.
- **FORK[TOOL-003:meow-natives]** `meow:*` imports → marked `external` (retained in output), since bundles
  run on the meow runtime that provides them; inlining is impossible (they are runtime ops) and erroring
  breaks the common case — flagged for Main.
- **FORK[TOOL-003:import-edge-source]** extract import edges with a local walker over the shared `Cst` for
  P3, rather than promoting it to a shared `GraphDb` stage-4 query — chosen to keep blast radius small;
  flag for promotion (a shared `imports(FileId)` query would let workspace/why-large/tree-shaking reuse it).
- **FORK[TOOL-003:rolldown-version]** pin an exact `rolldown` crate version (pre-1.0, fast-moving — track
  upstream like §24.8 V8 discipline); fall back to a git pin only if the published crate API lags. Flagged.

Shared P3 contract (proposed by TOOL-003 over IRC; for Main to consolidate after all drafters land):
- **`Diagnostic` home:** a new leaf crate `crates/diag` (`meow-diag`) depending on `meow-graph` only for
  `FileId` (so meow-graph has no reverse dep — no cycle); `From<&meow_graph::StripDiagnostic>` lives in
  meow-diag. Fields: `Diagnostic { file: FileId, span: Span, severity: Severity, code: DiagCode,
  message: String, help: Option<String>, related: Vec<Related> }`; `Severity { Error, Warning, Info, Hint }`
  (1:1 LSP `DiagnosticSeverity`); `DiagCode(&'static str)`. Lint/format/check/bundle all emit this.
- **Parse-once driver:** `ToolHost` owns the one `GraphDb` + one `Resolver` for an invocation; tools borrow
  it to run over the same parsed files. The load-bearing requirement for TOOL-003 is just the
  `Rc<RefCell<GraphDb>>` + `Resolver` pair (the `MeowModuleLoader::new` shape). If the group names/places it
  differently, only `Bundler::new`'s argument type changes — the algorithm is unaffected.

Honest coverage vs. a full bundler (report): COVERED — entry/relative/bare-cached/self-ref/`#imports`
resolution via the one Resolver, type-stripped (erasable-only) modules via the one GraphDb, ESM/CJS/IIFE
output, minify, external/inline sourcemaps, automatic dynamic-import code-splitting, deterministic
byte-identical output. GAPS (honest, by design): Rolldown re-parses internally (I-11 boundary); no CJS-dep
bundling (LOAD-004), no manual chunks / CSS/asset loaders / plugin ecosystem / watch mode / `dts` bundling;
`meow:*` kept external. The double-parse is an acknowledged engine boundary, not a parse-once violation of
meow's own pipeline.
