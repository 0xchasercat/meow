---
spec_id: LOAD-004
title: "First-party + dependency CommonJS interop under the Drop-In Mandate"
subsystem: crates/loader, crates/graph, crates/runtime, crates/cli
status: drafting
blast_radius: high
plan_ref: "PLAN.md P2.5 · Drop-In Core (Amendments 001/002 — the adoption spine, Tier-1)"
constitution_ref: [Amendment-001, Amendment-002, I-1, I-5, I-9, I-10, I-11]
depends_on: [LOAD-003, RT-003, RT-007, PKG-003, LSP-001]
estimate: large
backend: any
---

## Motivation

Amendment 001 explicitly supersedes the old “first-party CJS is refused” rule: a stock Node/Bun project must run under `meow` unchanged. P2.5’s five-minute test fails if `.cjs`, `module.exports`, `require("fs")`, or a cached dependency’s CommonJS entry still stops at an honest boundary instead of executing.

LOAD-003 already delivers the hard half of the problem: THE single resolver classifies `.cjs` / `.js` under `"type":"commonjs"`, resolves package `exports` / `imports`, and identifies cached package members without ever consulting `node_modules`. RT-007 already delivers the Node built-in surface (`fs`, `path`, `process`, `Buffer`, `node:*`) that CommonJS must consume. LOAD-004 closes the remaining UX gap: resolved CJS must lower into runnable code in the same V8 module graph, with sync `require`, cache semantics, cycles, and honest diagnostics.

Traceability: `plan_ref` → PLAN.md P2.5 (drop-in spine, first-party CJS via require interception + Oxc wrap). `constitution_ref` → Amendment 001/002 (drop-in + UX), **I-1** (one parser / one graph), **I-5** (one resolver, no `node_modules`), **I-9** (`__dirname` / `__filename` must point at real paths the user can reason about), **I-10** (no second JS engine / no second runtime), **I-11** (typed, honest failure on unsupported dynamic require / TS-CJS forms).

## Acceptance criteria

### A1 — first-party CJS executes instead of being refused

`meow run` and the shared `MeowModuleLoader` MUST execute all of these without user rewrites:

- a local `.cjs` entry or dependency,
- a local `.js` file whose nearest `package.json` is absent or has `"type":"commonjs"`,
- a local TS-authored CommonJS file (`.cts`, or `.ts` when the graph can strip it honestly and the file uses CJS runtime syntax rather than ESM syntax).

The old `ResolveError::FirstPartyCjs` boundary is removed. Resolution still identifies the same URL; the load path now lowers CJS into runnable JavaScript.

### A2 — cached dependency CJS executes through the same loader

A dependency member that LOAD-003 classifies as `ModuleKind::Cjs` (`.cjs`, or `.js` under `"type":"commonjs"`) MUST load and run instead of surfacing `CjsDependencyUnsupported`.

This includes:

- package `exports` that choose a `require` branch,
- legacy `main` / extensionless / directory-index resolution that lands on a CJS member,
- nested dependencies and multi-version graphs resolved through the owner’s exact lockfile edge.

### A3 — one resolver, import context and require context

The resolver remains singular. LOAD-004 adds a **require-context** arm to the same `Resolver`, not a second package walker.

Required behavior:

- ESM `import` keeps LOAD-003’s condition set (`["meow", "import", "node", "default"]`).
- CJS `require()` uses the same algorithm and data structures, but with require conditions (`["meow", "require", "node", "default"]`).
- `require("fs")` and `require("node:fs")` resolve to RT-007 built-ins.
- package bare requires, relative requires, `#imports`, self-references, legacy `main`, and cached subpaths all flow through that same resolver.

### A4 — wrapper model: CJS lowers into synthetic ESM, still one V8 module graph

CJS executes by lowering each CJS module into synthetic ESM source loaded by the existing `MeowModuleLoader`.

Settled fork: no nested sync ESM evaluator and no second parser of record. The implementation MUST:

- parse/analyze via the existing `meow_graph` / Oxc surface,
- stay inside the same V8 module graph the ESM loader already uses,
- pre-walk literal `require("...")` calls and materialize their resolved targets into the wrapper,
- provide a narrow sync `require` runtime for non-literal / dynamic `require(expr)` that succeeds only when the requested specifier was already pre-walked for that module; otherwise it throws an honest, typed diagnostic naming the specifier and referrer and pointing to `legacy` / LOAD-005.

### A5 — default export and named export interop

For ESM consumers of a CJS module:

