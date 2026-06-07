# Principles — meow

> Standing decisions, each with a **mechanical enforcement**. A principle with no enforcement is a wish —
> if it can't be checked by a hook, a gate, or a script, it doesn't belong here. These are the durable
> "how we work" rules; the *product* law lives in `CONSTITUTION.md` Part B.
>
> The generic set (P1–P11) ships with every 0xos harness and maps 1:1 to the CONSTITUTION Part A baseline.
> Project-specific principles go in the marked section below.

---

## Generic principles (the 0xos baseline)

### P1 · The spine is traceable
Every spec carries a non-empty `plan_ref` and `constitution_ref`; every line of work traces to a spec, every
spec to the Plan, every Plan item to a constitutional promise. Untraceable work is scope creep and is cut.
**Enforced by:** `harness/principles-check.sh` (grep gate, Tier-1 floor) + the `intent` gate. (CONSTITUTION A2)

### P2 · State is data, never prose
`state.json` is pure structured data, valid against `schemas/state.schema.json` on every write. Narrative
goes to the CHANGELOG (one line per merge) or a referenced evidence file — never inside the JSON.
**Enforced by:** the Stop hook (`hooks/turn-check.sh`) validating against the schema each turn. (CONSTITUTION A3)

### P3 · Reality is the gate
"Tests pass" is not "works". Done = the adversarial ceiling in GATES.md: auditors that hit real services
and re-derive reality, never trusting the local floor. Verdicts land in `state.json.gates`.
**Enforced by:** GATES.md Tier-2 (`reality` + `intent`); phase exits blocked on `green`. (CONSTITUTION A4)

### P4 · Disk is memory
The agent's context is a cache; the disk is truth. Everything to resume cold lives in `state.json` +
`CHANGELOG.md` + `decisions.json`. A fresh session resumes from those alone.
**Enforced by:** the SessionStart hook (`hooks/session-start.sh`) injecting the brief each cold start. (CONSTITUTION A1)

### P5 · Comment-marker concurrency
Shared-file edits across concurrent specs are fenced with `// === <SPEC-ID> ===` … `// === /<SPEC-ID> ===`.
The orchestrator (sole integrator) resolves conflicts along these fences; unbalanced fences are a bug.
**Enforced by:** `principles-check.sh` (fence balance check) + the orchestrator's merge step. (ORCHESTRATOR.md)

### P6 · ctx7 docs-first
Before touching any library, framework, SDK, CLI, or cloud service, read its **current** docs via `ctx7` —
your training data may be stale. No coding against a remembered API surface.
**Enforced by:** `principles-check.sh` flags new/changed third-party imports whose spec lacks a docs-checked
note; the implementer role contract requires a ctx7 fetch before non-trivial library work.

### P7 · No dev gates — bounds are mechanical
While `mode: incubating`, nothing waits on human approval. The only hard stops protect the *fleet*, not the
feature: **spend cap**, **scope jail**, **public-leak guard** — enforced in hooks, never a review prompt.
Agents act; the human corrects after via the Corrections inbox.
**Enforced by:** the PreToolUse hook (`hooks/guard-irreversible.sh`) + the spend/scope/leak bounds; flips on
graduation (A8). (CONSTITUTION A5, A6)

### P8 · Reversibility & the public-leak boundary
Secrets live freely on disk and in **private** git by design — they're the #1 friction source and agents
handle them well. The only secret stop is pushing one to a **public** remote. A human sees an action before
it happens only if it is BOTH un-undoable AND its damage escapes this project.
**Enforced by:** the public-leak guard (pre-push) + scope jail; both in hooks. (CONSTITUTION A6, A7)

### P9 · Craft is gated, not assumed
"Green" means well-built, not merely passing. An adversarial craft gate holds a senior-owner's bar — "is this
the right way to build it?" — on every change. It is **judgment, not a rule checklist** (we don't enforce
"use reusable components"; we enforce the decision-making that makes them appear by default). The point: quality
holds whether or not a human ever reads the code.
**Enforced by:** GATES.md `craft` gate — the `craft-auditor` blocks merge until the bar is met, against `CRAFT.md`. (CONSTITUTION A10)

### P10 · The harness is the absent product owner (before the build)
Everything built is a product. Every user-facing/product-shaping decision is weighed from the customer/UX/
cohesion angle **before** it's built — the agent raises the product concerns a human owner would, proactively,
and bakes the answers into the approach. Judgment, not a rulebook; the one gate that runs at the plan, not the diff.
**Enforced by:** GATES.md `product` gate — the `product-steward` reshapes the approach pre-implementation, against `PRODUCT.md`. (CONSTITUTION A11)

### P11 · Principles serve the product, not the reverse
The constitution/principles are means to a great product, honored through **delivery** — not displayed on
surfaces, and never elevated above the core experience. Don't over-index: a principle (or its literal expression)
harming the product is misapplied — the product comes first; the value, not its expression.
**Enforced by:** GATES.md `product` gate / the `product-steward`, which flags principle-over-indexing as a bad product decision. (CONSTITUTION A12)

---

## Project-specific principles

> Numbered `P12…`, each with a mechanical enforcement. These are the cheap, pre-merge **floor** tripwires that back the CONSTITUTION Part B invariants; the adversarial proof of each invariant is its Tier-2 gate (GATES.md).

### P12 · First-party is ESM-only
First-party code is ESM; no authored `.cjs`, no first-party `require()`. CommonJS is a read-only dependency-compatibility surface (CONSTITUTION I-2, ADR-3).
**Enforced by:** `principles-check.sh` (no first-party `.cjs`) + the `no-first-party-cjs` lint rule (floor) + the `compat` gate.

### P13 · Erasable TypeScript only
First-party TS uses erasable syntax only — no `enum`, runtime `namespace`, parameter properties, or `import =` (CONSTITUTION I-3).
**Enforced by:** `principles-check.sh` (bans `enum`/`namespace`/`import =` in first-party `.ts`) + the `strip-fidelity` gate (the property test covers parameter properties).

### P14 · One config, generated shadows
Humans edit only `meow.config.ts`. `.meow/`, the generated `tsconfig.json` content, and the root `package.json` are derived artifacts — gitignored where applicable, never hand-authoritative (ADR-8).
**Enforced by:** `principles-check.sh` (`.meow/` gitignored; generated files carry a "GENERATED — do not edit" header; no stray per-tool configs committed) + `meow doctor` staleness/hand-edit check.

### P15 · One parser, one resolver
There is a single parse pipeline and a single resolver; no subsystem constructs its own (CONSTITUTION I-1, I-5).
**Enforced by:** `principles-check.sh` (parser/resolver constructors confined to their crates) + the `graph-integrity` and `resolver-parity` gates.

### P16 · No ambient host reads
Subsystem crates do not read the host environment, clock, or randomness directly; all such access goes through the capability/hermeticity seam (CONSTITUTION I-6).
**Enforced by:** `principles-check.sh` (bans `std::env::var`/`SystemTime::now`/`Instant::now`/`rand` outside the sanctioned host crate) + the `determinism` gate.

### P17 · Generated runtime types; honest claims
`meow:*` types are generated from the implementation (no `@types/meow`), and no product-facing surface advertises a guarantee `meow` does not enforce (CONSTITUTION I-9, I-11).
**Enforced by:** `principles-check.sh` (no `@types/meow`; bans over-claim phrasings like "mathematically blocked"/"100% secure" in product surfaces) + the `types-fresh` and `honesty` gates.