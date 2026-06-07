# Constitution — meow

> Operational law, not marketing. This file is the top of the spine:
> `CONSTITUTION → PLAN → spec → commit → gate → surface`. Nothing should exist that can't trace back here.
>
> It has **two parts**. Part A is the **0xos baseline** — shared across every project, rarely touched.
> Part B is **this project's specifics** — you fill it in. Amending Part A or a numbered invariant is the
> one genuinely RED act in an otherwise gate-free dev model (see ORCHESTRATOR.md → Autonomy).

---

## Part A — The 0xos baseline (shared)

These hold for every project built under 0xos. They are about how the *harness* works, not what the
*product* is.

### A1 · Disk is memory
The agent's context is a cache; the disk is the source of truth. Everything needed to resume cold lives in
`harness/state.json` + `harness/CHANGELOG.md` + `harness/decisions.json`. A fresh session must be able to
pick up the work from those files alone. (Enforced by the SessionStart hook.)

### A2 · The spine is traceable
Every spec carries a non-empty `plan_ref` and `constitution_ref`. Every line of work traces to a spec, every
spec to the plan, every plan item to a constitutional promise. Untraceable work is scope creep and is removed.
(Enforced by `principles-check.sh` + the intent-gate.)

### A3 · State is data, never prose
`state.json` is pure structured data validated against `schemas/state.schema.json` on every write. Narrative
goes in the CHANGELOG (one line per merge) or a referenced evidence file — never inside the JSON. (Enforced by
the Stop hook.)

### A4 · Reality is the gate
"Passing tests" is not "working". Definition-of-done is the adversarial gate in GATES.md — real services, real
data, an auditor that re-derives reality and never trusts the local floor.

### A5 · No human gates during development
While a project is `incubating` (zero/trial users, no real money — even on a production domain), nothing waits
on human approval. A control only earns its keep when there's a real stakeholder to protect; solo with no
users there is none, so a gate is pure friction. Agents **act**; the human reviews and corrects from output.
The only hard stops are **mechanical bounds** (A6). This inverts when the project `graduates` (A8).

### A6 · Bounds are mechanical, not human
The only things that hard-stop an agent protect the *fleet*, not the feature, and are enforced in hooks —
never a review prompt:
- **Spend cap** — an agent cannot exceed the project's money budget; raise it async.
- **Scope jail** — an agent cannot touch paths/infra outside this project (protects your other projects).
- **Public-leak guard** — secrets live freely on disk and in *private* git by design (they are the #1 source
  of friction and agents handle them better than a human); only pushing a secret to a **public** remote is blocked.

### A7 · Reversibility is the master variable
Every action is reversible-within-scope or it isn't. The friction test: a human sees something *before* it
happens only if it is BOTH un-undoable AND its damage escapes this project. That set is tiny; everything else
flows and is corrected after.

### A8 · Graduate explicitly
`mode: incubating` is the default. `mode: graduated` means real users + real money — the project is a real
product and leaves the harness (or flips to a guarded mode that re-enables conservative gates). A production
domain with 0 users is still incubating.

### A9 · HTML is the human surface, JSON/markdown is the machine surface
Agents read and write terse machine state. Humans read rich generated HTML (specs, plans, decisions, audits).
The machine loop never consumes the rendered HTML; human edits round-trip back to the source.

### A10 · Craft is gated, not assumed
"Green" must mean **genuinely well-built**, not merely passing. Because no human reviews the fleet's thousands
of micro-decisions, an adversarial **craft gate** holds a principled senior-engineer's bar on every change —
*"is this the right way to build it?"* — and blocks merge until met. It is **judgment, not a rulebook**: we do
not enforce rules like "use reusable components"; we enforce the decision-making that makes good structure
appear by default. Quality holds whether or not anyone looks. (Enforced by GATES.md craft gate + CRAFT.md + the craft-auditor.)

### A11 · The harness is the absent product owner — proactively, before the build
Everything built is ultimately a **product** (even an internal tool has a user, a mental model, a first
impression). So every product-shaping decision — UX **and** the technical details whose consequences shape the
product (its quality, coverage, fitness against the promise), even when invisible in the UI — must account for
how it lands as a product: the customer's perspective, an intuitive UX, coherence with the whole, and **whether
it actually delivers the product's promise**. The harness enforces that the agent weighs this **before it
builds**, raising the questions a human product owner would (*"would a customer get this? does it fit the rest
of the product? does this actually deliver what we promise, or just appear to?"*) **itself, up front**, instead
of waiting to be prompted after the fact. This is **judgment, not a rulebook**, and it runs at the **plan**, not
the diff — the one gate that must come before implementation. (Enforced by GATES.md product gate + PRODUCT.md + the product-steward.)

### A12 · Principles serve the product — not the reverse
The constitution and these principles describe **how** we build well; they are **means in service of a great
product — not the product itself, and not things to put on screen.** The product's core purpose comes first (a
casino must first be a great casino). Honor a principle through the **quality and delivery** of the experience —
never by plastering its name across the UI, bolting on features that announce it, or letting literal adherence
degrade the core experience. Distinguish the **value** (honored through how the product works) from any
particular **expression** of it (a label, a feature, a UI element — *not* sacred; it yields to the product). When
a principle seems to conflict with the core experience it's almost always over-literalization — find the
expression that serves both; if truly forced, **the core product experience wins.** Do not over-index: an agent
that elevates principle-adherence above the product has inverted the hierarchy. (Genuine integrity invariants —
provably-fair, no-hidden-tables, ledger-sums-to-zero — are themselves *for* the product; honoring them is not
over-indexing. Decorating the UI with principle-words is.) (Enforced by the product-steward, which catches BOTH
under-serving the product AND over-indexing on principles.)

