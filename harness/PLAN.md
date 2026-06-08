# Plan — meow

> The middle of the spine: `CONSTITUTION → **PLAN** → spec → commit → gate → surface`.
> Keep it terse — the Plan is a map, not a design doc. Specs carry the detail; each spec's
> `plan_ref` must point at a section here (enforced by `principles-check.sh`).
>
> The Plan **drifts** as reality teaches you things. That is expected and fine. The **intent-gate**
> (GATES.md) re-derives the built surface against this Plan and raises a `drift_flag` when they diverge —
> the Plan is a living target, not a frozen contract.

---

## What & when

**Product:** a standards-first JavaScript/TypeScript **runtime + unified toolchain in one Rust binary** (runtime · packages · workspaces · tasks · tests · bundle · lint · format · check · security · observability) built on V8, all reading one canonical project graph. Full architecture, ADRs, and resolved decisions live in **`harness/CANON.md`** (the operator's canonical planning document); this Plan is the build map over it.
**Definition of "shipped" (MVP, CANON §22):** a developer can `meow run` a small TypeScript **ESM** web service that uses dependencies, with **no `node_modules`**, deterministic lockfile-pinned resolution, type stripping (no emitted JS), and identical behavior across two machines on the same `meow` version + lockfile.
**Mode:** `incubating` (flip to graduated only per CONSTITUTION A8).
**Stack:** Rust — one **Cargo workspace (monorepo)**, subsystems are crates; embeds V8 via upstream `deno_core`/`rusty_v8`; parse/transform via Oxc; bundling via Rolldown. **Prod target:** none yet.

---

## Subsystem → spec-prefix map

| Prefix | Subsystem | Owns | Backend lean | Constitution refs |
|--------|-----------|------|--------------|-------------------|
| `GRAPH` | Parse & graph pipeline | Oxc CST → semantic → module graph → runtime IR; incremental query system (CANON §7) | omp | I-1, I-3 |
| `RT` | Runtime | V8 embedding, isolates, event loop, op layer, async I/O, Web/WinterTC globals, type-stripping execution, `meow:*` surface | omp | I-3, I-6, I-9, I-10 |
| `LOAD` | Module loader & resolution | ESM resolution, CJS→ESM wrapping, virtual package access, lockfile-at-load; the one shared resolver | omp | I-1, I-2, I-5, I-7 |
| `PKG` | Package manager | content-addressed cache, `meow.lock.jsonl`, install modes, npm registry, integrity, provenance metadata | omp | I-5, I-7, I-8 |
| `WS` | Workspace graph | monorepo discovery, internal package linking, affected detection | omp | I-1 |
| `TASK` | Task runner | typed `meow.tasks.ts`, input/output hashing, graph-aware caching, artifact→task wiring | omp | I-1, I-6 |
| `TEST` | Test runner | isolate-per-file, deterministic clock + fake network + seeded randomness, capability-scoped | omp | I-6, I-8 |
| `TOOL` | Lint · format · bundle | linter, formatter, Rolldown bundler — all over the shared graph | omp | I-1, I-2, I-11 |
| `CHK` | Typecheck & diagnostics | delegated `tsc`/`tsgo` daemon, `meow check`, fast-preview type-aware linter, shadow-tsconfig consumption | any | I-4, I-11 |
| `LSP` | Language server | shared parser/resolver, `meow:*` types, shadow `node_modules` symlink map, IDE diagnostics | claude | I-1, I-5, I-9 |
| `SEC` | Security | capability model + tiered enforcement (process → Trust Zones), provenance verify (Sigstore/OSV), anomaly detection | claude | I-7, I-8, I-11 |
| `OBS` | Observability | `why-slow`/`why-large`/`why-dep`/`trace`/`profile`/`doctor` | claude | I-1, I-7, I-11 |
| `CFG` | Configuration | `defineMeow`, shadow-config generation (`.meow/` + root `package.json`/`tsconfig.json`), `meow sync` | claude | I-1, I-9 |
| `DIST` | Build & distribution | static single-binary build, upstream V8 binding pinning, SemVer/versioning, install channels | omp | I-10 |
| `H` | harness chores | tooling, hooks, CI, docs | any | A1–A9 |

---

## Build sequence (phases)

Ordered by dependency (CANON §21), not excitement; each phase is independently demonstrable and exits on a reality-checkable condition bound to an invariant gate.
> ⚠ **Amendments 001/002 (CANON) re-order this.** Drop-in compatibility + Tier-1 UX now lead. A new **P2.5 · Drop-In Core** is inserted before P3 (it pulls the former Phase-5 node-compat + CJS work forward), and sequencing within every later phase follows the success tiers (Tier-1 `install`/`run`/`test`/`dev` first). The dependency order below is preserved for history; the amendment overrides *priority*, not the dependency facts.


- **P0 · Foundations** — Rust CLI skeleton; embed V8 (`deno_core`) + event loop + op layer + async I/O (`tokio`; `io_uring` on Linux); stand up the Oxc pipeline as an incremental query system (stages 1/2/5); draft `meow.lock.jsonl` + cache layout; first shadow-config + `defineMeow` cut.
  - *Exit:* a trivial ESM file runs through V8; a trivial TS file strips-and-runs; one dependency resolves from the content-addressed cache; `principles-check` green. Gates: `strip-fidelity`, `graph-integrity`, `footprint` (initial).
- **P1 · Minimal Runtime (MVP)** — `meow run`; ESM loader; TC39 type stripping (erasable-only); `strict-web` "Stateless Edge" globals (`fetch`/`Response`/`Headers`/`URL`/`crypto.subtle`/`TextEncoder`/`AbortController`/`Blob`/`FormData`/`setTimeout`); early `meow:*` + generated types; first-party CJS rejection.
  - *Exit:* a small TS web server runs with no emitted JS; behavior is identical across two machines on the same version+lockfile; first-party CJS is rejected. Gates: `determinism` (happy path), `compat` (CJS refusal), `strip-fidelity`, `types-fresh`.
- **P2 · Packages & Resolution** — content-addressed cache; deterministic `meow.lock.jsonl`; PnP in-memory loader **and `--materialize` together** (ADR-4); npm registry resolution; integrity checks; `meow install`; `meow why-dep`.
  - *Exit:* a project installs and runs pure-ESM dependencies with no `node_modules`; the same lockfile reproduces the same graph; runtime and LSP resolve identically. Gates: `resolver-parity`, `lockfile-integrity`.
- **P2.5 · Drop-In Core (Amendments 001/002 — the adoption spine, Tier-1)** — `package.json` becomes the dependency/scripts/workspaces authority (`meow` reads it; `meow add`/`remove` mutate it; the `meow.config.dependencies` field + config→package.json generation are retired); native Node built-ins (`fs`/`path`/`process`/`Buffer`/`node:*`), always-on; first-party CJS via `require`-interception + Oxc synchronous wrap; `meow run <script>` / `meow <script>` / `meow dev`. Default mode flips to drop-in; `strict-web` becomes opt-in.
  - *Exit (the 5-minute test):* `cd existing-project && meow install && meow run dev` runs a stock npm/Bun app with **zero file edits**, and feels nicer than what it replaced. Gates: `drop-in` (real-project corpus installs + runs unmodified), `resolver-parity`, `determinism` (opt-in/frozen).
- **P3 · Parse-Once Toolchain** — consolidate lint/format/check/bundle on the shared graph; `meow lint`/`fmt`/`bundle`; `meow check` via the delegated `tsc`/`tsgo` daemon + fast-preview linter; shared diagnostics model.
  - *Exit:* a file is parsed once and reused across multiple tool operations in one invocation; lint/format/check/bundle agree on resolution; `meow check` matches the reference compiler. Gates: `graph-integrity`, `check-parity`.
- **P4 · Workspaces & Tasks** — workspace-graph discovery + internal linking; `meow.tasks.ts` with input/output hashing, caching, parallel execution; artifact→task wiring (importing a stale `./x.wasm` runs its producing task).
  - *Exit:* a multi-package workspace builds in dependency order; unchanged tasks are skipped; affected-package detection works. Gates: `determinism` (caching), `graph-integrity`.
- **P5 · Node Compatibility & Legacy** — `node-compat` + Node built-in layer; CJS dependency analysis + synthetic-ESM wrappers (`cjs-module-lexer`); `legacy` mode; hardened materialized install.
  - *Exit:* common modern npm packages run in `node-compat`; legacy CJS deps import cleanly from first-party ESM; first-party CJS is still rejected. Gates: `compat` (both directions).
- **P6 · Security, Testing & Observability** — capability policy at the **process-level honest default**; provenance metadata + anomaly **detection** (advisory); isolate-backed `meow test` (deterministic clock + fake network); `why-slow`/`why-large`/`trace`/`profile`/`doctor`.
  - *Exit:* tests cannot leak state across isolates; a dependency without a grant cannot reach the resource at the process tier; `why-large` explains the largest contributors; `trace` shows load/permission/timing. Gates: `capability` (tier-1), `determinism` (test clock), `honesty` (threat-model doc).
- **P7 · Security Enforcement — Trust Zones** — ship tier-2 Trust Zones (per-package capability via load-time AST rewrite + lexical interposition); keep tier-3 (membranes / context-or-isolate-per-zone) as a flagged research track.
  - *Exit:* a dependency without a capability cannot reach it via static code paths at near-zero overhead; the threat model states plainly what tier-2 does and does not stop; the claim is only as strong as the enforcement. Gates: `capability` (tier-2), `honesty`.
- **Stretch / post-1.0** — Wasm Component Model imports; VFS/FUSE; deeper type-aware lint on the semantic graph; tier-3 security escalation.

Continuous gates (run by blast radius, not phase): `footprint` (every `DIST` change), `types-fresh` (every `meow:*`/`CFG` change), `honesty` (every product-facing surface change).

---

## Dependencies & risk notes

- `GRAPH` + `RT` underpin everything; `LOAD`/`PKG` precede `TOOL`/`CHK`/`WS`; `CFG` shadow-gen precedes the `CHK` daemon (it consumes `.meow/tsconfig.json`); `LSP` must reuse `LOAD`'s resolver (I-5 — divergence is a guaranteed bug).
- `--materialize` ships **with** PnP in P2, not after (CANON §24.4) — PnP breaks filesystem-assuming tools and the escape hatch is required, not polish.
- Security enforcement (P7) is the research-grade pillar and is **last**; until tier-3, market tiers 1–2 as defense-in-depth only (I-8, CANON §24.3).
- Biggest redesign risks (CANON §24): `tsgo` maturity for `CHK` (fallback `tsc`); CJS interop depth for `LOAD`/`PKG` (keep `legacy` explicit); V8 version churn for `DIST`/`RT` (track upstream, never fork).
- Drift is normal — the intent-gate flags Plan/reality divergence; resolve via a DR, then reconcile this map. Do not retro-edit to hide divergence.

---

## Initial spec backlog

Concrete enough for the spec-drafter to start. Wave 1 = P0; Wave 2 = P1 (MVP); later phases sketched at coarser grain.

**Wave 1 (P0 · Foundations)**
- `DIST-001` — Cargo workspace + `meow` CLI skeleton; pin unmodified upstream `deno_core`/`rusty_v8`; static release build; binary-size budget probe (I-10).
- `RT-001` — Embed V8: isolate, event loop, op/extension layer (I-6, I-10).
- `RT-002` — Async I/O layer (`tokio`; `io_uring` on Linux, `kqueue`/IOCP elsewhere) (I-6).
- `GRAPH-001` — Oxc parse pipeline as an incremental query system: stages 1 (CST), 2 (semantic), 5 (runtime IR) (I-1).
- `RT-003` — TC39 type stripping (erasable-only) on the hot path; non-erasable constructs error with a fix message (I-3).
- `LOAD-001` — ESM loader + content-addressed cache read for a single dependency (I-1, I-5).
- `PKG-001` — `meow.lock.jsonl` schema (strictly-sorted JSON-lines) + global cache layout (I-7).
- `CFG-001` — `defineMeow` schema + shadow `.meow/tsconfig.json` generation + committed root `tsconfig.json` shim (I-1, I-9).

**Wave 2 (P1 · Minimal Runtime / MVP)**
- `RT-004` — `strict-web` "Stateless Edge" global subset (CANON §8.1) (I-6).
- `RT-005` — `meow:http serve` + native `meow:*` type generation from the implementation (I-9).
- `RT-006` — Determinism/hermeticity harness: gate env/clock/randomness behind capabilities (I-6).
- `LOAD-002` — First-party CJS rejection in all modes (I-2).
- `LOAD-003` — Full Node resolution algorithm (conditions, `exports`/`imports`, self-references) shared by runtime + LSP (I-5).
- `CFG-002` — Generated root `package.json` projection from `meow.config.ts` (I-9).

**Later phases (sketch — Amendment 001/002 priority order)**
- **P2.5 · Drop-In Core (NOW — Tier 1, the adoption spine):** `CFG-003` **package.json = dependency authority** (read deps from `package.json`; `meow add`/`remove` mutate it; retire the `meow.config.dependencies` field + the `CFG-002` config→package.json *generation*); `RUN-001` `meow run <script>` / `meow <script>` / `meow dev` (package.json scripts runner); `RT-007` native Node built-ins (`fs`/`path`/`process`/`Buffer`/`node:*`), pulled forward from P5; `LOAD-004` first-party + dependency CJS interop (`require`-interception → Oxc wrap → execute; `cjs-module-lexer`), pulled forward from P5.
- P3 (Parse-Once Toolchain — backlog drafted, reconcile vs. pivot): `TOOL-001` formatter; `TOOL-002` linter (**`no-first-party-cjs` flips error→advisory/off per Amendment 001**; erasable-only becomes opt-in); `TOOL-003` Rolldown bundler; `CHK-001` delegated `meow check`; `CHK-002` fast-preview linter.
- P4: `WS-001` workspace discovery/linking (reads `package.json` `workspaces`); `TASK-001` typed tasks (OPTIONAL layer atop package.json scripts); `TASK-002` artifact→task (Wasm) wiring.
- P5 (residual after pull-forward): `LOAD-005` `legacy` mode; hardened materialized install.
- P6: `SEC-001` capability model + process-tier enforcement; `SEC-002` provenance (Sigstore/OSV) + anomaly detection; `TEST-001` isolate-per-file `meow test` (**Tier 1** — bring forward as deps allow; interim `meow test` = the package.json `test` script via RUN-001); `OBS-002` `why-slow` timeline (**Tier 2**); `OBS-003` `trace`/`profile`/`doctor` (**Tier 2**).
- P7: `SEC-003` Trust Zones (load-time AST rewrite + interposition); `SEC-004` threat-model doc + tier-3 research flag.
