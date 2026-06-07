# Craft — meow

> The bar that makes "green" mean **genuinely good** — including the code no human will ever read.
> At full autonomy the gates *are* the quality: lint, tests, typecheck, reality and intent are all
> necessary but say nothing about whether the code is *well-built*. The **craft gate** is the one that does.
> It is **judgment, not a checklist** (CONSTITUTION A10).

---

## The bar (the only rule that matters)

Every change must be the decision a **principled senior engineer who owns this codebase for the long term**
would make. Not "does it work" (that's the reality gate), not "is it what we asked" (that's the intent gate)
— **"is this the right way to build it?"** If a thoughtful owner reading the diff would say *"this passes, but
I'd build it differently,"* it does not pass.

## Why disposition, not rules

We deliberately **do not enforce specific rules** ("use reusable components", "functions under N lines",
"no nested ifs"). Rules are gameable and breed their own pathologies — an agent told to extract components
will over-abstract exactly as wrongly as it under-abstracts. We enforce the **judgment that produces good
structure**. Get the decision right and reusable components, lookup tables, clean seams appear *by default*.

The catalog below is a list of **symptoms of mis-judgment to reason from — never a checklist to satisfy.**
An agent that "passes craft" by ticking items has missed the point as badly as the 102-case `if` it was
trying to avoid.

## The reviewer's lens (questions, not checks)

The `craft-auditor` reads the diff in its real context and asks:

- Is this the **simplest design that is correct and won't fight the next change**? (not the cleverest, not the most abstract)
- Would a senior owner **accept this in review, or send it back**?
- Is the **shape of the code the shape of the problem**? — data modeled as data, control flow matching real branching, the right abstraction at the right level.
- Did the agent reach for the **right tool**, or pattern-match a default? (a table vs the 102-`if`; a shared seam that *earns itself* vs copy-paste; the idiom/stdlib vs a hand-roll)
- Will this be **obvious to the next reader**, or does it hide intent behind structure that needs a comment to be legible?
- Does it **fit the existing patterns** of this codebase, or invent a divergent one for no reason?
- Is the **abstraction earned** — used enough, hiding the right thing, leaking nothing — or premature/leaky?
- Is there anything **speculative, dead, or premature** here?
- Is **error / edge handling deliberate**, or incidental?
- If the **most likely next requirement** landed tomorrow, does this design help or fight it?

A "no" to any of these is a finding — with severity and, crucially, **the better decision**, not just the complaint.

## Symptoms (reason from these; do not tick them)

Non-exhaustive signals that a wrong decision was probably made:

- A giant branch ladder / switch where **data or polymorphism** belonged (the 102-`if`).
- The same logic **copy-pasted** where a shared seam was right — **or** an abstraction extracted *before it earned itself*.
- A function/module doing several unrelated things (low cohesion); the wrong thing made public; a leaky interface.
- **Hand-rolled** code for a solved problem (parsing, date math, retries) instead of the idiom/stdlib.
- Names that hide intent; structure legible only with a comment where a better structure wouldn't need one.
- A **data structure fighting its access pattern** (a list where lookups want a map, etc.).
- Cleverness or indirection that **buys nothing**.
- **Speculative generality** — config, hooks, params for needs that don't exist yet.
- Divergence from the codebase's established patterns **without a reason**.
- Tests that assert the **implementation** rather than the **behavior** (green but meaningless).

## How the gate operates

- Runs on **every change, before merge**. Agent-to-agent — it costs tokens, not your attention. Holding the
  bar *whether or not you ever look* is the entire point of running a fleet autonomously.
- Output per finding: `{ severity, where, why-it's-the-wrong-decision, the-better-decision }`. Any
  `block`-severity finding stops the merge.
- The author agent fixes; the craft-auditor **re-reviews**. Loop until a principled reviewer *would approve*
  — not until findings hit zero by attrition, but until the bar is genuinely met.
- **High-blast-radius changes get a panel** (diverse lenses — clarity · design-for-change · idiom) instead of
  a single reviewer; a majority "would reject" blocks.
- Genuinely subjective ties resolve toward the **simpler design** and the **existing pattern**. A real fork
  with no right answer becomes an `assumption` record — shipped, surfaced for your correction, never a stall.

## Trusting the gate (who reviews the reviewer)

The thesis of 0xos is that this gate — adversarial, on everything — **substitutes for a human reviewing every
change**, so agentic development can reach the quality of constant human review. That claim is only as strong as
the gate's bar, so the gate is kept honest rather than trusted blindly:

- **Independence.** The craft-auditor is never the author's agent/context; a strong reasoning model; a panel of
  diverse lenses on high-blast-radius work. No self-review, no rubber-stamp.
- **Calibration spot-audits.** Periodically, and on a random sample of merged changes, an independent re-review
  (a fresh auditor, or you) checks whether the gate's verdicts match what a principled human reviewer would say.
  Divergence is the signal — sharpen Part B or the auditor; never paper over it.
