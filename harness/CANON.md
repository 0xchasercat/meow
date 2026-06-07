# meow — Canonical Planning Document

> A standards-first JavaScript/TypeScript runtime and unified toolchain, written in Rust on top of V8.
> One binary: runtime, package manager, workspace manager, task runner, test runner, bundler, linter, formatter, typechecker, security and observability toolchain.

---

## 0. Status of this document

- **Type:** Canonical planning document / product and architecture specification.
- **State:** Pre-implementation baseline. No code exists yet; this defines what gets built, in what order, and what we will *not* claim.
- **Last updated:** 2026-06-07.
- **Authority:** Single source of truth for scope and architecture. When this document and an implementation disagree, one of them is a bug — resolve it explicitly; do not fork the vision.
- **How to read it:** §1–§4 are the *what* (summary, thesis, principles, non-goals). §5–§20 are the *how* (decisions and subsystems). §21–§23 are the *order of operations* (roadmap, MVP, first 90 days). §24–§25 are the *honest constraints* (risks, invariants) — **read §24 before quoting any performance or security claim externally.** §26 is *build, distribution & versioning*; §27 is the *resolved-decision ledger* (every `Q-ID` and how it was settled); §28–§30 are metrics, glossary, and the one-line pitch.

A note on tone: this spec deliberately separates **commitments** (things we will build and can stand behind) from **aspirations** (things we want that have unsolved problems). Marketing blurs that line; a canonical plan must not. Where a headline claim needs qualification, the qualification lives inline and is consolidated in §24.

---

## 1. Executive Summary

`meow` is an uncompromising JavaScript and TypeScript runtime, package manager, task runner, test runner, bundler, linter, formatter, and observability toolchain delivered as **one coherent native system** — a single Rust binary.

It is not another Node wrapper, bundler, or package manager. The goal is to collapse the fragmented JS toolchain into one runtime where parsing, type analysis, module resolution, dependency management, testing, bundling, security policy, and execution all share **one canonical understanding of the project**. You install one thing; every tool reads the same parse of your code and the same model of your project.

The core bet:

> **Same source + same lockfile + same `meow` version + same capabilities = same behavior.**

`meow` is written in Rust, embeds V8 deeply, executes TypeScript via native type stripping, treats WebAssembly as a first-class module format, defaults to WinterTC-aligned Web APIs, and treats Node.js compatibility as an *explicit mode* rather than the default shape of the platform.

### 1.1 The performance claim, stated honestly

The aspiration is "10× faster than Node." That number is **only defensible for the things `meow` controls natively**, and the spec commits to it only there:

- **Cold start, install, lint, format, typecheck, bundle, test orchestration** — native Rust, no per-tool re-parsing. 10× (and more) is realistic; this is the category of win Bun, esbuild, Oxc, and Rolldown already demonstrate.
- **Steady-state JavaScript compute** — `meow` runs on **V8, the same engine Node runs on** (ADR-1). Hot-loop JS throughput is at *parity* with Node, not 10× faster. Claiming otherwise is dishonest and will not survive a benchmark.

External claim (ADR-9): **"zero-latency toolchain, instantaneous cold starts, substantially faster I/O and FFI, identical V8 peak compute."** The "10×" only ever describes startup and toolchain. Restated in §24.1.

---

## 2. Product Thesis

JavaScript tooling is powerful but fragmented. A modern project routinely carries separate tools for runtime execution, package installation, typechecking, linting, formatting, testing, bundling, task orchestration, workspace management, dependency auditing, and profiling. Each tool reparses the same files, keeps its own config model, and makes slightly different assumptions about module resolution and runtime behavior. The slowness and incoherence are emergent: not one bad tool, but a dozen tools disagreeing at the seams.

`meow` eliminates this by making the **runtime itself the canonical project authority**. It owns:

- The JavaScript and TypeScript execution environment.
- The dependency graph and the lockfile.
- The parse, semantic, type, module, and runtime graphs (§7).
- The typed task graph.
- The security and capability model.
- The test isolation model.
- The first-party developer tooling surface (lint/format/check/bundle/LSP).

Everything else in this document is a consequence of that single ownership claim.

---

## 3. North Star Principles

The invariants every feature must respect.

1. **One runtime, one graph, one truth.** All tooling reads from one shared in-memory model (§7); duplicated parsing/resolution is a defect.
2. **Web standards first; Node compatibility is explicitly requested, never ambient.** Top-level APIs are WinterTC/Web-standard; Node APIs live behind a mode (§11).
3. **First-party code is modern ESM only.** CJS exists solely as a read-only dependency-compatibility surface (ADR-3).
4. **TypeScript runs with no transpilation tax.** Types are erased like comments; no emitted JS, no sourcemap translation layer (§9).
5. **Deterministic & hermetic.** Same source + lockfile + version + capabilities → same behavior; the host environment cannot influence execution unless explicitly granted.
6. **Dependencies are content-addressed and capability-scoped.** Global cache, hash-pinned, integrity-checked, least-authority by default.
7. **Tooling reuses one parsed representation.** Linter, formatter, typechecker, and bundler tap shared graph state instead of re-deriving it.
8. **Runtime APIs ship with native types.** No hand-maintained `@types/*` drift; `meow:*` declarations are generated from the runtime implementation itself.
9. **Observability is built in, not an aftermarket plugin ecosystem.** Profiling and dependency analysis are CLI features (§17).
10. **Monorepos are a first-class project shape** (§13), not an external-tool problem.
11. **Configuration collapses, not multiplies.** One typed config file (§18).
12. **Boring escape hatches always exist.** `--materialize`, `node-compat`, `legacy` ensure the ambitious defaults never become a wall.

(Principles describe philosophy. The hard rules that require an explicit re-charter to change are listed separately as **Project Invariants**, §25.)

---

## 4. Non-Goals

Stated up front to prevent scope creep and set expectations. `meow` is **not**:

- **A new language.** It runs JS and TS; it does not invent syntax.
- **A new engine.** It embeds V8 (ADR-1); it does not write a JS engine.
- **A from-scratch parser/bundler.** It builds on Oxc and Rolldown (ADR-2); reinventing them is out of scope.
- **A CommonJS authoring environment.** First-party CJS is refused by design (ADR-3).
- **A drop-in Node clone.** `node-compat` targets the common, documented surface; `legacy` widens it; neither promises bug-for-bug parity with undocumented Node behavior.
- **A generic, engine-neutral abstraction layer.** Engine neutrality is explicitly rejected (ADR-1).
- **A runtime that reads host state for convenience.** No ambient authority (§15).
- **A runtime built on `node_modules` as its primary resolution model** (ADR-4).
- **A browser.** No DOM. Web-standard *platform* APIs (`fetch`, streams, WebCrypto), not web *document* APIs.
- **A collection of unrelated tools under one CLI brand.** The point is the shared graph, not the shared prefix.

---

## 5. Definitive Architectural Decisions (ADRs)

ADR-1 through ADR-4 are the founding choices. ADR-5 through ADR-9 forcibly resolve the items that earlier drafts left "aspirational" (§24) — each is now a committed direction, not a wish. All are recorded with rationale and — critically — **consequences**. An ADR without its consequences is marketing.

### ADR-1 · Engine: **V8**