---

## Part B — meow specifics

### Product promise
`meow` is an uncompromising JavaScript/TypeScript **runtime and unified toolchain in one Rust binary** — runtime, package manager, workspace + task runner, test runner, bundler, linter, formatter, typecheck orchestrator, security and observability — all reading **one canonical model of your project**: one parse, one graph, one lockfile, one config. It is Rust-on-V8, ESM-first, deterministic, and capability-scoped (full architecture in `harness/CANON.md`). The one thing it must never get wrong is **divergence between what it pins/promises and what it does**: the same `source + lockfile + meow version + capabilities` must always produce the same behavior; the editor and the runtime must resolve a module the same way; a capability boundary must be exactly as strong as advertised; and a performance/compat claim must be benchmark- or fixture-defensible or it is not made. `meow` may ship *less* than it wants (tier-2 security, delegated typechecking) — it must never advertise *more* than it enforces. For a runtime, trust is the product; honesty about its own boundaries is therefore load-bearing, not decoration (A12).

### Articles
- **One truth.** There is a single canonical project model. Every tool reads it; none re-derives it. Two answers to "what does this import resolve to?" is a defect, not a feature.
- **Standards first, Node by request.** The default surface is Web/WinterTC-aligned; Node compatibility is an explicit mode, never ambient (CANON §11).
- **First-party is modern.** First-party code is ESM and erasable TypeScript only; CommonJS exists solely as a read-only dependency-compatibility surface (ADR-3).
- **Determinism is the contract.** Execution is hermetic: the host environment, clock, and randomness cannot influence a run unless explicitly granted.
- **Least authority.** Dependencies are content-addressed, integrity-checked, and capability-scoped; nothing receives authority it was not granted.
- **Honest claims.** Every external claim — performance, security, compatibility — states its scope and is defensible; the runtime never sells a guarantee it does not enforce.

### Invariants
> The code-enforced, reality-verified projection of the articles. Each `I-N` is checked by the matching GATES.md gate (named in brackets); the cheap floor tripwire, where one exists, lives in `principles-check.sh`.

- **I-1 · One parse, one graph.** No subsystem instantiates its own parser or resolver; lint, format, check, bundle, the runtime, and the LSP all consume the shared pipeline (CANON §7). [`graph-integrity`]
- **I-2 · First-party is ESM-only; CJS is refused.** A first-party `.cjs` or first-party `require()` errors in every compatibility mode (ADR-3). [`compat`]
- **I-3 · Type-strip is meaning-preserving and erasable-only.** TS executes by erasing annotations to whitespace with identical source positions; `enum`, runtime `namespace`, parameter properties, and `import =` error with a fix-pointing message; stripping never emits downlevel JS (ADR-5, CANON §9). [`strip-fidelity`]
- **I-4 · No native typechecker.** `meow check` diagnostics equal the reference compiler's (`tsc`/`tsgo` daemon); the in-process type-aware linter is labelled a "fast preview" and is never authoritative (ADR-5). [`check-parity`]
- **I-5 · No `node_modules` by default; one resolver.** `meow install` writes no `node_modules`; the runtime and the LSP resolve modules through the identical resolver (ADR-4). [`resolver-parity`]
- **I-6 · Determinism & hermeticity.** Same `source + lockfile + meow version + capabilities` ⇒ same behavior; ungranted host env, clock, and randomness cannot influence execution (the core bet, CANON §1). [`determinism`]
- **I-7 · Lockfile integrity.** Every dependency is content-hash-pinned and integrity-checked before execution; `meow.lock.jsonl` is strictly sorted, one dependency per line (CANON §12.3). [`lockfile-integrity`]
- **I-8 · No ambient authority.** A dependency without a grant cannot reach the resource at the current enforcement tier; capability tiers 1–2 are documented as "defense-in-depth," never "mathematically blocked," until the tier-3 escalation ships and is adversarially tested (ADR-6, CANON §24.3). [`capability`]
- **I-9 · Runtime types are generated.** `meow:*` TypeScript declarations are generated from the runtime implementation; there is no `@types/meow` and no hand-maintained drift (CANON §8.2). [`types-fresh`]
- **I-10 · Small footprint; upstream V8.** No `rustc`/`cargo`/clang is embedded; V8 is consumed via unmodified upstream `deno_core`/`rusty_v8` (never forked); the released binary stays within budget (≤ 60 MB target) (ADR-1, ADR-7, CANON §26). [`footprint`]
- **I-11 · Honest claims.** No doc, CLI string, help text, or output advertises a guarantee `meow` does not deliver: a performance claim names its workload and ships a reproducible benchmark; "10×" never attaches to steady-state JS compute or `meow check`; a security claim matches the enforced tier (ADR-9, CANON §24.1). [`honesty`]