- **Evidence trail = the proof.** Every verdict + findings + fixes is logged (`reality-checks/craft-<N>.md`), so
  "quality held" is an auditable record, not a vibe. This trail is how the thesis is *demonstrated*.
- **The human's role is to calibrate the gate, not to review changes.** Sample, confirm the bar is real, sharpen
  the standard. That scales; reviewing every diff does not.

## Builder's responsibility

Builder roles (implementer, shipper) build to this bar **the first time** — the craft gate is the backstop,
not the design phase. Read this file before non-trivial work: the reject-loop is expensive; getting the
decision right up front is cheap.

---

## Part B — meow craft specifics

> The universal bar above is the law; this is the meow-specific judgment a reviewer holds the line on. Keep it as judgment, not a rulebook. The canonical design source is `harness/CANON.md` — its discipline (commitments separated from aspirations; *"an ADR without its consequences is marketing"*; the §24 honest-constraints register) **is** the craft bar for this codebase. Build to it.

### Prose must match code — and claims must match enforcement (the meow cardinal sin)
This product's entire premise is that what `meow` pins and promises equals what it does (CONSTITUTION I-6, I-11). That makes a comment, doc, help string, or CLI output that overstates a guarantee the **worst** class of defect here — worse than a bug, because it sells trust the code does not back:
- A security string that says "blocked"/"sandboxed"/"mathematically blocked" at a tier that only rewrites static access is a **block** (I-8). Say "defense-in-depth," name what it does and does not stop, and point at `CANON §24.3`.
- A performance claim ("10× faster", "faster than Node") with no named workload + reproducible benchmark is a **block** (I-11). "10×" never attaches to steady-state JS compute or `meow check` — V8 *is* Node's engine.
- Dead code that implies a guarantee is a lie with a function signature: a persisted-but-never-read field, a capability check with no enforcement behind it. Wire it or delete it; correct the prose. Honest boundaries belong in `CANON §24`, not papered over.

### One graph, one resolver — re-derivation is a defect, not an optimization
A second parser, a second resolver, a tool that re-`stat()`s the filesystem instead of reading the module graph — these are blocks (CONSTITUTION I-1, I-5), because the product *is* the shared model. The runtime and the LSP MUST resolve through the identical code path; if they can disagree, that is the bug the `resolver-parity` gate exists to catch. Reach for the shared pipeline (`GRAPH`) and the shared resolver (`LOAD`) before writing anything that parses or resolves.

### Determinism & hermeticity are structural, not best-effort
Ambient host reads are the enemy of I-6. A `std::env::var`, `SystemTime::now()`, `Instant::now()`, or `rand` call buried in a subsystem crate is a craft defect even when it "works" — it routes authority around the capability layer and makes runs non-reproducible. All host access flows through the sanctioned host/hermetic seam where it is capability-gated and fakeable (the test runner depends on this: deterministic clock, fake network, seeded randomness). `principles-check.sh` greps for the stray call; the `determinism` gate is the adversarial proof.

### Rust idioms — the shape of the code is the shape of the problem
- **Model with types, not strings.** Content hashes, capability grants, module specifiers, and spec/prefix ids are newtypes, not raw `String`/`PathBuf` — the type system is the cheapest invariant enforcer.
- **The runtime does not panic on user input.** `unwrap()`/`expect()`/`panic!` on a path reachable from user code or a dependency is a block; return a diagnostic that points at the fix (see *Diagnostics* below). `unwrap` is acceptable only for genuine invariants that cannot fail.
- **Errors are typed and causal** (`thiserror`/`Result`), never stringly-typed swallow-and-continue.
- **`unsafe` only at the V8/FFI boundary**, in the narrowest possible seam, with the invariant it upholds written above it. Gratuitous `unsafe` elsewhere is a block.
- **Don't allocate gratuitously.** The performance story is startup + I/O + toolchain (CONSTITUTION I-10, CANON §24.1); borrow and reuse buffers / the parsed representation over cloning. A clone on the parse hot path needs a reason.
- **Reuse the ecosystem we bet on.** Oxc (parse/semantic/transform), Rolldown (bundle), `deno_core`/`rusty_v8` (V8). Hand-rolling what these provide is the "hand-roll of a solved problem" symptom — and forking V8 is an invariant violation (I-10).

### Diagnostics are a product surface — every error points at the fix
`meow` errors are held to the product's diagnostic bar (PRODUCT.md): name the cause **and** the remedy, like *"Enums emit runtime code and cannot be type-stripped. Use a `const` object instead."* A bare `Err`, an unwrapped `?` that bubbles a low-level message, or "something went wrong" over a real failure (a failed install, a stale shadow config) is a craft defect — surface the real reason and the next step.

### Build to the quality of
Until the first chokepoint crates exist, build to the discipline of `harness/CANON.md` itself: every decision carries its consequences, every claim its scope, every limit written down rather than implied. Once they land, the exemplars are the **single-parse pipeline crate (`GRAPH`)** and the **single resolver crate (`LOAD`)** — one chokepoint, the boundary enforced exactly as documented, the comment telling the real (not the aspirational) truth.