- **Choice:** V8 only, embedded via `rusty_v8`/`deno_core`. No swappable-engine abstraction.
- **Why:** Isolate architecture is best-in-class and we depend on it for per-isolate security (§15) and isolate-per-test (§16). Deno and Cloudflare Workers prove single-digit-ms isolate spin-up at scale. V8 also has the most production-ready Wasm integration. A single-engine commitment lets `meow` exploit V8 memory behavior, zero-copy strings, and GC hooks far deeper than a generic layer could.
- **Rejected:** JavaScriptCore (Bun's choice) — excellent cold start, but a weaker isolate/embedding story for our security and test models.
- **Consequences:**
  - (+) Mature Rust bindings; we inherit `deno_core`'s op/extension model.
  - (+) Wasm and isolates are first-class.
  - (+) Engine is *fixed*, so the runtime can optimize against one target — but this also means engine config is **not** a user-facing knob (see §18).
  - (−) **JS execution speed is bounded by V8 — identical to Node.** Our speed story must come from startup, I/O, and tooling, *not* steady-state compute (§1.1, §24.1).
  - (−) V8 is a large, fast-moving C++ dependency; version bumps are a recurring, high-priority maintenance event (§24.8).

### ADR-2 · Implementation language: **Rust**

- **Choice:** Rust.
- **Why:** The next-gen JS tooling ecosystem already standardized on Rust. **Oxc** (parser/semantic/transformer) powers our parse pipeline; **Rolldown** powers bundling; `deno_core` provides V8 glue. Rust also suits content-addressed storage, graph processing, native CLI tooling, and sandbox-policy enforcement. We absorb thousands of engineer-hours instead of writing a parser from scratch.
- **Rejected:** Zig (Bun's choice) — extreme control, but a far smaller reusable tooling ecosystem; we'd rebuild what Oxc/Rolldown already give us.
- **Consequences:**
  - (+) Memory safety across a large native surface.
  - (+) Direct reuse of Oxc and Rolldown as libraries, not subprocesses. Core crates should be designed as reusable internal components from day one.
  - (−) Careful FFI boundary management is required at the V8 and native-module edges.
  - (−) Cross-platform packaging and build reproducibility need early investment (§24.10).
  - (−) We are coupled to the maturity of Oxc (parse/semantic/transform) and Rolldown (bundling). The hardest gap — a native *type graph* — is sidestepped entirely by delegating typechecking to `tsc`/`tsgo` (ADR-5), so the residual coupling is to stripping/bundling, not type inference.

### ADR-3 · CJS/ESM boundary: **First-party code MUST be ESM; CJS is read-only for dependencies**

- **Choice:** `meow` refuses to execute a locally authored `.cjs` file or a first-party `require()`. Dependencies written in CJS are statically analyzed and wrapped in a synthetic ESM shell at the module loader.
- **Why:** If new CJS can be authored, the ecosystem never heals and the exact ambiguity `meow` exists to remove is preserved. You may *import* legacy CJS seamlessly; you may not *write* it.
- **Consequences:**
  - (+) First-party graph is pure ESM — simpler resolution, tree-shaking, top-level await.
  - (+) CJS semantics never leak into user code.
  - (−) The CJS→ESM wrapper must handle the ugly cases: dynamic `require()`, conditional exports, circular deps, `module.exports` reassignment, `__dirname`/`__filename`. The named-exports problem (statically discovering CJS named exports) is genuinely hard; the wrapper guarantees the default export and best-effort named exports (§11.4).
  - (−) Packages that monkey-patch the module system at require-time need `legacy` mode (§11).

### ADR-4 · Default package loading: **PnP-style virtual memory (no `node_modules` by default)**

- **Choice:** Default mode writes **no** `node_modules`. The module resolver intercepts lookups in-memory and points at the global content-addressed cache. FUSE/VFS is opt-in; `--materialize` writes real files when a legacy tool demands them.
- **Why:** FUSE/VFS as a default is a cross-platform minefield (Windows, macOS kext deprecation, Docker volumes). In-memory interception avoids it entirely, gives zero-install startup, and lets `meow` enforce hermeticity, provenance, and capability policy at resolution time.
- **Consequences:**
  - (+) Zero-install, instant, disk shared across projects; no `node_modules` to corrupt or `rm -rf`.
  - (+) The lockfile and package cache become core runtime infrastructure, not a side artifact.
  - (−) Tools that `stat()` the real filesystem for packages break under PnP. **`--materialize` is a required escape hatch, not optional polish** — it ships in the *same phase* as PnP (§24.4).
  - (−) The runtime and the language server **must share identical resolution logic**, or the editor and the runtime will disagree.
  - (−) The resolver must implement the full Node resolution algorithm (conditions, `exports`/`imports` maps, self-references) faithfully or subtle bugs appear.

### ADR-5 · TypeScript diagnostics: **delegate to the reference compiler; never reimplement it**

- **Choice:** `meow` will **not** build a native Rust TypeScript typechecker. TypeScript is split into two concerns: **Execution** and **Diagnostics**.
  - *Execution* is pure Oxc **type stripping** — instant, native, on the hot path (§9).
  - *Diagnostics* (`meow check`, editor squiggles) are **delegated** to the official compiler — `tsc` today, `typescript-go` (`tsgo`) as it stabilizes — orchestrated as a long-lived, reused, isolated **daemon**.
  - A **fast, intentionally-incomplete type-aware linter** runs in-process on the Oxc semantic graph (§7) to catch obvious mismatches cheaply; absolute soundness is left to the reference compiler.
- **Why:** TS's type system is Turing-complete, partly undocumented, and defined by a decade of bug-for-bug behavior. Native reimplementations (`stc`, `Ezno`) have proven to be multi-year efforts, and Microsoft itself chose **Go** for `typescript-go`, which does not compose with a Rust graph. Delegation buys correctness now and saves an estimated multi-year compiler effort.
- **Consequences:**
  - (+) Execution stays instant and native; checking is *exactly* as correct as the reference compiler.
  - (+) Frees the core team from the single largest item in the spec (§24.2) — roughly two years of work not taken on.
  - (+) The daemon consumes the generated **shadow `tsconfig.json`** (ADR-8), so integration is clean and zero-config for the user.
  - (−) **`meow check` is only as fast as `tsc`/`tsgo` — it is explicitly NOT part of the "10× toolchain" claim.** Editor diagnostic latency equals the daemon's latency.
  - (−) The fast path depends on Microsoft's `tsgo` roadmap/maturity; the fallback is `tsc` (correct, slower).
  - (−) Two type engines (the fast in-process linter and the reference checker) can disagree. The linter MUST be labeled a *fast preview* and is never authoritative.

### ADR-6 · Per-package security: **tiered, default to AST-rewrite "Trust Zones," not isolate-per-package**

- **Choice:** Three tiers, shipped in order:
  1. **Default — process-level grants** (Deno-style): capabilities apply to the whole program. Honest and fast.
  2. **Trust Zones (the committed innovation):** because `meow` owns the parser *and* the module loader, per-package capability is enforced by **load-time AST rewriting + lexical interposition** — a dependency that lacks a capability never receives the real binding in its module scope, and residual access points (`fetch`, `require('net')`, `import "node:net"`, …) are rewritten to capability traps. No per-dependency V8 isolate, no membrane on the hot path.
  3. **Escalation (research):** intrinsic-freezing membranes (SES/LavaMoat) and/or isolate-per-trust-zone for workloads that need soundness against *adversarial* code.
- **Why:** spinning up hundreds of isolates for micro-dependencies destroys startup; full membranes break monkey-patching and add friction. Owning the pipeline lets us interpose at load time at near-zero steady-state cost for the common case.
- **Consequences:**
  - (+) Near-zero runtime overhead for tiers 1–2; no isolate explosion; excellent default DX.
  - (+) Strong against *accidental* capability use and *naive/common* malicious patterns.
  - (−) **Static rewriting is NOT a sound sandbox against adversarial dynamic access.** Computed/reflective reaches — `globalThis["fe"+"tch"]`, `Function("return fetch")()`, `constructor.constructor`, dynamic `import()`, `eval`, and access via other globals — can defeat pure rewriting. Soundness against a determined attacker requires tier 3 (freeze intrinsics / per-context curated globals / separate isolate). Messaging is **"defense-in-depth,"** never *"mathematically blocked,"* until tier 3 ships. (Residual tracked in §24.3.)
  - (−) Scoped grants (e.g., network to `host:port`) still need a tiny runtime check inside the trap; "zero overhead" is exact only for rewrite-to-deny.
  - (−) Interposition must cover every access shape (call, namespace import, dynamic import, property access) and stay correct across the CJS→ESM wrapper (ADR-3).
  - *Implementation avenue:* per-V8-**context** curated globals (contexts are far cheaper than isolates and share the heap) are the natural home for Trust Zones when scope-stripping alone is insufficient.

### ADR-7 · WebAssembly: **no source imports; graph-aware task-driven compilation**

- **Choice:** `meow` will never execute `.rs`/`.c`/source files directly and will **not** embed `rustc`/`cargo`/clang. Native code is compiled to `.wasm` by a user-defined task (e.g., `build:wasm`) that invokes the host toolchain. Because the module graph knows `./crypto.wasm` is that task's **output**, importing a missing/stale artifact automatically triggers its producing task *before* the dependent JS runs.
- **Why:** embedding a Rust/C toolchain bloats the binary into the gigabytes, wrecks install time, and breaks hermeticity. Task-driven compilation keeps the boundary clean and reuses the graph-aware task runner (§14).
- **Consequences:**
  - (+) Binary stays small (no toolchain bloat; overall distribution target < 60 MB, §26.1); install stays fast; hermeticity is preserved — `meow` pins the `.wasm` artifact, not the toolchain.
  - (+) Feels automatic via graph wiring; no magic source interpreter, a mathematically clean boundary.
  - (−) The *compile* step requires the relevant host toolchain to be installed (by design); CI must provision it. Building from source is therefore **not** hermetic w.r.t. toolchain availability — only the resulting `.wasm` is pinned (§24.6).
  - (−) Requires a concrete **artifact→task** mapping: task `outputs` register artifacts in the module graph, and importing such an artifact creates a dependency on its producing task.

### ADR-8 · Config: **single human-edited source + generated shadow configs**

- **Choice:** humans edit **only** `meow.config.ts`. `meow` derives and regenerates the legacy configs the ecosystem expects:
  - **`tsconfig.json`** — a committed one-line root shim `{ "extends": "./.meow/tsconfig.json" }`; the real content is regenerated into `.meow/`.
  - **`package.json`** — has **no `extends` mechanism**, so the shadow trick does not apply: `meow` generates the **real root `package.json`** as a derived, regenerated artifact (publishing metadata is projected out of `meow.config.ts`).
  - Regeneration triggers: `meow install`, config change (daemon/LSP watch), and `meow sync` / `meow doctor`.
- **Why:** VS Code, WebStorm, CI, and thousands of tools hardcode `package.json`/`tsconfig.json`. Deleting them outright breaks every non-`meow` tool; generating them keeps `meow` the single source of truth while the ecosystem "just works."
- **Consequences:**
  - (+) One human-edited file; full IDE/CI compatibility with zero manual duplication.
  - (+) The generated `.meow/tsconfig.json` is exactly what the delegated typechecker (ADR-5) consumes.
  - (−) Because `package.json` cannot be redirected, `meow` **owns/overwrites** the root `package.json`; hand-edits to it are clobbered on regeneration (`meow doctor` warns).
  - (−) Generated files are build artifacts: `.meow/` is gitignored; CI must run `meow sync` (or `meow install`) so the shadows exist. Staleness is a real failure mode — `meow doctor` checks it. (Residual in §24.5.)

### ADR-9 · Performance positioning: **win on I/O, FFI, and cold start — not steady-state compute**

- **Choice:** stop claiming faster steady-state JS. Embrace V8-parity compute and compete where Node is actually weak: cold start, the native toolchain, and the runtime's **syscall/FFI layer** — Rust async I/O (`tokio`; `io_uring` on Linux where available, `kqueue`/IOCP elsewhere) and low-overhead FFI versus Node's `libuv` + N-API. Public claim becomes: *"Zero-latency toolchain, instantaneous cold starts, substantially faster I/O and FFI, identical V8 peak compute."*
- **Why:** `meow` and Node share V8; hot-loop math is identical, and lying about it ruins credibility. The defensible wins are startup, tooling, concurrent I/O, and FFI.
- **Consequences:**
  - (+) Every claim is benchmark-defensible and cannot be refuted by a trivial compute microbenchmark.
  - (−) "Faster I/O" is **workload-specific**, not a constant. A specific multiplier (e.g., "3× I/O") may hold for concurrent network, many-file reads, WebSockets, and FFI-heavy paths, but **not** for trivial sequential reads where `libuv` is already optimal. Any headline number MUST name the workload and ship with a reproducible benchmark (§24.1).
  - (−) `io_uring` is Linux-only; the cross-platform commitment is the `tokio`/`mio` abstraction, with `io_uring` as a per-platform accelerant — not the portability story.

---

## 6. System Overview

### 6.1 The canonical pipeline

`meow` is organized around one lowering pipeline. Tools do not independently parse and reinterpret the project; they subscribe to shared graph state.

```text
Source Text
  -> Lossless Syntax Tree
  -> Semantic Graph
  -> Type Graph            (delegated → tsc/tsgo daemon, ADR-5 — not a native stage)
  -> Module Graph
  -> Runtime IR
  -> Execution, Bundling, Testing, Linting, Formatting, Diagnostics
```

Detailed in §7.

### 6.2 Primary subsystems

- **Runtime** — V8-backed execution, isolates, permissions, Web APIs, native `meow:*` APIs.
- **Parser & graph engine** — lossless syntax tree, semantic graph, module graph, runtime IR; type information comes from the delegated checker (ADR-5), not a native type graph.
- **Module loader** — ESM-first resolution, dependency CJS wrapping, virtual package access, lockfile enforcement.
- **Package manager** — global content-addressed cache, deterministic lockfile, install modes.
- **Workspace manager** — monorepo graph discovery, package linking, task orchestration.
- **Task runner** — typed `meow.tasks.ts`, declared inputs/outputs, graph-aware caching.
- **Test runner** — isolate-backed tests with deterministic clocks, fake I/O, isolated state.
- **Tooling layer** — formatter, linter, typechecker, bundler, diagnostics, editor integration.
- **Security layer** — package capability policy, provenance tracking, anomaly detection.
- **Observability layer** — built-in tracing, profiling, dependency analysis, bundle analysis.
- **Configuration layer** — single typed `defineMeow({...})` file.

### 6.3 Feature map (pillars → sections)

Stable feature IDs (`Fn`) for cross-referencing in issues, ADRs, and the roadmap.

| Pillar | Features | Spec |
|--------|----------|------|
| **1 · Core Architecture & Engine** | F1 Monogamous V8 integration · F2 Canonical parse pipeline · F3 Deterministic & hermetic execution | ADR-1, §7, §12 |
| **2 · Language & Execution** | F4 Type erasure & Wasm-native · F5 Typed native runtime APIs · F6 Explicit compatibility modes | §9, §10, §8, §11 |
| **3 · Dependency & Project** | F7 Pluggable virtual `node_modules` · F8 First-class workspace graph · F9 Typed runtime tasks | §12, §13, §14 |
| **4 · Security & Observability** | F10 Supply-chain sandboxing & provenance · F11 Native observability · F12 Isolate-backed tests | §15, §17, §16 |
| **5 · Developer Experience** | F13 The config singularity | §18 |

---

## 7. The Canonical Parse Pipeline (F2)

The technical heart of `meow`. Everything else is downstream of getting this right.

### 7.1 Stages

Source text flows through five lowering stages, each a stable, queryable artifact:

| Stage | Artifact | Primary consumers | Built on |
|------|----------|-------------------|----------|
| 1 | **Lossless Syntax Tree** (full-fidelity CST; trivia, comments, whitespace preserved) | Formatter, codemods | Oxc parser |
| 2 | **Semantic Graph** (scopes, bindings, symbols, references) | Linter, resolver | `oxc_semantic` |
| 3 | **Type info** (declarations, inference, assignability) | `meow check`, type-aware lint | **delegated** `tsc`/`tsgo` daemon (ADR-5); in-process lint reads stage 2 |
| 4 | **Module Graph** (import/export edges, resolved specifiers) | Bundler, loader, workspace graph | Rolldown + native resolver |
| 5 | **Runtime IR** (lowered, type-erased, ready for V8) | Runtime, bundler output | Oxc transformer |

Lossless at stage 1 is non-negotiable: the formatter needs trivia and codemods must round-trip. Stage 3 is **delegated, not native** (ADR-5) — `meow` builds no type graph; the `tsc`/`tsgo` daemon does, out of process, consuming the shadow `tsconfig.json` (ADR-8). Type erasure happens late (stage 5), so the delegated checker still sees full annotations.

### 7.2 Incrementality is a requirement, not an optimization

A shared model is only valuable if it stays warm. An editor keystroke must not re-run the whole pipeline. The pipeline MUST be built as a **query system with memoized, invalidatable nodes** (salsa / `rust-analyzer`-style). Editing one file invalidates only the dependent slices of the semantic and module graphs. This is load-bearing for the LSP (§20) and for `meow.tasks.ts` input-hashing (§14) — design it in from day one; retrofitting incrementality is a rewrite.

### 7.3 One model, two clients

The same pipeline serves the **CLI** (batch: lint/format/check/bundle/test) and the **LSP** (interactive: hover, completion, diagnostics). They differ only in scheduling and lifetime, not in the model. This is what makes "the linter and the typechecker never disagree" true *by construction* rather than by convention — and it is why a file is parsed once and reused across many operations in a single invocation.

---

## 8. Runtime & Standards Model

### 8.1 Standards-first API surface

The default global runtime is Web-standard aligned; `meow` tracks WinterTC and relevant Web Platform specs as the top-level compatibility target. Node APIs are **not** ambient by default (§11). The eventual target surface is the WinterTC common-minimum:

`fetch` · `Request` · `Response` · `Headers` · `URL` · `URLPattern` · `AbortController` · `ReadableStream` · `WritableStream` · `TransformStream` · `crypto` / `SubtleCrypto` · `TextEncoder` / `TextDecoder` · `EventTarget` · `structuredClone` · `queueMicrotask` · timers · `Blob` · `FormData` · `WebSocket`.

**Phase-1 committed subset — "Stateless Edge" (Q7, resolved).** Exactly what is needed to stand up a web server or worker, nothing more: `fetch`, `Request`/`Response`, `Headers`, `URL`/`URLPattern`, `console`, `crypto.subtle`, `TextEncoder`/`TextDecoder`, `AbortController`, `Blob`, `FormData`, `setTimeout` (streaming body types arrive alongside `fetch`). **No** DOM, `window`, or `localStorage`.

### 8.2 Native `meow:` APIs (F5)

Runtime-specific capabilities are imported behind a typed namespace, never bolted onto globals:

```ts
import { serve } from "meow:http";
import { readTextFile } from "meow:fs";
import { spawn } from "meow:process";
import { workspace } from "meow:project";
import { test, expect } from "meow:test";
import { defineMeow } from "meow:config";
import { defineTasks } from "meow:tasks";
```

Rules:

- Every `meow:*` API ships with native TypeScript declarations.
- Declarations are generated from the **same source of truth** as the runtime implementation — they cannot drift.
- IDE types must never depend on separately installed community type packages (no `@types/meow`).
- Capability requirements (§15) are attached to APIs where relevant, so the editor can surface what an API will request.

---

## 9. TypeScript Execution Model (F4)

`meow` executes TypeScript through native **type stripping**: annotations are erased to whitespace, producing valid JS with identical source positions — no transpilation output and no sourcemap translation layer for normal TS. Type-only imports are removed before execution.

This works for **erasable syntax only — resolved as strict TC39 Erasable Syntax (Q6).** Constructs that emit runtime JS — `enum`, non-ambient `namespace`, parameter properties (`constructor(public x: number)`), `import =` / `export =` — are **banned**: they cannot be deleted as whitespace, so `meow` errors with a fix-pointing message, e.g. *"Enums emit runtime code and cannot be type-stripped. Use a `const` object instead."* This keeps the "types are comments" guarantee exact and pushes first-party code to the modern standard; there is no transpile-to-downlevel crutch.

### 9.1 Type system responsibilities

Type *checking* is a separate, far larger effort than *stripping* — large enough that `meow` **does not build a native typechecker at all** and delegates it to the reference compiler (ADR-5). The type system exists to support:

- Editor diagnostics.
- `meow check`.
- Native typing for `meow:*` modules.
- Task typing and test typing.
- Type-aware linting.
- Type-aware dead-code and bundle analysis.

**How these are served (ADR-5).** `meow check` and authoritative editor diagnostics are produced by the official compiler (`tsc` today, `typescript-go`/`tsgo` as it stabilizes) running as a reused, isolated daemon that consumes the generated shadow `tsconfig.json` (ADR-8). In parallel, a fast, intentionally-incomplete **type-aware linter** runs in-process on the Oxc semantic graph to flag obvious mismatches with low latency. The linter is a *fast preview*; the daemon is the source of truth. Consequence: `meow check` is as fast as the reference compiler — it is **not** part of the "10× toolchain" claim (§24.1).

**Runtime execution must not wait on full typechecking** unless the user explicitly asks for that mode. Stripping is on the hot path; checking is not.

---

## 10. WebAssembly Native Model (F4)

`meow` treats WebAssembly as a first-class module format in the same module graph as JS/TS:

```ts
import { parseImage } from "./image.wasm"; // first-class Wasm import
import { encrypt }    from "./crypto.wasm"; // compiled by a build:wasm task (ADR-7)
```

Design goals: Wasm artifacts are pinned in the lockfile; Wasm modules carry explicit capability policy; host bindings are typed; native FFI avoids unnecessary serialization. The long-term target is the **WebAssembly Component Model** (typed imports/exports via WIT, capability boundaries, efficient host bindings).

**Resolved — no source imports (ADR-7).** `meow` will never execute `.rs`/`.c` source directly and will not embed `rustc`/`cargo`/clang — that would bloat the binary into the gigabytes and break hermeticity. Instead you import the compiled `./crypto.wasm`, and a `build:wasm` task (§14) invokes the host toolchain to produce it. Because the module graph records that artifact as the task's `output`, importing a missing or stale `.wasm` **automatically runs its producing task first** — it feels automatic, yet the boundary stays clean and only the `.wasm` is pinned (the compile step requires the host toolchain; §24.6). WebAssembly Component Model support (typed WIT boundaries, capability boundaries) is staged after direct `.wasm` (Q11).

---

## 11. Compatibility Modes (F6)

Three explicit modes; mode is set per-project in `defineMeow` and may be narrowed per-dependency.

| Mode | Purpose | First-party CJS | Node built-ins | Legacy monkey-patching |
|------|---------|-----------------|----------------|------------------------|
| `strict-web` | Pure standards-first runtime | No | No | No |
| `node-compat` | Modern npm compatibility | No | Yes (`fs`, `path`, `crypto`, `process`, `Buffer`, …) | Limited |
| `legacy` | Old-package survival mode | No | Yes | Tolerated for **dependencies** |

- **`strict-web`** — only Web-standard globals and `meow:*`. For edge services, portable libraries, security-sensitive apps, and new projects. The cleanest mode.
- **`node-compat`** — Node-style APIs + npm compatibility while first-party code stays ESM. For server apps migrating off Node and frameworks expecting `fs`/`path`/`crypto`/`process`.
- **`legacy`** — deeper dependency compatibility for old CJS quirks. For large legacy apps, migration audits, temporary bridges. **Not** the long-term default for healthy projects; diagnostic-heavy, and it names exactly which dependency forced it.

First-party CJS is rejected in **all** modes (ADR-3). The mode only widens what *dependencies* may do.

### 11.1 CJS interop (ADR-3)

CJS dependencies are wrapped in a synthetic ESM module (Q5, resolved). `meow` parses each CJS dependency with **`cjs-module-lexer`** to extract top-level `exports.foo = …` / `module.exports.foo = …` assignments, then emits a synthetic ESM shell exposing those **named exports** plus a **default export** carrying the entire `module.exports` object. `__dirname` and `__filename` are natively mocked to the file's resolved cache path. **Dynamic deep requires** (`require(variable)`) cannot be statically resolved — `meow` emits a diagnostic naming the package and asking you to move it to `legacy` mode rather than silently guessing.

---

## 12. Dependency & Package Management (F7)

### 12.1 Global content-addressed cache

Packages are stored globally by content hash; projects reference them through the lockfile and module loader rather than copying dependency trees into each workspace.

- Shared disk storage across projects.
- Hash-pinned package contents; integrity checked before execution.
- Provenance metadata attached to packages (§15.3).
- Lockfile-driven reproducibility.

### 12.2 Install / access modes

| Mode | Default | Description | Best for |
|------|:------:|-------------|----------|
| **PnP (in-memory)** | ✅ | No `node_modules`; loader resolves directly from cache | Normal development |
| **VFS** | | Virtual folder backed by `meow` resolution (FUSE) | Local tools needing directory semantics |
| **Materialized** | | Real files written to disk | CI, Docker, legacy tools |
| **Vendor** | | Full local dependency copy | Air-gapped deployments |

```sh
meow install                 # PnP, writes lockfile, no node_modules
meow install --vfs
meow install --materialize   # required escape hatch — ships with PnP (§24.4)
meow install --vendor
```

The resolver must implement the full Node resolution algorithm faithfully (ADR-4), and the LSP must use the **same** resolver (ADR-4) — see the LSP package-exposure mechanism in §20.

### 12.3 Lockfile

The lockfile is the execution contract. It pins:

- Package versions and **content hashes**.
- Transitive dependency resolutions.
- Wasm artifacts.
- Native binaries.
- Build scripts and outputs where applicable.
- Registry provenance.
- Capability grants (§15).
- `meow` runtime-version constraints.

The lockfile is **`meow.lock.jsonl`** — a custom, strictly-sorted **JSON-lines** format (Q4, resolved): one dependency per line, alphabetically sorted, so git merges rarely conflict and machine parsing is trivial. `meow` deliberately does **not** interop with `pnpm-lock.yaml` or `package-lock.json`, because it tracks metadata they lack — fine-grained capability grants, Wasm hashes, synthetic-ESM wrapper info — and a single flat JSON document produces large, frequent merge conflicts. Honest limit (§24.7): pinning gives reproducible **selection** (content hash per platform), not reproducible **builds** of arbitrary native binaries.

---

## 13. Workspace Graph (F8)

`meow` treats monorepos as a native shape, not an external-tool problem. The workspace graph understands applications, packages, internal vs. external dependencies, task dependencies, build outputs, test targets, and affected files — inferring internal package relationships without complex `tsconfig.json` path mapping or Turborepo.

```text
apps/web  -> packages/ui, packages/auth, packages/config
apps/api  -> packages/auth, packages/db, packages/config
```

This graph feeds task scheduling (§14), affected-test selection (§16), and `why-dep` (§17).

---

## 14. Typed Task Runner (F9)

`package.json` scripts are replaced by a typed `meow.tasks.ts`:

```ts
import { defineTasks } from "meow:tasks";

export default defineTasks({
  build: {
    inputs:  ["src/**/*", "meow.config.ts"],
    outputs: ["dist/**"],
    async run(ctx) {
      await ctx.bundle({ entry: "src/index.ts", outdir: "dist" });
    },
  },
  test: {
    inputs:  ["src/**/*", "tests/**/*"],
    outputs: [".meow/test-results.json"],
    async run(ctx) {
      await ctx.test();
    },
  },
});
```

Rules: tasks are typed; declare `inputs`/`outputs`; can be cached, parallelized when the graph allows, and skipped when source + dependency + graph state are unchanged; and can request explicit capabilities (§15). The input/output hashing reuses the incremental machinery of §7.2, and tasks compose across the workspace graph (§13).

---

## 15. Security Model (F10)

> This pillar is the most valuable and the **least solved**. Read §24.3 before promising any of it.

### 15.1 Capability model

Code receives **no ambient authority**. A package gets filesystem/network/env/subprocess/etc. access only when granted, and grants are **scoped** — not just "network," but "network to `tcp:localhost:5432`"; not just "read," but "read `assets/**`."

```ts
// inside defineMeow({...})
permissions: {
  packages: {
    lodash: [],
    pg:     [{ network: "tcp:localhost:5432" }],
    sharp:  [{ read: "assets/**" }, { write: ".meow/cache/sharp/**" }],
  },
},
```

### 15.2 Capability categories

Filesystem read · filesystem write · network · environment-variable access · subprocess execution · native-binary execution · Wasm host capabilities · system time · randomness · test-only fake services.

### 15.3 Enforcement boundary — resolved (ADR-6)

Per-package capabilities are only **sound** if a package cannot reach another package's authority through the shared heap. Rather than bet everything on one boundary, `meow` ships three tiers in order:

1. **Default — process-level grants** (Deno-style): capabilities apply to the whole program. Honest, fast, sound — just not per-package.
2. **Trust Zones — the committed innovation:** because `meow` owns the parser *and* the module loader, per-package capability is enforced by **load-time AST rewriting + lexical interposition**. A dependency that lacks a capability never receives the real binding in its module scope, and residual access points (`fetch`, `require('net')`, `import "node:net"`, …) are rewritten to capability traps. No per-dependency isolate, no membrane on the hot path, near-zero steady-state overhead. (Per-V8-*context* curated globals — cheaper than isolates — are the implementation avenue when scope-stripping alone is insufficient.)
3. **Escalation (research):** intrinsic-freezing membranes (SES/LavaMoat) or isolate-/context-per-trust-zone, for workloads that need soundness against *adversarial* code.

**Honest boundary (the residual, §24.3):** tier 2 is strong *defense-in-depth* — it defeats accidental capability use and naive/common malicious patterns — but static rewriting is **not** a sound sandbox against adversarial *dynamic* access (`globalThis["fe"+"tch"]`, `Function("return fetch")()`, dynamic `import()`, `eval`). Soundness against a determined attacker is tier 3. So `meow` markets tiers 1–2 as **"defense-in-depth," never "mathematically blocked,"** until tier 3 ships. Usability matters too (§24.11): good defaults, permissions suggested from observed behavior, and confirmation required for privilege expansion.

### 15.4 Provenance & anomaly detection

- Pin package identity and publisher in the lockfile.
- **Provenance sources (Q9, resolved):** verify **npm-registry Sigstore** signatures (npm / GitHub Actions provenance) at install time, and mirror the public **OSV** (Open Source Vulnerabilities) database into the global cache to flag known-malicious/vulnerable hashes locally. No bespoke threat-intelligence layer is invented.
- Flag at install/update: ownership changes, capability escalations, new install scripts, postinstall network calls, a transitive dep introducing a native binary, a build script changing between lockfile updates.
- Warnings are specific and actionable:
  > ⚠ `express-utils` changed ownership on 2026-06-06 and now requests read access to `~/.ssh`.

This is **detection/advisory**, distinct from §15.3 enforcement. It is shippable independent of the enforcement boundary, is lower-risk, and is therefore sequenced earlier (§21).

---

## 16. Test Runner (F12)

`meow test` runs each test file in its own cheap V8 isolate.

- Isolated global state per file; leaks across tests are structurally prevented.
- Deterministic by default: **fake clock**, **fake network**, seeded randomness; tests opt into real I/O explicitly.
- Filesystem access is capability-scoped (§15).
- Massively parallel; runtime traces (§17) can be attached to failing tests.
- Built into the binary via `meow:test`; configuration lives in `defineMeow` (§18), not a separate runner config.
- Shares the parse pipeline, so coverage maps and source positions come from the same model that ran the code.

```ts
import { test, expect } from "meow:test";

test("responds with a greeting", async () => {
  const res = await fetch("http://example.test/hello");
  expect(await res.text()).toBe("hello");
});
```

---

## 17. Observability & Diagnostics (F11)

Native, no Chrome DevTools or third-party analyzers required. All commands read from the same module graph (§7) and runtime op layer, so they are consistent with what actually executed.

```sh
meow why-slow        meow why-large        meow why-dep <pkg>
meow trace           meow profile          meow doctor
```

- **`why-slow`** — slow imports, expensive initialization, long-running tasks, cold-start cost, parse/typecheck hotspots, per-dependency execution cost. **Phase-6 first subset (Q10, resolved): the Module Load/Parse Timeline** — a terminal waterfall of the milliseconds spent parsing, linking, and evaluating each dependency's top-level scope (initialization tax is ~95% of "why is my app slow"), before any deep V8 JIT/CPU profiling.
- **`why-large`** — largest modules, duplicate packages, unused exports, CJS tree-shaking barriers, Wasm/native asset size.
- **`why-dep <pkg>`** — which workspace introduced it, which transitive path requires it, its version/hash, its capabilities, its runtime and bundle cost.
- **`trace`** — module-load timeline, isolate creation, permission checks, network calls, filesystem access, GC activity, Wasm-boundary costs.
- **`profile`** — sampling/allocation profile of a run.
- **`doctor`** — environment + config + lockfile health check.

---

## 18. The Config Singularity (F13)

One typed configuration file replaces the graveyard of per-tool configs.

```ts
import { defineMeow } from "meow:config";

export default defineMeow({
  mode: "strict-web",                         // | "node-compat" | "legacy"

  workspace: { packages: ["apps/*", "packages/*"] },

  runtime:   { typescript: "strip" },         // engine is fixed to V8 (ADR-1) — not configurable

  install:   { mode: "pnp" },                 // | "vfs" | "materialized" | "vendor"

  lint:      { rules: { "no-first-party-cjs": "error", "no-ambient-node-api": "error" } },
  format:    { style: "meow" },
  types:     { strict: true },
  test:      { isolate: true, clock: "deterministic", network: "fake" },

  permissions: { default: [] },               // per-package grants: §15
});
```

This replaces `package.json` scripts, `tsconfig.json`, `.eslintrc`, `.prettierrc`, `jest.config.js`, `vite.config.ts`, and any workspace-orchestrator config. Autocomplete and validation come from native types, not a bolted-on JSON schema.

Note: `runtime.engine` is deliberately **absent** — ADR-1 makes V8 non-swappable, so exposing it as config would imply a choice that does not exist.

### 18.1 Coexisting with the ecosystem — shadow configs (ADR-8)

Deleting `tsconfig.json`/`package.json` outright would break VS Code, WebStorm, CI, and thousands of tools that hardcode them. Resolved (ADR-8): the human edits **only** `meow.config.ts`; `meow` regenerates the legacy files the ecosystem expects.

- **`tsconfig.json`** — a committed one-line root shim, `{ "extends": "./.meow/tsconfig.json" }`; the real, regenerated content lives in `.meow/` and is also exactly what the delegated typechecker daemon (ADR-5) consumes.
- **`package.json`** — has **no `extends`**, so the shim trick cannot apply: `meow` generates the **real root `package.json`** as a derived artifact and owns it (publishing metadata is projected from `meow.config.ts`; hand-edits are clobbered on regeneration, and `meow doctor` warns).
- **Regeneration** runs on `meow install`, on config change (daemon/LSP watch), and via `meow sync` / `meow doctor`. `.meow/` is a gitignored build artifact, so CI must run `meow sync` (or `meow install`) to materialize the shadows; staleness is a checked failure mode (§24.5).

`meow` stays the single source of truth; the legacy ecosystem is transparently kept in sync. `package.json` may still hold publishing metadata, but it is a *projection* of `meow.config.ts`, not an authority.

---

## 19. CLI Surface

```sh
meow run <entry>            # execute a program (default mode = strict-web)
meow <file.ts>             # shorthand for run
meow dev                   # watch-mode run

meow install [--vfs|--materialize|--vendor]
meow add / remove <pkg>    # mutate dependencies + lockfile

meow task <name>           # run a typed task from meow.tasks.ts
meow test [filter]         # isolate-backed test runner

meow check                 # typecheck — delegated to the tsc/tsgo daemon (ADR-5)
meow lint                  # linter over the shared pipeline
meow fmt                   # formatter over the shared pipeline
meow bundle                # bundle via Rolldown over the Module Graph

meow why-slow | why-large  # observability
meow why-dep <pkg>         # dependency provenance / paths
meow trace <entry>         # execution trace
meow profile <entry>       # sampling/allocation profile
meow doctor                # environment / config / lockfile health
meow sync                  # regenerate shadow configs (.meow/tsconfig.json, root package.json) — ADR-8
```

Principles: commands are short and predictable; diagnostics explain causes, not just symptoms; every command understands the workspace graph and respects the same lockfile and capability model; every command reads **one** `defineMeow` config — no per-tool config files.

---

## 20. Editor & Language Server

`meow` needs a **first-party** language server — it is not an afterthought. It is how the zero-`node_modules` model becomes pleasant instead of mysterious. Responsibilities:

- Use the **same** parser and resolver as the runtime (ADR-4 — divergence here is a guaranteed bug).
- Provide native types for `meow:*` APIs (§8.2).
- Expose the virtual package cache to editors via the **Shadow `node_modules` Symlink Map** (§20.1), and understand the workspace graph.
- Surface diagnostics from the lint, type, dependency, and permission systems through the shared pipeline (§7.3).
- Surface `why-dep`, `why-large`, and capability warnings inside the IDE.

### 20.1 Exposing the virtual cache — Shadow `node_modules` Symlink Map (Q8, resolved)

Standard TS/JS language servers break without on-disk paths, yet the project root must stay clean (ADR-4). Resolution: the `meow` LSP writes a hidden **`.meow/deps/`** folder of **OS-level symlinks** that point directly into the global `~/.meow/cache/`, and the generated shadow `.meow/tsconfig.json` (ADR-8) adds a `paths` mapping to it. The IDE gets ordinary filesystem traversal and go-to-definition while the project root keeps **no** real `node_modules`. `.meow/` is gitignored and regenerated by `meow sync`/`meow install`, so the editor and the runtime resolver (ADR-4) never diverge.

---

## 21. Implementation Roadmap

Ordered by **dependency**, not by excitement. Each phase is shippable and demonstrable on its own; the research-grade pillar (security enforcement) is last. CJS/Node compatibility is deliberately deferred behind a working strict-web core, because pure-ESM dependencies can be resolved and run without the CJS interop layer.

### Phase 0 · Foundations
- **Deliverables:** Rust CLI skeleton; embed V8 (`deno_core`), event loop, op layer, and the **async I/O layer** (`tokio`; `io_uring` on Linux — ADR-9); stand up the Oxc parse pipeline as a query system (§7.2) — stages 1, 2, 5; lockfile schema draft; package-cache design; CJS synthetic-ESM-wrapper and artifact→task wiring (ADR-7) design research.
- **Acceptance:** run a trivial ESM file through V8; strip-and-run a trivial TS file; resolve one dependency from a content-addressed cache; V8 integration risks documented.

### Phase 1 · Minimal Runtime (the MVP, §22)
- **Deliverables:** `meow run`; ESM loader; TC39 type stripping (erasable-only, §9); `strict-web` standard globals (`fetch`, streams, WebCrypto); native types for early `meow:*` modules; first cut of `defineMeow` + shadow-config generation (`.meow/tsconfig.json` + root shim, ADR-8); first-party CJS rejection.
- **Acceptance:** a small TS web server runs with no emitted JS; behavior is byte-for-byte deterministic across two machines on the same version + lockfile; first-party CJS is rejected.

### Phase 2 · Packages & Resolution
- **Deliverables:** global content-addressed cache; deterministic lockfile (`meow.lock.jsonl`, §12.3); PnP in-memory loader **and `--materialize` together** (ADR-4, §24.4); npm registry resolution; integrity checks; `meow install`; `meow why-dep`.
- **Acceptance:** a project installs and runs (pure-ESM) dependencies with no `node_modules`; the same lockfile reproduces the same graph; runtime and LSP resolve packages identically.

### Phase 3 · Parse-Once Toolchain
- **Deliverables:** consolidate lint/format/check/bundle on the shared graph; `meow lint`, `meow fmt`, `meow bundle`; `meow check` via the **delegated `tsc`/`tsgo` daemon** plus a fast in-process type-aware linter (ADR-5); shared diagnostics model.
- **Acceptance:** a file is parsed once and reused across multiple tool operations in one invocation; lint/format/check/bundle agree on module resolution; formatter round-trips trivia and common TS syntax.

### Phase 4 · Workspaces & Tasks
- **Deliverables:** workspace-graph discovery; internal package linking; `meow.tasks.ts` with input/output hashing, caching, and parallel execution reusing Phase 0 incrementality; **artifact→task wiring** so importing a missing/stale build output (e.g., `./crypto.wasm`) triggers its producing task (ADR-7); `meow task`.
- **Acceptance:** a multi-package workspace builds in dependency order; unchanged tasks are skipped; affected-package detection works from source changes.

### Phase 5 · Node Compatibility & Legacy
- **Deliverables:** `node-compat` mode + Node built-in compatibility layer; CJS dependency analysis + synthetic ESM wrappers (§11.1); `legacy` mode; hardened materialized install.
- **Acceptance:** common modern npm packages run in `node-compat`; legacy CJS deps import cleanly from first-party ESM; local first-party CJS is *still* rejected.

### Phase 6 · Security, Testing & Observability
- **Deliverables:** capability policy + runtime enforcement at the **process-level honest default** (§15.3); provenance metadata + anomaly **detection** (advisory, low-risk); isolate-backed `meow test` with deterministic clock + fake network; `why-slow`, `why-large`, `trace`, `profile`, `doctor`.
- **Acceptance:** tests cannot leak mutable state across isolates; a dependency without network capability cannot reach the network (at the chosen boundary); `why-large` explains the largest bundle contributors; `trace` shows module load, permission checks, and timing.

### Phase 7 · Security Enforcement — Trust Zones (ADR-6)
- **Deliverables:** ship the committed **tier-2 Trust Zones** — per-package capability via load-time AST rewriting + lexical interposition; keep the **tier-3 escalation** (intrinsic-freezing membranes / context-or-isolate-per-zone) as a flagged research track for adversarial soundness.
- **Acceptance:** a dependency without a capability cannot reach it through *static* code paths at near-zero overhead; the documented threat model states plainly what tier 2 does and does not stop (dynamic/reflective access → tier 3); the model is claimed only as strongly as it is enforced.

**Stretch / post-1.0:** WebAssembly Component Model imports (§10); VFS/FUSE strategy (§12.2); deeper type-aware lint rules on the semantic graph; tier-3 security escalation (SES / context-or-isolate-per-zone, §15.3).

---

## 22. Minimum Viable Product

The MVP (≈ Phase 1 + the cache/lockfile slice of Phase 2) is deliberately narrow.

**In:** Rust CLI; V8 runtime; `meow run`; ESM-only first-party execution; TS type stripping (erasable syntax); `strict-web` mode; minimal `fetch`/Web APIs; basic `defineMeow`; content-addressed cache; lockfile; PnP in-memory resolution; first-party CJS rejection.

**Out:** full Node/npm compatibility; full formatter/linter/bundler/task-runner/test-runner; Wasm Component Model; Rust source imports; complete config replacement.

**MVP succeeds if** developers can run a small TypeScript ESM service with deterministic dependency resolution and no local `node_modules`.

---

## 23. First 90 Days

- **Days 1–30:** Rust CLI skeleton; embed V8 and run a basic script; ESM entrypoint loading; TS stripping for a narrow subset; draft lockfile + `defineMeow` schemas; reject first-party CJS.
- **Days 31–60:** basic package cache; resolve dependencies from cache; `meow install`; PnP in-memory resolution; initial Web-standard globals; basic `meow:http`; start the LSP resolution prototype.
- **Days 61–90:** run a small TS service with dependencies and no `node_modules`; deterministic lockfile verification; minimal `meow doctor`; early `meow why-dep`; workspace-graph discovery prototype; publish MVP architecture notes and documented compatibility limits.

---

## 24. Major Technical Risks & Honest Constraints

Each entry is a place where the headline could mislead. Entries marked **Resolved** were decided by ADR-5–9 and now read as *resolved → residual risk* — the decision is made, but the honest caveat that survives it is stated. Unmarked entries remain open risks with mitigations. The spec is only useful if these are stated plainly.

- **24.1 — Performance. Resolved (ADR-9) → residual.** The win is startup/toolchain + I/O/FFI, never steady-state JS compute (V8 *is* Node's engine; hot-loop math is identical). *Residual:* any I/O multiplier (e.g., "3× I/O") is **workload-specific** — plausible for concurrent network / many-file reads / WebSockets / FFI-heavy paths, but **not** for trivial sequential reads where `libuv` is already optimal — and `io_uring` is Linux-only (`kqueue`/IOCP elsewhere). Every headline number must name its workload and ship a reproducible benchmark.

- **24.2 — Typechecking. Resolved (ADR-5) → residual.** No native Rust typechecker is built: execution is Oxc stripping (instant); diagnostics are a delegated `tsc`/`tsgo` daemon plus a fast in-process type-aware linter. *Residual:* `meow check` runs at reference-compiler speed and is **not** part of the "10× toolchain" claim; the fast path depends on `tsgo` maturity (fallback `tsc`, correct-but-slower); the in-process linter is a *fast preview* that can disagree with the daemon and is never authoritative.

- **24.3 — Per-package security. Partially resolved (ADR-6) → residual is prominent.** Tiered: process-level default (sound, coarse) → AST-rewrite Trust Zones (per-package, near-zero overhead) → membrane/isolate escalation (research). *Residual (the project's highest-risk security claim):* static rewriting is **not** sound against adversarial *dynamic* access (`globalThis["fe"+"tch"]`, `Function("return fetch")()`, dynamic `import()`, `eval`). Market tiers 1–2 as "defense-in-depth," **never** "mathematically blocked," until the tier-3 escalation ships and is adversarially tested.

- **24.4 — PnP breaks filesystem-assuming tools.** Real cost, not theoretical (certain bundlers, native-addon toolchains, IDE plugins). *Mitigation:* `--materialize` ships **with** PnP (Phase 2), not after; provide `vendor` too; emit clear diagnostics when a tool needs real files; first-party LSP so the common path doesn't need materialization.

- **24.5 — Config singularity. Resolved (ADR-8) → residual.** Single source `meow.config.ts`; generated shadow `.meow/tsconfig.json` (root `extends` shim) plus a generated root `package.json` (no `extends`, so `meow` owns it). *Residual:* generated files are build artifacts — `.meow/` is gitignored, CI must run `meow sync`/`meow install`, hand-edits to the root `package.json` are clobbered on regen, and staleness is a real failure mode `meow doctor` must catch.

- **24.6 — Native code. Resolved (ADR-7) → residual.** No source imports and no embedded `rustc`/`cargo`/clang (keeps the binary < 50 MB); `.wasm` is produced by a graph-wired `build:wasm` task and auto-rebuilt when stale. *Residual:* the compile step requires the host toolchain (CI must provision it) — building from source is not hermetic w.r.t. toolchain availability; only the resulting `.wasm` is pinned.

- **24.7 — Determinism vs. native binaries.** "Mathematically pinned native binaries" (§12.3) means reproducible **selection** (content hash per platform), not reproducible **builds**. *Mitigation:* be explicit — we pin what we fetch; we do not rebuild the world. Cross-OS bit-for-bit determinism of arbitrary native deps is out of scope.

- **24.8 — V8 maintenance. Resolved (Q13) → residual.** `meow` consumes **unmodified upstream `deno_core` + `rusty_v8`** — Deno already does the multi-platform static V8 build and publishes the bindings — and **never forks V8**, keeping `cargo build` fast (target < 1 min) and offloading the C++ maintenance. *Residual:* tracking V8 is still a permanent cost — we absorb upstream bumps and V8 API churn on Deno's cadence (semver carve-out for engine bumps, §26.3); keep the host API narrow until the loader stabilizes.

- **24.9 — CommonJS compatibility sinkhole.** Perfect CJS interop can consume unlimited effort. *Mitigation:* keep first-party CJS banned; prioritize dependency compatibility by package impact; make `legacy` explicit and diagnostic-heavy; surface exactly which dependency forced legacy behavior.

- **24.10 — Scope explosion.** `meow` wants to be runtime + package manager + test runner + bundler + linter + formatter + profiler + workspace orchestrator. *Mitigation:* sequence by foundation, not marketing value (§21); ship narrow slices that prove the shared architecture; never build every tool surface before the shared graph works.

- **24.11 — Security-policy usability.** Capability systems become noisy or unconfigurable. *Mitigation:* good defaults; generate suggested permissions from observed behavior; require confirmation for privilege expansion; specific, actionable warnings.

---

## 25. Project Invariants

These remain true unless the project explicitly re-charters.

1. `meow` is Rust-native.
2. `meow` embeds V8 (no engine abstraction).
3. `meow` defaults to Web-standard APIs.
4. First-party source is ESM-only.
5. Dependency CommonJS is compatibility-only.
6. No local `node_modules` by default.
7. A global content-addressed cache.
8. One typed config file.
9. All tools share the canonical project graph.
10. Capabilities and provenance are runtime concerns.
11. No native TypeScript typechecker — diagnostics are delegated to the reference compiler (ADR-5).
12. No embedded language toolchains (`rustc`/`cargo`/clang); the binary stays small — target < 60 MB (ADR-7, §26.1).
13. `meow.config.ts` is the only human-edited config; legacy `tsconfig.json`/`package.json` are generated projections (ADR-8).
14. Performance is claimed on startup, toolchain, I/O, and FFI — never on steady-state JS compute (ADR-9).

---
## 26. Build, Distribution & Versioning

How `meow` is compiled, shipped, and versioned. These three decisions (Q12–Q14) are intertwined: a single static binary embedding upstream V8, versioned so engine bumps don't break users.

### 26.1 Distribution — single static binary (Q14, resolved)

`meow` ships as **one statically-linked native binary** per platform, with V8 and the Rust runtime linked in; users install **no external dependencies**. Channels: `curl | sh`, Homebrew, and Winget. **Size target < 60 MB.** Honest note: V8 itself is tens of MB, so < 60 MB is an aggressive *budget* — it refers to the compressed download (on-disk is larger), and comparable V8 runtimes ship bigger, so this number is tracked, not assumed.

### 26.2 V8 bindings — consume upstream, never fork (Q13, resolved)

`meow` uses **unmodified upstream `deno_core` + `rusty_v8`**. Deno already performs the multi-platform static V8 compilation and publishes prebuilt bindings, so `meow` consumes them directly: no V8-from-source in CI, `cargo build` under a minute, and the heavy C++ maintenance is offloaded. `meow` **never forks V8**. (Residual maintenance cost: §24.8.)

### 26.3 Versioning & compatibility promise (Q12, resolved)

**SemVer governs the public surface — `meow:*` APIs, Web/global APIs, and CLI commands — not the internal engine.**

- Bumping the embedded V8 within a **minor** release is allowed (e.g., `meow 1.2 → 1.3` may move V8 12.0 → 12.4). If V8 removes a non-standard JS quirk, that is **not** a `meow` breaking change.
- Changing the signature of a `meow:*` API (e.g., `import { serve } from "meow:http"`), removing a CLI command, or dropping a guaranteed Web API **requires a major** bump.
- The `meow.lock.jsonl` runtime-version constraint (§12.3) records which `meow` line a project expects, so an engine bump stays reproducible.

---

## 27. Resolved Decisions (Decision Ledger)

Every design question is now resolved — there are **no open questions**. This ledger keeps each `Q-ID` with its resolution for traceability (ADR pointers where a question became an architectural decision; section pointers otherwise).

- **Q1 — Security enforcement boundary. ✅ Resolved → ADR-6** (tiered: process-level default → AST-rewrite Trust Zones → membrane/isolate escalation; §15.3).
- **Q2 — Typecheck strategy. ✅ Resolved → ADR-5** (delegate diagnostics to a `tsc`/`tsgo` daemon; native stripping for execution; fast in-process type-aware linter; §9.1).
- **Q3 — Config authority. ✅ Resolved → ADR-8** (single human-edited source + generated shadow configs; §18.1).
- **Q4 — Lockfile format. ✅ Resolved** → `meow.lock.jsonl`, strictly-sorted JSON-lines; no pnpm/npm interop (§12.3).
- **Q5 — CJS wrapper minimum. ✅ Resolved** → `cjs-module-lexer` named-export extraction + default `module.exports` + mocked `__dirname`/`__filename`; dynamic deep `require` → `legacy` diagnostic (§11.1).
- **Q6 — TS erasable subset. ✅ Resolved** → strict TC39 erasable syntax only; `enum`/namespace/parameter-properties error with a fix message (§9).
- **Q7 — WinterTC subset. ✅ Resolved** → the Phase-1 "Stateless Edge" subset (§8.1).
- **Q8 — LSP package exposure. ✅ Resolved** → Shadow `node_modules` Symlink Map (`.meow/deps/` → global cache) + shadow-tsconfig `paths` (§20.1).
- **Q9 — Provenance sources. ✅ Resolved** → npm-registry Sigstore + the OSV database (§15.4).
- **Q10 — `why-slow` first subset. ✅ Resolved** → the Module Load/Parse Timeline waterfall (§17).
- **Q11 — Wasm staging. ✅ Resolved → ADR-7** (no source imports; task-driven `.wasm`; Component Model post-1.0; §10).
- **Q12 — Compatibility promise. ✅ Resolved** → SemVer on `meow:*` / Web APIs / CLI; engine-version carve-out (§26.3).
- **Q13 — V8 binding strategy. ✅ Resolved** → unmodified upstream `deno_core`/`rusty_v8`, never fork (§26.2, §24.8).
- **Q14 — Distribution. ✅ Resolved** → single statically-linked binary, < 60 MB target, `curl|sh` / Homebrew / Winget (§26.1).

---

## 28. Success Metrics

- **Early:** runs TS ESM with no emitted JS; installs/resolves packages without `node_modules`; rejects first-party CJS consistently; reproduces execution from lockfile across machines; provides editor types for native `meow:*` APIs; shadow configs make VS Code/CI work with zero manual duplication (ADR-8).
- **Medium-term:** runs common npm dependencies in `node-compat`; builds and tests multi-package workspaces; shares parse work across check/lint/format/bundle; `meow check` matches reference-compiler correctness via the delegated daemon (ADR-5); demonstrates substantially faster I/O and FFI than Node on named, reproducible benchmarks (ADR-9); explains dependency presence and bundle cost better than existing tools; enforces capabilities (tier 1–2, ADR-6) without making normal development painful.
- **Long-term:** a credible default runtime for new TS services; `node_modules` optional for most projects; dependency behavior inspectable and enforceable; JS tooling feels like one system instead of a pile of configs.

---

## 29. Glossary

- **WinterTC** — the standards body (formerly WinterCG) defining a common server-side/edge JS API surface (`fetch`, streams, etc.). "Standards-first" = targeting this.
- **Oxc** — Rust JS/TS toolchain (parser, semantic analyzer, transformer) underpinning pipeline stages 1, 2, 5.
- **Rolldown** — Rust bundler (Rollup-compatible) underpinning the Module Graph / `meow bundle`.
- **`deno_core` / `rusty_v8`** — Rust crates embedding V8 and providing the op/extension model.
- **PnP (Plug'n'Play)** — package resolution with no `node_modules`; the loader maps imports to a cache in memory.
- **Content-addressed cache** — package storage keyed by content hash, shared across projects, integrity-checked.
- **Type stripping** — TC39 approach erasing TS annotations to produce valid JS without transpilation; works on *erasable* syntax only.
- **Membrane / SES** — object-capability isolation techniques (cf. Endo, LavaMoat) for sandboxing code that shares one heap.
- **Component Model / WIT** — WebAssembly's typed component/interface system for language-agnostic modules.
- **Isolate** — an independent V8 heap/context; cheap to create; the basis for per-package security and isolate-per-test.
- **CST / Lossless Syntax Tree** — concrete syntax tree retaining all trivia (whitespace, comments) so formatting/codemods round-trip.
- **Trust Zone (ADR-6)** — a per-package capability boundary enforced by load-time AST rewriting + lexical interposition (no separate isolate); `meow`'s committed tier-2 security mechanism.
- **Shadow config (ADR-8)** — a legacy config file (`.meow/tsconfig.json`, root `package.json`) generated from `meow.config.ts` so non-`meow` tools and IDEs work unchanged.
- **`typescript-go` / `tsgo`** — Microsoft's Go port of the TypeScript compiler; `meow`'s fast delegated-checking path (fallback: `tsc`).
- **V8 context** — an independent global environment *within* one isolate; far cheaper than a separate isolate, and the implementation avenue for Trust Zones.
- **`meow.lock.jsonl`** — the lockfile: strictly-sorted JSON-lines (one dependency per line) for git-merge resistance; tracks versions, content hashes, capability grants, and Wasm artifacts (§12.3).
- **Sigstore** — signing/transparency system; `meow` verifies npm / GitHub Actions provenance signatures at install (§15.4).
- **OSV** — Open Source Vulnerabilities database, mirrored into the global cache to flag known-bad hashes locally (§15.4).
- **Stateless Edge subset** — the minimal Phase-1 Web-API set (`fetch`, `Response`, `crypto.subtle`, …) needed to run servers/workers; no DOM/`window`/`localStorage` (§8.1).
- **Shadow `node_modules` Symlink Map** — `.meow/deps/` of OS symlinks into the global cache, exposing packages to editors without a real root `node_modules` (§20.1).

---

## 30. Canonical One-Sentence Description

> **meow is a Rust-built, V8-powered, ESM-first JavaScript and TypeScript runtime that unifies execution, packages, workspaces, tasks, testing, security, observability, and tooling around one deterministic project graph.**
