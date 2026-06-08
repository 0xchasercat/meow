---
spec_id: TOOL-001
title: "meow fmt — deterministic, idempotent formatter over the shared CST"
subsystem: crates/fmt
status: drafting
blast_radius: medium
plan_ref: "Build sequence · P3 Parse-Once Toolchain; backlog P3 (TOOL-001 formatter)"
constitution_ref: [I-1, I-6, I-10, I-11]
depends_on: [GRAPH-001, DIAG-001]
estimate: medium
backend: omp
---

## Motivation

P3 consolidates lint/format/check/bundle on the **already-existing shared graph** (PLAN P3:
*"a file is parsed once and reused across multiple tool operations in one invocation"*).
`meow fmt` is the formatter half: a deterministic, idempotent pretty-printer that reuses
the stage-1 **lossless CST** (`meow_graph::Cst`) GRAPH-001 already produces and **constructs
no parser of its own** (I-1, `graph-integrity`; CANON §7.1 — *"Lossless at stage 1 is
non-negotiable: the formatter needs trivia"*; CANON §19 `meow fmt`).

The CST's `write_source` is **verbatim retention** (it re-emits the original bytes — the
round-trip proof), explicitly *not* a re-serializer: `crates/graph/src/cst.rs` states *"A
structural re-serializer (formatter) is a separate, additive stage"* and *"structural re-emit
is the formatter's job and is deliberately out of scope here."* TOOL-001 **is** that additive
stage. It walks the shared `Cst::program()` (the Oxc AST + comment trivia + spans) and prints
formatted source through Oxc's own Prettier-compatible engine (`oxc_formatter`, the same Oxc
generation already pinned for GRAPH-001 — reuse, never rebuild, ADR-2 / footprint I-10).

Determinism is the contract (I-6, `determinism`): the formatter is pure over `(source, options)`
— no host env/clock/randomness (P16), line endings pinned to LF, options caller-supplied — so the
same file formats to the same bytes on any machine, and `fmt(fmt(x)) == fmt(x)`. `--check` reports
through the **shared `Diagnostic` model** (DIAG-001) without rewriting; the formatter never sells
a guarantee it does not enforce (I-11) — it refuses to rewrite a file it cannot fully parse.

Traceability: `plan_ref` → PLAN.md *Build sequence · P3* + the P3 backlog (`TOOL-001`).
`constitution_ref` → **I-1** [`graph-integrity`] (one parse pipeline; fmt reuses `GraphDb::cst`,
builds no parser), **I-6** [`determinism`] (byte-stable, idempotent, host-independent),
**I-10** [`footprint`] (the `oxc_formatter` dep is flagged + co-versioned), **I-11** [`honesty`]
(`--check` is honest; unparseable files are reported, never silently mangled).

## Acceptance criteria

Concrete, checkable Rust. New crate **`crates/fmt`** (`meow-fmt`). Every reachable failure is
surfaced as a typed `meow_diag::Diagnostic` (the shared tool error channel; `DiagCode` is the
stable discriminant) — the library has no `unwrap`/`expect`/`panic!` on file/user paths (CRAFT
Part B). It is **host-pure** (I-6/P16): source, source-type, and options are caller-supplied;
all FS/stdin/env access lives at the CLI edge.

### `crates/fmt/src/lib.rs` — the formatter (host-pure; reuses the shared CST, I-1)