- `import cjsDefault from "./x.cjs"` MUST observe the final `module.exports` value after evaluation completes.
- `module.exports = ...` reassignment MUST update that default export binding to the final object/value.
- LOAD-004 MUST synthesize best-effort named exports for the frozen pattern set discovered by the owned AST walk:
  - `exports.foo = ...`
  - `module.exports.foo = ...`
  - `Object.defineProperty(exports, "foo", ...)` (and the `module.exports` equivalent if the same machinery makes it cheap)
- the named bindings must be “live enough” for the common cycle case: assignments to the in-flight exports object update the corresponding ESM bindings during execution, not only after the module finishes.

Anything outside that frozen pattern set is explicitly best-effort, not claimed Node-bug parity.

### A6 — CommonJS cycle semantics and require cache semantics

The wrapper MUST preserve the common Node cycle pattern:

- module A starts evaluating,
- A requires B,
- B requires A before A finished,
- B observes A’s partially initialized exports object rather than `undefined`,
- repeated `require()` of the same resolved module returns the same module object/value,
- after load, `require()` returns the final `module.exports` value.

This is the key reason the lowering is not “default export only”: the in-flight exports object has to exist before the executor body runs.

### A7 — `__dirname` / `__filename` are real, honest paths

LOAD-004 owns the `__dirname` / `__filename` seam required by P2.5 bin execution.

- local modules use their real filesystem path.
- cached package modules use `meow_pkg::UnpackedStore::ensure(integrity)` and point into the unpacked cache member path (for example `~/.meow/cache/unpacked/<algo>-<hex>/bin/run.cjs`), never a fabricated `node_modules/...` path.

This seam is consumed later by RUN-001 when package `bin` entries are executed on `meow`.

### A8 — TypeScript-authored CJS is honest

LOAD-004 MUST NOT silently miscompile TS-authored CommonJS.

Allowed:

- erasable TS syntax inside a CJS file (`const x: number = ...`, interfaces/types, etc.) lowers through the existing runtime-IR strip seam and runs.

Disallowed / must error honestly:

- non-erasable TS that emits runtime JS (`enum`, namespace with body, parameter properties, etc.),
- TS forms whose meaning is its own CJS emit (`import = require(...)`, `export =`) if the graph’s erasable strip policy rejects them.

The diagnostic MUST come from the existing typed graph/lowering path, not a panic and not a silent “best guess.”

### A9 — strict-web still parses CJS, but RT-007 withdrawal stands

`strict-web` does not ban CJS parsing or wrapping. A CJS module that requires `fs` / `process` / `Buffer` still resolves those modules through RT-007; API use is then withdrawn by RT-007’s strict-web behavior at call time.

That keeps the adoption path un-opinionated while preserving the explicit strict-web mode.

## Non-goals

- Full bug-for-bug Node parity for every CommonJS edge case (`require.extensions`, `require.cache` user mutation, `module.parent`, package self-path oddities, long-tail legacy resolution quirks).
- A second parser, second resolver, nested sync ESM evaluator, or an actual `node_modules` tree.
- `legacy` mode. Dynamic deep requires that were not pre-walked surface the honest LOAD-005 pointer instead.
- Bundler parity. TOOL-003’s CJS bundling story stays its own slice.

## Interface

### `meow_loader`

Add / change the following surface:

- `Resolver::locate(...)` remains the import-context API.
- `Resolver::locate_require(...)` / `resolve_require(...)` (or equivalent narrowly named require-context helpers) resolve through the same resolver with `require` conditions.
- `ResolvedModule` may carry the resolved locator/path metadata needed for wrapper generation and path synthesis.
- `ResolveError::FirstPartyCjs` and `ResolveError::CjsDependencyUnsupported` are removed or retired from the load path; unsupported CJS behavior now reports through specific wrapper/lowering diagnostics.

### `meow_graph`

Expose one narrow owned-analysis surface over the existing CST/AST, for example:

- `meow_graph::CjsAnalysis` — ordered literal `require` specifiers, frozen-pattern named exports, and whether the file exhibits CJS syntax / ESM syntax.
- `meow_graph::analyze_cjs(&Cst)` (or an equally narrow `GraphDb` query).

No crate outside `meow-graph` constructs a parser or allocator.

### synthetic module shapes

For each resolved CJS module URL `<u>`, the loader synthesizes:

