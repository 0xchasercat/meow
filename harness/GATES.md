# Gates — meow

> Definition of done. The spine's penultimate link: `… → commit → **gate** → surface`.
> **Reality is the gate** (CONSTITUTION A4): "tests pass" is not "works" — and "works" is not "well-built".
> Two tiers: a cheap **local floor** (pre-merge, necessary but never sufficient) and an adversarial **ceiling**
> that asks the three questions a green checkmark can't — *does it work* (reality), *is it what we wanted*
> (intent), and *is it well-built* (craft) — re-deriving each independently and never trusting the floor.
>
> Every gate writes a verdict to `state.json.gates` as `{ status, at, evidence_ref }`
> (`status ∈ green|yellow|red|unknown`). The verdict is data; the *evidence* is a markdown audit doc in
> `harness/reality-checks/`. `evidence_ref` points at the doc — never paste evidence into the JSON (A3).

---

## Tier 1 — the local floor (cheap, fast, pre-merge)

Runs in the sub-agent's worktree before the orchestrator integrates. Mechanical, deterministic, seconds.
A red floor blocks the merge; it is necessary but **never sufficient**.

| Check | Command | Blocks merge |
|-------|---------|--------------|
| lint | `cargo clippy --all-targets --all-features -- -D warnings` | yes |
| typecheck | `cargo check --all-targets` | yes |
| test | `cargo test --workspace` | yes |
| principles | `harness/principles-check.sh` | yes |
| build | `cargo build --release` | yes |
| format | `cargo fmt --all -- --check` | yes |

`principles-check.sh` is the grep gate: every spec carries non-empty `plan_ref` + `constitution_ref`,
comment-marker fences are balanced, no banned patterns (see PRINCIPLES.md). It is the floor's spine check.

---

## Tier 2 — the adversarial ceiling (auditors re-derive reality)

Run by **auditor** roles, not the implementer. They start from the real running system and external
sources of truth — never the floor's green checkmarks, never the implementer's claims. Output: a dated
audit doc in `harness/reality-checks/` plus the `state.json.gates` verdict.

### `reality` gate — *is it actually true?*
Re-derives the product's claimed behavior against **real services and real data**.
- Hit live endpoints / real DB / real third-party APIs; do not mock.
- Re-compute claimed numbers from source; reproduce the user-visible happy path end to end.
- Probe the failure modes the spec promised it handles.
- Verdict `green` only if reality matches the claim. Discrepancy → `red` + a `drift_flag`.
- Evidence: `harness/reality-checks/round-<N>.md`.

### `intent` gate — *is it what we wanted?*
Re-derives the **built surface** against `PLAN.md` + `CONSTITUTION` (+ mockups, if any). Catches the
"correct but not what we meant" drift the reality gate can't see.
- Diff shipped behavior vs each `plan_ref` it claims to satisfy and the constitution invariants it cites.
- Plan drift is expected (PLAN.md is a living map) — record it as a `drift_flag`, resolve via a DR, then
  reconcile the Plan. Do not silently retro-edit the Plan to hide the divergence.
- Evidence: `harness/reality-checks/intent-<N>.md`.

### `craft` gate — *is it well-built?*
The question a green checkmark cannot answer. A change can pass every floor check, work, and be exactly what
we asked — and still be the wrong way to build it (the 102-case `if`, the leaky abstraction, the copy-paste,
the hand-roll of a solved problem). Run by the **`craft-auditor`** against `CRAFT.md` — **judgment, not a
checklist**: *"is this the decision a principled senior owner of this codebase would make?"* We do not enforce
rules ("use reusable components"); we enforce the judgment that makes good structure appear by default.
- Reads the diff in real context; high-blast-radius changes get a **panel** of diverse lenses.
- Findings carry `{ severity, where, why-it's-wrong, the-better-decision }`; any `block` finding stops the merge.
- Loop: author fixes → re-review, until a principled reviewer *would approve*.
- Runs on **every** change — quality must hold whether or not a human ever reads the code. (CONSTITUTION A10)
- Verdict: `state.json.gates.craft`; evidence: `harness/reality-checks/craft-<N>.md`.

### `product` gate — *is it the right product decision?* (the one that runs BEFORE the build)
The only gate that fires at the **plan/approach stage, before implementation** — because catching a confusing
or incoherent product decision *after* it's built is rework and incoherence. Run by the **`product-steward`**
against `PRODUCT.md` — **judgment, not a checklist**: *"is this the decision a thoughtful product owner who
understands the customer would make?"* Everything built is a product; every user-facing/product-shaping change
is reviewed from the customer/UX/cohesion angle before any code.
- Reviews the **approach/plan** (spec, intended UX/IA, chosen shape) — not a finished diff. Required for
  user-facing/product-shaping work; a fast no-op for purely-internal mechanical changes.