```rust
use std::path::Path;
use std::sync::Arc;

use meow_diag::{DiagCode, Diagnostic, Severity};
use meow_graph::{Cst, FileId, GraphDb};
use oxc_allocator::Allocator;
use oxc_formatter::{format_program, JsFormatOptions};
use oxc_formatter_core::{IndentStyle, IndentWidth, LineEnding, LineWidth};
use oxc_span::Span;

/// The resolved formatter style: the curated "meow" preset + the two exposed knobs.
/// Built by the CLI edge from `MeowConfig.format`; the library NEVER reads config or
/// host state itself (I-6 / P16 — caller-supplied). `Copy`, so it threads cheaply
/// through a batch with no allocation.
#[derive(Debug, Clone, Copy)]
pub struct FmtOptions {
    /// Wrap target. CANON §18 default 100. Clamped to the engine's legal range.
    pub line_width: u16,
    /// Spaces per indent level. Default 2.
    pub indent_width: u8,
}

impl Default for FmtOptions {
    fn default() -> Self {
        Self::meow()
    }
}

impl FmtOptions {
    /// The shipped "meow" style (CANON §18 `format.style: "meow"`): width 100, indent 2.
    pub const fn meow() -> Self {
        Self { line_width: 100, indent_width: 2 }
    }

    /// Lower onto the engine. Indent STYLE (spaces), quote style (double), semicolons,
    /// trailing commas, and bracket spacing are the FIXED meow preset (= `JsFormatOptions`
    /// defaults); only width + indent are tunable (see Operator notes: config-surface fork).
    /// `line_ending` is pinned to LF and is NEVER host-derived, so output is byte-identical
    /// across machines (I-6). Out-of-range knobs fall back to the engine default — never panic.
    fn to_js(self) -> JsFormatOptions {
        JsFormatOptions {
            line_width: LineWidth::try_from(self.line_width).unwrap_or_default(),
            indent_width: IndentWidth::try_from(self.indent_width).unwrap_or_default(),
            indent_style: IndentStyle::Space,
            line_ending: LineEnding::Lf,
            ..JsFormatOptions::new()
        }
    }
}

/// Outcome of formatting ONE file.
pub struct FmtOutcome {
    /// Formatted source. `None` iff formatting was REFUSED (syntax errors, or a printer
    /// failure) — the file is then left byte-for-byte untouched (I-11: never rewrite a
    /// file we cannot fully parse). `Some` on success, even when identical to the input.
    pub code: Option<String>,
    /// `true` when `code` is `Some` AND differs from the input (the file was unformatted).
    pub changed: bool,
    /// Shared-model diagnostics. Empty on a clean, formatted file.
    pub diagnostics: Vec<Diagnostic>,
}

/// THE formatter entry point. Consumes the SHARED parse: it reuses `cst.program()`
/// (the Oxc AST + `cst.comments()` trivia) and constructs NO parser (I-1, P15). Pure
/// over `(cst, opts)`: identical input bytes ⇒ identical output bytes (I-6).
///
/// PRECONDITION: `cst` MUST be a formatter-compatible parse from `GraphDb` (stage-1
/// parsed with `preserve_parens: false`, JSX-on-for-JS — see Implementation sketch).
/// `oxc_formatter::format_program` documents that it panics on a `preserve_parens: true`
/// program; the workspace-wide GraphDb parse-option switch removes that input, so this
/// function cannot panic in the wired system.
pub fn format_cst(cst: &Cst, file: FileId, opts: FmtOptions) -> FmtOutcome {
    // Refuse to format an unparseable file — REPORT, do not rewrite (I-11).
    if cst.panicked() || !cst.errors().is_empty() {
        return FmtOutcome {
            code: None,
            changed: false,
            // `Diagnostic::from_oxc` is the shared OxcDiagnostic→Diagnostic mapper (DIAG-001).
            diagnostics: cst
                .errors()
                .iter()
                .map(|e| Diagnostic::from_oxc(file, e, DiagCode("fmt/syntax-error")))
                .collect(),
        };
    }
    // Scratch arena for the formatter's IR — this is NOT a parse (no `Parser::new`; P15 clean).
    let scratch = Allocator::default();
    let formatted = format_program(&scratch, cst.program(), opts.to_js(), None);
    match formatted.print() {
        Ok(printed) => {
            let code = printed.into_code();
            let changed = code != cst.source();
            FmtOutcome { code: Some(code), changed, diagnostics: Vec::new() }
        }
        Err(err) => FmtOutcome {
            code: None,
            changed: false,
            diagnostics: vec![Diagnostic {
                file,
                span: Span::default(),
                severity: Severity::Error,
                code: DiagCode("fmt/print-failed"),
                message: format!("formatter could not print this file: {err}"),
                related: Vec::new(),
            }],
        },
    }
}

/// Single-file convenience: load `text` into `db` ONCE, then format. Multi-tool
/// invocations instead share one `db` through the parse-once driver (DIAG-001) and
/// call `format_cst` on the already-loaded `Cst`, so the file is parsed exactly once
/// for the whole invocation (the P3 exit condition).
pub fn format_file(db: &mut GraphDb, path: &Path, text: Arc<str>, opts: FmtOptions) -> FmtOutcome {
    let file = db.set_file(path, text);
    match db.cst(file) {
        Some(cst) => format_cst(cst, file, opts),
        // Unreachable: `file` was just minted by `set_file` above. Handled, never panicked.
        None => FmtOutcome { code: None, changed: false, diagnostics: Vec::new() },
    }
}
```