1. the **public wrapper** at `<u>` — re-exports the registry module’s default / named bindings and imports the executor for side effects,
2. a **registry companion** — exports the in-flight CommonJS exports object and the named-binding setters; crucially this evaluates before the executor’s dependency walk so cycles see a live placeholder object,
3. an **executor companion** — imports the registry + pre-walked dependencies, defines sync `require`, `module`, `exports`, `__dirname`, `__filename`, then runs the lowered body.

Companion URLs are internal to the loader and MUST never leak into user-facing resolution APIs.

### runtime helper module

Add one internal native helper module (for example `meow:internal/cjs`) that holds the tiny JS runtime used by synthetic wrappers:

- proxy/wrapper creation for tracked `exports` objects,
- `module.exports` getter/setter wiring,
- `require()` unwrapping for synthetic CJS-vs-ESM-vs-JSON targets,
- JSON require cache,
- dynamic-require diagnostics.

This helper is internal — not part of the typed `meow:*` public surface.

## Implementation sketch

Shared-file edits MUST be fenced `// === LOAD-004 ===`.

- `crates/graph/src/lib.rs` + new `crates/graph/src/cjs.rs`
  - add the narrow CJS analysis surface over the existing CST / Oxc AST (no new parser callsite).
  - detect literal `require("...")`, frozen named-export patterns, and coarse CJS-vs-ESM syntax flags.
- `crates/loader/src/resolver.rs`
  - add require-context resolution using the same algorithm + data structures with the require condition set.
  - stop refusing first-party `.cjs` / `.js` under local commonjs.
  - classify local file kinds honestly enough for the load path to decide whether to wrap.
- `crates/loader/src/lib.rs`
  - replace the LOAD-003 CJS refusal with wrapper planning + synthetic module generation.
  - analyze/lower CJS bodies through `GraphDb` once, store the plan, and serve the public wrapper / registry companion / executor companion sources.
  - synthesize `__dirname` / `__filename`, using `UnpackedStore::ensure` for cached modules.
- `crates/runtime/src/native.rs` + new `crates/runtime/src/js/meow/cjs.ts`
  - register the internal helper source.
- `crates/cli/src/cli.rs`
  - no new architecture; `meow run` automatically benefits through the shared loader. If helper construction needs a path/cache seam, wire it here without reading any new ambient input beyond what the binary edge already owns.
- `crates/lsp/*`
  - no second resolver change. Only parity comments/tests update: locate parity stays identical; runtime no longer rejects CJS after shared resolution.

## Tests required

### loader / runtime behavior

Add targeted tests proving:

1. local first-party `.cjs` runs end-to-end,
2. local `.js` under absent / `type: commonjs` runs end-to-end,
3. cached dependency CJS runs end-to-end with no `node_modules`,
4. default import observes final `module.exports`,
5. named export synthesis works for the frozen patterns,
6. `module.exports =` reassignment updates the default export,
7. CommonJS cycle pattern sees a live partial exports object,
8. repeated `require()` returns the same object/value,
9. `require("fs")` and `require("node:fs")` both hit RT-007,
10. `__dirname` / `__filename` are real local paths and unpacked-cache paths,
11. dynamic non-prewalked `require(expr)` errors honestly and names specifier + referrer + legacy/LOAD-005,
12. TS-authored CJS with erasable syntax runs, and unsupported TS-CJS forms error honestly,
13. strict-web still parses CJS but withdraws host-backed APIs at use time.

### resolver / LSP parity

- import-context locate parity remains identical to the LSP.
- require-context chooses `require` branches where appropriate.
- loading a resolved CJS URL now succeeds instead of surfacing the old boundary.

### gates

Run targeted cargo tests for touched crates (`graph`, `loader`, `runtime`, `cli`, `lsp`) and clippy for the touched crates. Relevant gates: `drop-in`, `resolver-parity`, `determinism`, `footprint`, `compat`, `honesty`.

## Rollout

No migration is required. The change is a clean cutover from “resolve but refuse” to “resolve and execute.”

Rollback path: restore the old CJS refusal in the loader and remove the synthetic-wrapper/helper path. Because nothing is persisted on disk beyond the existing unpacked-cache store, rollback is code-only.

## Operator notes

- The named-export pass intentionally follows a frozen CJS pattern set over the owned AST. If the compat corpus later proves gaps, the fallback decision allows swapping in `cjs-module-lexer` — but only after evidence.
- The synthetic wrapper must normalize a leading shebang in CJS bodies before embedding them inside the executor function body; otherwise package `bin` scripts fail even though the source parses as a top-level script.
- Companion URL encoding is internal. Pick the boring format that is easy to strip back to the original module URL and does not alter user-visible resolution behavior.