- Concerns carry `{ severity, where, why-it-misleads-or-fragments-the-product, the-better-decision }`; a `block`
  concern **reshapes the approach before any build**; builder revises → steward re-checks.
- Genuine product forks with no clear answer → a concise operator decision (the few a product owner must make),
  not a guess or a stall.
- Builders raise the product questions **proactively** themselves up front; the steward is the backstop. (CONSTITUTION A11)
- Verdict: `state.json.gates.product`; evidence: `harness/reality-checks/product-<N>.md`.

---

## Project-specific gates

> The gates the meow invariants demand. Each re-derives reality independently and is bound to a CONSTITUTION Part B invariant. Verdicts land in `state.json.gates`; evidence in `harness/reality-checks/`.

| Gate | Asserts (invariant) | How it re-derives reality | Evidence |
|------|---------------------|---------------------------|----------|
| `graph-integrity` | I-1 | Assert a single parse pipeline + single resolver entrypoint; trace lint/format/check/bundle/runtime to the shared graph; no second parser/resolver instantiated | reality-checks/graph-integrity-<N>.md |
| `compat` | I-2 | Author a first-party `.cjs`/`require()` → must be rejected in every mode; a pure-ESM (and, from P5, a legacy CJS) dependency imports cleanly | reality-checks/compat-<N>.md |
| `strip-fidelity` | I-3 | Property test: `strip → reparse` equals source AST minus types, source positions stable; `enum`/namespace/param-props/`import =` fixtures error with a fix message; no downlevel JS emitted | reality-checks/strip-fidelity-<N>.md |
| `check-parity` | I-4 | Diff `meow check` diagnostics against the reference compiler (`tsc`/`tsgo`) over a corpus → match; confirm the in-process linter is labelled "fast preview" | reality-checks/check-parity-<N>.md |
| `resolver-parity` | I-5 | `meow install` writes no `node_modules`; resolve a specifier corpus through the runtime and the LSP → identical results | reality-checks/resolver-parity-<N>.md |
| `determinism` | I-6 | Run the same project on two machines + with a scrambled host env/clock/randomness → identical observable output; ungranted host state is invisible | reality-checks/determinism-<N>.md |
| `lockfile-integrity` | I-7 | Tamper a cached package's bytes → execution refuses (hash mismatch); `meow.lock.jsonl` is strictly sorted with no duplicate lines | reality-checks/lockfile-integrity-<N>.md |
| `capability` | I-8 | A dependency without a grant cannot reach the resource at the current tier; the threat-model doc states exactly what the tier does and does not stop | reality-checks/capability-<N>.md |
| `types-fresh` | I-9 | Regenerate `meow:*` types from the implementation → diff against the committed `.d.ts` must be empty; no `@types/meow` dependency exists | reality-checks/types-fresh-<N>.md |
| `footprint` | I-10 | Measure the release binary (≤ 60 MB target); assert no `rustc`/`cargo`/clang bundled and unmodified upstream `deno_core`/`rusty_v8` (non-fork) | reality-checks/footprint-<N>.md |
| `honesty` | I-11 | Audit every product-facing claim against CANON §24: each perf claim names a workload + ships a repro benchmark; no "10×" on steady-state JS or `meow check`; security wording matches the enforced tier | reality-checks/honesty-<N>.md |

---

## Cadence

| Gate | When it runs | Failure → |
|------|--------------|-----------|
| `product` | **before implementation** — every user-facing/product-shaping change, at the plan | `block` concern → reshape the approach before building |
| floor (Tier 1) | every sub-agent, pre-merge | block merge; agent fixes in its worktree |
| `craft` | **every change, pre-merge** | `block` findings → author fixes, re-review; merge held until green |
| `reality` | per merged wave that touches behavior; before any phase exit | `red` + `drift_flag`; phase cannot exit |
| `intent` | per merged wave; before any phase exit | `drift_flag`; reconcile Plan via DR |
| project gates | per their invariant's blast radius | per the gate's own rule |

A phase's exit-gate (PLAN.md) is satisfied only when its required ceiling gates are `green` in
`state.json.gates`. `yellow` = known gap, tracked by a `drift_flag`; `unknown` = not yet run (treat as
not-passing for any exit decision).