### Behaviors (the acceptance bar)

- **Parse-once reuse (I-1).** `format_cst` reads `cst.program()` from `GraphDb`; it contains
  **no** `oxc_parser::Parser::new` and `meow-fmt` does not depend on `oxc_parser`. The only path
  to a parsed artifact is `GraphDb`. (The `Allocator::default()` it creates is formatter-IR
  scratch, not a parse — P15's grep targets `Parser::new`, which is absent.)
- **Determinism (I-6).** For any file, `format_cst` returns the same `code` bytes regardless of
  host env/locale/clock; LF line endings always; no `HashMap`-iteration-order or float-format
  variance (the engine is deterministic; options are pinned, not host-read).
- **Idempotence (I-6).** `code = format_cst(x)`; re-loading `code` and formatting again yields
  byte-identical `code` (`fmt(fmt(x)) == fmt(x)`), and `changed == false` on the second pass.
- **TypeScript is formatted, not stripped.** A `.ts`/`.tsx` file's type annotations, generics,
  `interface`/`type` declarations, and parameter types are **reformatted in place and retained**
  byte-meaningfully (fmt is orthogonal to RT-003 type-stripping; fmt never erases types). Source
  type is taken from the path (`SourceType::from_path`, already on the `Cst`).
- **Comments & trivia preserved.** Leading/trailing/inline comments, JSDoc blocks, and shebangs
  survive and are re-laid-out by the engine (CANON §7.1 — the formatter is *why* stage 1 is lossless).
- **Refuses to mangle broken input (I-11).** A file with recovered syntax errors (`cst.errors()`
  non-empty) or an unrecoverable parse (`cst.panicked()`) yields `code: None` + an error
  `Diagnostic` per syntax error; the on-disk file is left untouched.
- **`--check` rewrites nothing.** Check mode computes `changed` and emits a diagnostic per
  unformatted file; it never writes.

### Config — minimal, lives in `MeowConfig.format` (CANON §18; extend `meow-config`)

`crates/config/src/schema.rs`'s existing `Format { style: FormatStyle }` is extended with the
**two** tunable knobs only (fenced `// === TOOL-001 ===`); everything else is the fixed `meow`
preset (config-surface fork — Operator notes):

```rust
// crates/config/src/schema.rs   // === TOOL-001 ===
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]   // NOTE: derive(Default) REMOVED
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Format {
    #[serde(default)]
    pub style: FormatStyle,                       // "meow" (the curated preset)
    #[serde(default = "default_line_width")]
    pub line_width: u16,                          // default 100
    #[serde(default = "default_indent_width")]
    pub indent_width: u8,                         // default 2
}

// A manual Default is REQUIRED: `#[derive(Default)]` would make line_width/indent_width 0,
// not 100/2. serde's per-field `default = "..."` covers deserialization of a present-but-partial
// `format` block; this impl covers `Format::default()` (absent block / programmatic construction).
impl Default for Format {
    fn default() -> Self {
        Self { style: FormatStyle::default(), line_width: 100, indent_width: 2 }
    }
}

fn default_line_width() -> u16 { 100 }
fn default_indent_width() -> u8 { 2 }
// === /TOOL-001 ===
```

The CLI edge maps `MeowConfig.format` → `meow_fmt::FmtOptions { line_width, indent_width }`;
a missing/un-evaluatable config falls back to `FmtOptions::meow()`.

### CLI surface — `crates/cli/src/cli.rs` (fenced `// === TOOL-001 ===`)

The existing `Command::Fmt(PathArgs)` stub is replaced by a dedicated `FmtArgs`, and the dispatch
arm flips from the catch-all `EXIT_UNIMPLEMENTED` stub to `cmd_fmt` (clean cutover):

```rust
// crates/cli/src/cli.rs
/// Format over the shared pipeline.
Fmt(FmtArgs),                                   // was: Fmt(PathArgs)

// === TOOL-001 ===
#[derive(Debug, Args)]
pub struct FmtArgs {
    /// Target files/dirs (default: the project root, formatted recursively).
    pub paths: Vec<PathBuf>,
    /// Check mode: write NOTHING; exit non-zero if any target is not already formatted.
    #[arg(long)]
    pub check: bool,
    /// Read source from stdin, write the formatted result to stdout.
    #[arg(long, conflicts_with = "paths")]
    pub stdin: bool,
    /// Source-type hint for `--stdin` (e.g. `a.tsx`); default treats stdin as TypeScript.
    #[arg(long, value_name = "PATH", requires = "stdin")]
    pub stdin_filepath: Option<PathBuf>,
}
// === /TOOL-001 ===
```

Dispatch (in `Cli::run`, before the catch-all `other =>` arm):

```rust
// === TOOL-001 ===
Command::Fmt(args) => cmd_fmt(&args),
// === /TOOL-001 ===
```

`landing()` keeps `Command::Fmt(_) => ("fmt", "P3")` (it matches on the variant, not the inner
type — no change needed). `cmd_fmt` (CLI edge — owns ALL host access; library stays pure):

```rust
// === TOOL-001 ===
/// `meow fmt`: build ONE `GraphDb` for the whole invocation (parse-once across the
/// batch), load each target's text once, format via `meow_fmt`, then write back
/// (or, with `--check`, report). The binary edge owns the ambient reads (cwd, file
/// I/O, stdin) and diagnostic rendering; `meow-fmt` stays host-pure (I-6).
///   exit 0  — everything clean (formatted, or already-formatted in --check)
///   exit 1  — files would change (--check) OR a syntax/print error blocked formatting
fn cmd_fmt(args: &FmtArgs) -> ExitCode { /* … */ }
// === /TOOL-001 ===
```

**Exit-code contract (checkable):**
- `meow fmt` (write mode): formats targets in place; **0** when all targets were formatted
  successfully; **1** if any file was refused (syntax/print error) — its diagnostics print to
  stderr, that file is left unchanged, the rest are still formatted.
- `meow fmt --check`: writes nothing; **0** iff every target is already formatted; **1** if any
  target would change (each listed on stderr) or any was unparseable.
- `meow fmt --stdin [--stdin-filepath a.tsx]`: reads stdin, writes formatted output to stdout,
  **0**; with `--check`, writes nothing and exits **1** if stdin is not already formatted.
- Source-type for `--stdin` comes from `--stdin-filepath` (via `SourceType::from_path`); absent,
  stdin is treated as TypeScript (`.ts`) — the meow default.

## Non-goals

- **Linting / autofix** — TOOL-002. fmt only re-lays-out; it changes no semantics, adds/removes
  no code, and applies no lint fixes. (`fmt` and `lint --fix` are separate passes.)
- **Type stripping / transpiling** — RT-003 / stage 5. fmt **keeps** type annotations; it never
  erases or downlevels. fmt operates on stage 1 (CST), strip on stage 5 (IR) — orthogonal.
- **The shared `Diagnostic` model + parse-once driver themselves** — **DIAG-001** (coordinated).
  TOOL-001 *consumes* `meow_diag::{Diagnostic, Severity, DiagCode, Diagnostic::from_oxc}` and runs
  inside the driver when multiple tools share one invocation; it does not define them.
- **Config evaluation of `meow.config.ts`** — CFG/RT. fmt reads the resolved `MeowConfig` the CLI
  already loads (static `meow.config.json` today; honest fallback to `FmtOptions::meow()` otherwise).
- **Non-JS/TS targets** (CSS/JSON5/YAML/Markdown/HTML that `oxfmt` supports) — out of MVP scope;
  JSON may be added trivially later via `SourceType`, but is not promised here.
- **A second/standalone parser, a hand-rolled Wadler printer, or `oxc_formatter`'s text-in
  `format()`** — all rejected: text-in re-parses (defeats parse-once, I-1); a hand-rolled printer
  rebuilds what the pinned Oxc generation already ships (ADR-2 / footprint). fmt uses the AST-in
  `format_program` over the shared `Cst`.
- **Editor/range formatting, format-on-type** — LSP (LSP-* / DIAG driver), not this CLI spec.
- **Watch mode** — not in scope; `meow fmt` is a one-shot batch.

## Interface

Public surface of `meow-fmt` after TOOL-001:
- `meow_fmt::{FmtOptions, FmtOutcome, format_cst, format_file}` (signatures above).
- `FmtOptions::{meow, default}`; `FmtOutcome { code: Option<String>, changed: bool, diagnostics: Vec<Diagnostic> }`.

Consumes (does NOT redefine):
- `meow_graph::{GraphDb, Cst, FileId}` — `set_file`/`cst`, `Cst::{program, comments, source, errors, panicked, source_type}` (GRAPH-001). **No** Oxc parser is constructed (I-1).
- `meow_diag::{Diagnostic, Severity, DiagCode, Diagnostic::from_oxc}` — the shared model (DIAG-001).
- `oxc_formatter::{format_program, JsFormatOptions}` + `oxc_formatter_core::{IndentStyle, IndentWidth, LineWidth, LineEnding, Printed, PrintError}` — the printing engine; `Formatted::print(self) -> PrintResult<Printed>`, `Printed::into_code(self) -> String`.
- `oxc_allocator::Allocator` (formatter-IR scratch), `oxc_span::Span` (diagnostic spans).

New workspace deps (flag for footprint, I-10): `oxc_formatter`, `oxc_formatter_core` — pinned
**co-versioned with the existing `oxc_* = 0.134` generation** (GRAPH-001 fence). They reuse the
`oxc_ast` already compiled in-tree, so the incremental footprint is the formatter IR + printer,
not a new parser. Declared under a new `# === TOOL-001 ===` fence in `[workspace.dependencies]`.

CLI surface (DIST-001 tree): `meow fmt [PATHS]... [--check] [--stdin] [--stdin-filepath <PATH>]`.

## Implementation sketch

New crate `crates/fmt` (`meow-fmt`); `crates/fmt/Cargo.toml` deps: `meow-graph`, `meow-diag`,
`oxc_formatter`, `oxc_formatter_core`, `oxc_allocator`, `oxc_span` (all `{ workspace = true }`),
`thiserror` (edge errors if any). No `oxc_parser` dep (I-1).

Files: `src/lib.rs` (`FmtOptions`/`FmtOutcome`/`format_cst`/`format_file`), `tests/format_corpus.rs`,
`tests/determinism.rs`.

Shared-file edits — comment-marker fences (`// === TOOL-001 ===` / `// === /TOOL-001 ===`):
- **`Cargo.toml` (workspace)** — new fence pinning `oxc_formatter` + `oxc_formatter_core` co-versioned with the `oxc_* 0.134` set.
- **`crates/cli/src/cli.rs`** — (a) change `Command::Fmt(PathArgs)` → `Fmt(FmtArgs)`; (b) add the `FmtArgs` struct; (c) add the `Command::Fmt(args) => cmd_fmt(&args)` dispatch arm before the catch-all; (d) add `cmd_fmt` (host edge: walk paths / read files / read stdin / build one `GraphDb` / call `meow_fmt` / write-back or `--check` report / render diagnostics / pick exit code). Add `meow-fmt = { path = "../fmt" }` to `crates/cli/Cargo.toml` (fenced).
- **`crates/config/src/schema.rs`** — extend `Format` with `line_width`/`indent_width` + manual `Default` + the two `default_*` fns (block above). Re-export unchanged (`Format`/`FormatStyle` already exported from `meow-config`).
- **`crates/graph/src/cst.rs`** — switch the single stage-1 parse to **formatter-compatible options** (see fork below): `Parser::new(&owner.allocator, &owner.source, source_type).with_options(ParseOptions { preserve_parens: false, allow_return_outside_function: true, allow_v8_intrinsics: true, parse_regular_expression: false }).parse()`, with JSX enabled for JS source types (`source_type.with_jsx(true)` when `source_type.is_javascript()`), matching `oxc_formatter::parse_for_format`. This keeps the workspace at **one** parse that serves every P3 tool (parse-once) and removes `format_program`'s panic precondition. Justification + fallback in Operator notes.

Flow (`cmd_fmt`): resolve targets (cwd-relative; default = project root recursive over JS/TS
extensions) → for each, `db.set_file(path, Arc<str>)` once → `db.cst(file)` (parse-once, memoized)
→ `meow_fmt::format_cst(cst, file, opts)` → in write mode, write `code` back iff `changed`; in
`--check`, accumulate `changed`/diagnostics; render diagnostics via the shared model → exit code
per the contract. `--stdin` is the same path over a single synthetic file fed from stdin, emitting
to stdout. One `GraphDb`, one parse per file, one formatter engine — no second parser anywhere.

## Tests required

`crates/fmt/tests/format_corpus.rs` + `tests/determinism.rs` (integration) and unit tests in
`lib.rs`. Proves **I-1** [`graph-integrity`], **I-6** [`determinism`], **I-11** [`honesty`].

- **Idempotence (property; `determinism`).** Over a corpus (TS generics, `interface`/`type`,
  JSX/TSX, comments + JSDoc, template literals, long arg lists that must wrap, already-formatted
  files): `let a = format_cst(parse(x)); let b = format_cst(parse(a.code))` ⇒ `b.code == a.code`
  **byte-for-byte**, and `b.changed == false`. *Invariant: `fmt(fmt(x)) == fmt(x)`.*
- **Determinism / host-independence (`determinism`).** Format the corpus twice with a **scrambled
  `std::env`/locale** between runs ⇒ identical bytes; assert output line endings are LF on every
  case regardless of host. *Invariant: same source + options ⇒ same bytes on any machine (I-6).*
- **Parse-once / single parser (`graph-integrity`).** Structural: `meow-fmt` has no `oxc_parser`
  dependency and no `Parser::new` (backs P15); `format_cst` reads only `GraphDb::cst`. Probe
  `GraphDb::recomputes()`: format one file twice over the same `db` ⇒ the stage-1 `cst` recompute
  count does **not** advance the second time (the parse is reused). *Invariant: one parse, reused.*
- **TypeScript formatted, not stripped.** A `.ts` input with type annotations formats to output
  whose re-parse still contains those annotations (e.g. a `TSTypeAnnotation`/`interface` is present
  in `format_cst(out).program()`); fmt changes layout, not type content.
- **Comment/trivia retention.** Inputs with leading/trailing/inline comments + a shebang ⇒ all
  comments present in the output (count + text preserved by the engine).
- **Refuses broken input (`honesty`, I-11).** A file with a syntax error ⇒ `code: None`, a
  `DiagCode("fmt/syntax-error")` diagnostic with the right `file`, and (in `cmd_fmt`) the on-disk
  bytes unchanged; an unrecoverable parse (`cst.panicked()`) ⇒ same, no panic.
- **`--check` semantics.** An unformatted file ⇒ `cmd_fmt --check` exits 1 + names it and writes
  nothing (assert mtime/bytes unchanged); an already-formatted file ⇒ exit 0.
- **`--stdin`.** Unformatted stdin ⇒ formatted stdout, exit 0; `--stdin --check` on unformatted
  stdin ⇒ exit 1, empty stdout; `--stdin-filepath a.tsx` selects TSX formatting.
- **No panic (robustness).** Empty file, comment-only file, BOM, CRLF input, huge minified line,
  deeply nested expression ⇒ a valid `FmtOutcome`, never a panic (covers the `format_program`
  precondition via the GraphDb option switch).

GATES: `determinism` (I-6 — idempotence + byte-stability are this gate's fmt fixture),
`graph-integrity` (I-1 — fmt traces to the shared graph, no second parser). Floor:
`cargo test/clippy/fmt`, `principles-check.sh` (P15 single-parser, P16 host-purity).

## Rollout

New crate + flipping one CLI stub from `EXIT_UNIMPLEMENTED` to real. No persisted state, no
migration, no network. The config additions are backward-compatible (serde defaults → existing
configs parse unchanged; `Format::default()` unchanged in value). The `crates/graph/src/cst.rs`
parse-option switch is internal (no public API change) and behavior-preserving for existing
consumers (semantic/IR/strip are span/scope based — see fork). **Reversibility:** revert the
`crates/fmt` crate, the four `// === TOOL-001 ===` fences (workspace `Cargo.toml`, `cli.rs`,
`config/schema.rs`, `graph/cst.rs`), and the cli `Cargo.toml` dep; `Command::Fmt` returns to its
honest stub. Nothing to roll back on disk (write mode only ever rewrites user files the user asked
to format; `--check`/`--stdin` write nothing).

## Operator notes

Integration points (named):
- **GRAPH-001:** consumes `GraphDb::{set_file, cst, recomputes}`, `Cst::{program, comments, source, errors, panicked, source_type}`. **Requires** the stage-1 parse-option switch (fork #1).
- **DIAG-001 (coordinated):** consumes `meow_diag::{Diagnostic, Severity, DiagCode}` and the shared `Diagnostic::from_oxc(file, &OxcDiagnostic, DiagCode)` mapper, plus the parse-once `ToolSession` driver when multiple tools share one invocation. **`depends_on` lists `DIAG-001` as the proposed id for the shared-diagnostic+driver spec — pending the P3 cohort's pin over `irc`; Main reconciles if the cohort chooses another id.**
- **CFG-001/002:** reads the resolved `MeowConfig.format`; the CLI's existing config-load path supplies it (honest fallback to `FmtOptions::meow()` when config can't be evaluated).
- **oxc:** `oxc_formatter` (Beta at the 0.134 generation, Prettier-compatible, Wadler doc-IR over the Oxc AST). AST-in `format_program` reuses the shared parse — **confirmed not to construct a second parser on our path**.

Forks (FORK[] one-liners — surfaced for Main; NOT written to `decisions.json`, Main consolidates):
- **FORK[TOOL-001:parse-options]** *Switch GraphDb's single stage-1 parse to formatter-compatible options (`preserve_parens: false`, JSX-on-for-JS, `allow_return_outside_function`, `allow_v8_intrinsics`)* because `oxc_formatter::format_program` panics otherwise and parse-once (P3 exit) means ONE canonical parse must serve every tool. Safe for current consumers: strip (RT-003) is span-based, semantic (oxc_semantic) is scope/binding-based, runtime IR is the erased lowering — none depends on `ParenthesizedExpression` nodes or on the relaxed error options; `write_source` round-trip is verbatim (unaffected). This touches **merged** GRAPH-001 behavior — Main may prefer to route it as a tiny GRAPH-002 rather than a TOOL-001 fence. **Fallback** (if a future consumer needs `preserve_parens: true`): add a memoized `GraphDb::cst_for_format(file) -> &Cst` formatter-view query inside `crates/graph` (still one parser crate, I-1) — costs a second parse-per-file for files fed to BOTH the runtime and fmt, slightly denting strict "parsed once"; the global switch is preferred.
- **FORK[TOOL-001:config-surface]** *Expose only `line_width` + `indent_width`; keep indent-style (spaces), quotes (double), semicolons, trailing commas as the fixed "meow" preset* because CANON §18 collapses `.prettierrc` and the meow style is opinionated (gofmt-leaning). Reversible — promoting more `JsFormatOptions` knobs into `Format` is additive. (Zero-knob "pure preset" was the alternative; two knobs chosen for pragmatism — width + tab-size are the genuinely-needed ones.)
- **FORK[TOOL-001:idempotence]** *Guarantee + test idempotence and byte-stability as hard acceptance criteria, but do NOT promise byte-for-byte Prettier parity* because `oxfmt` is Beta ("differences from recent Prettier are treated as bugs" upstream, not a frozen contract). Honest claim (I-11): "deterministic, idempotent, Prettier-compatible style" — never "identical to Prettier vX output."
- **FORK[TOOL-001:trivia]** *Delegate comment/trivia attachment + re-layout to `oxc_formatter` (it reads `program.comments`)* rather than hand-managing trivia, because the engine already implements Prettier's comment-attachment rules; fmt passes the shared `Cst`'s program (whose `comments` are populated by GRAPH-001's lossless parse) straight through.
- **FORK[TOOL-001:crate-layout]** *Per-tool crates (`crates/fmt` = `meow-fmt`), not one shared `meow-tool`* because TOOL-003's Rolldown bundler is heavy (I-10) and a shared crate would pull it into fmt/lint builds; per-tool crates keep fmt's dep cone to `oxc_formatter` only. TOOL-002/TOOL-003 draft their own crates; Main reconciles the `crates/*` layout. (Coordinated over `irc`.)

Assumptions (local defaults — flagged, not baked):
1. `oxc_formatter`/`oxc_formatter_core` are published at versions matching the pinned `oxc_* 0.134` tag; the implementer confirms the exact crate versions and adds the pins under `# === TOOL-001 ===`. If a version skew exists, co-version to the same generation as the `oxc_ast` already in-tree (must share the AST types).
2. Default target set for a bare `meow fmt` = the project root walked recursively over JS/TS extensions (`.ts/.tsx/.mts/.cts/.js/.jsx/.mjs/.cjs`); `.cjs` is still *formatted* (fmt is style-only) even though authoring first-party `.cjs` is refused elsewhere (I-2 is a lint/load concern, not a formatting one).
3. `--stdin` without `--stdin-filepath` treats input as TypeScript (the meow default); JSON/other languages are out of MVP scope.
