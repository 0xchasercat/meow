# Product — meow

> The bar that makes the harness the **absent product owner** — so the agent weighs a decision's product
> implications *before* it builds, instead of waiting for a human to catch the incoherence after.
> Where CRAFT.md asks **"is it well-built?"** (code, after), this asks **"is it the right product decision?"**
> (customer · UX · cohesion · **does it actually deliver the promise**, *before*) — for **UX decisions AND for
> technical details whose consequences shape the product**, even when they're invisible in the UI. It is
> **judgment, not a checklist** (CONSTITUTION A11).

---

## Why this gate runs *before* the build

Normally a human has to prompt the agent — *"wait, would a customer find that intuitive? does that fit the rest
of the product? does that name even make sense?"* — and usually only **after** something's been built wrong.
That's the gap this closes. **Everything we build is ultimately a product** (even an internal tool — it has a
user, a mental model, a first impression). So every product-shaping decision must account for how it lands as a
product, and the harness enforces that the agent raises those questions **itself, up front**, and bakes the
answers into the approach. Catching it after is rework and incoherence; catching it before is just good product
sense, made non-optional.

The motivating failure this exists to prevent: a "Fleet" view that showed per-project data under a cross-project
title with no project label — confusing, incoherent with the per-project surfaces, built before anyone asked
"what would a user expect 'Fleet' to mean?" A product-steward review of the *approach* would have killed it
before a line was written.

## The bar

Every product-shaping decision — anything a user sees / names / navigates, **and any technical choice whose
consequences shape the product** (its quality, coverage, completeness, fitness against the promise) — must be
the decision a **thoughtful product owner who understands both the customer and the product's promise** would
make. If a careful owner looking at the *plan* would say *"a user won't get this,"* *"this doesn't fit the rest
of the product,"* or *"that won't actually deliver what we promise,"* the approach does not pass — caught now,
at the plan, not after the build.

The product's **promise** is the constitution (its articles + invariants). The steward holds every decision —
UX and technical — against it. *"Our promise is the best-quality residential proxies; does this filter actually
deliver that, or leak?"* is as much a product question as *"would a customer understand this screen?"*

**It's all one bucket.** A misleading "Fleet" view, a 26-ASN filter that breaks the quality promise, and a casino
that shoves "empathy" onto every surface while keeping an unplayable game stage — all three shipped for the
*same* reason: **a decision made without taking the product's seat.** The steward always takes that seat. The
question is always the same: *would a thoughtful product owner, looking at this from the product's perspective,
make this decision?*

## Not only UX — technical details are product decisions too

The most dangerous product failures are **invisible**: a technical detail that passes tests, reads as clean
code, and ships — yet silently breaks the product's promise. These don't *look* like "product" decisions at
first glance, so no one questions them from the product angle. The steward must.

**The canonical example (real):** a proxy gateway whose promise is *"only the best-quality residential proxies
reach customers"* filtered datacenter/CDN traffic against a **hardcoded list of ~26 ASNs**. It compiled, passed,
read fine — but thousands of *other* datacenter ASN ranges leaked straight through to customers, gutting the core
quality promise. Nobody flagged it: the craft gate saw clean code; tests saw those 26 filtered correctly. Only
the *product* question exposes it — **"would the product's quality be acceptable filtering only those ASNs? what
about every datacenter range that leaks?"** A toy/sample standing in for the real dataset is a product defect.

So judge **thresholds, allow/deny lists, coverage, datasets, heuristics, defaults, limits, fallbacks** by *"does
this actually deliver the promise at real-world scale — or just appear to?"* A thing that works on the happy
path / the sample / the test fixture but fails the promise in the wild does not pass.

## Principles serve the product — don't over-index (the inverse failure)

The steward catches the *opposite* mistake too: not under-considering the product, but **over-indexing on the
constitution/principles until they harm it.** Principles are *how* we build a great product; they're honored
through the **quality and delivery** of the experience — never by plastering their names on surfaces, bolting on
features that announce them, or letting adherence degrade the core experience.

**The canonical example (real):** chi is a casino whose rewards run on *empathy*. The right expression is empathy
baked into **how the rewards work and feel** — not the word "empathy" on every surface, which cluttered the UI
and overpowered the core casino experience. Worse, an agent refused to remove a UI element that made the game
stage unplayable on mobile because it "violated a principle" — inverting the hierarchy: **a playable casino
honors the principle better than an unplayable, cluttered one.** Had the agent taken the product's seat — *"are
we still building a great casino?"* — none of it would have shipped. Same bucket as Fleet and the ASN filter.

Hold the line: the **value** (honored through delivery) is distinct from any particular **expression** of it (a
label, a feature, a UI element — not sacred; it yields to the product). When a principle seems to fight the core
experience it's almost always over-literalization — find the expression that serves both; if truly forced, the
**core product experience wins.** (Genuine integrity invariants — provably-fair, no-hidden-tables — are
themselves *for* the product; honoring them isn't over-indexing. Decorating the UI with principle-words is.)
Flag any decision that puts the principles we build *by* above the product *itself*.

## Why disposition, not rules

We don't enforce product rules ("every screen needs a header", "max 3 tabs"). Rules miss the point and break in
the cases that matter. We enforce the **judgment** that makes coherent, intuitive product appear by default.
Get the decision right — by genuinely taking the customer's seat — and intuitive UX, consistent IA, and a
cohesive whole follow. The symptoms below are signals to reason from, never a checklist to tick.

## The steward's lens (questions, asked *at the plan*)

The `product-steward` (and the builder, proactively) takes the customer's seat and asks, before building:

- **Customer's seat:** who uses this, and what are they actually trying to do? Does this serve that, or the
  implementer's convenience?
- **First-encounter:** would someone seeing this for the first time understand it *without explanation*? Does it
  match what they'd expect from the name/placement?
- **Mental model:** does it fit the model the product has already taught the user, or fight it? (the Fleet test:
  does "X" actually mean what the user will assume "X" means here?)
- **Cohesion across the board:** does it fit the rest of the product — naming, lexicon, IA, layout patterns,
  interaction grammar? Or does it invent a divergent one? Is it consistent with sibling surfaces?
- **Redundancy / conflict:** does it overlap, duplicate, or contradict an existing surface? (two places that do
  the same thing, differently, is a product smell.)
- **Simpler shape:** is there a clearer product framing that needs less explaining? Is each piece of complexity
  *earned* by user value?
- **Fitness for the promise (incl. technical details):** does this *actually deliver* what the product promises
  at real-world scale/coverage — or just appear to? (a filter that filters too little; a sample list standing in
  for the real dataset; a lenient threshold; a default wrong for most users.) Hold technical choices to the
  promise, not merely to "it runs / it passes."
- **What would the owner push back on?** Name it now, and resolve it in the approach.

A "no" is a concern — raised **before** the build, with the **better product decision**, not just the worry.

## Symptoms (reason from these; do not tick them)

- A surface whose **name promises something its content isn't** (a cross-project "Fleet" showing one project's
  unlabeled data).
- **Two surfaces doing the same job** differently (per-project corrections in a workspace tab *and* a
  Mission-Control inbox) — redundancy that confuses.
- IA / naming / lexicon **inconsistent across surfaces** (the same concept named two ways; divergent patterns).
- A feature that **needs a manual** to use — non-obvious to a first-time user.
- Complexity, options, or screens **not earned** by a real user need.
- A decision made **without taking the customer's seat** — optimized for what's easy to build, not what's right
  to use.
- A flow that's locally fine but **breaks the product's coherence** when seen next to everything else.
- A **technical detail that quietly breaks the promise**: a narrow hardcoded allowlist / sample dataset / lenient
  threshold / happy-path-only coverage standing in for the real thing (the 26-ASN "quality" filter that leaks
  thousands of datacenter ranges) — passing tests while failing the product.

## How the gate operates

- Runs at the **planning / approach stage — BEFORE implementation** (and again on any approach change), distinct
  from the post-build craft/reality/intent gates. The unit it reviews is the *approach/plan*, not the diff.
- For user-facing or product-shaping work it is **required**; for purely-internal mechanical changes it's a
  quick pass (often a no-op). Scale the scrutiny to the product blast radius.
- Output: product concerns `{ severity, where, why-it-misleads-or-fragments-the-product, the-better-decision }`.
  A `block` concern **reshapes the approach before any build**; the builder revises and the steward re-checks.
- Resolve genuine forks (real product choices with no clear answer) by **asking the operator** as a concise
  decision — the few questions a product owner truly must answer — rather than guessing or stalling. Everything
  else the steward decides toward the customer + cohesion.
- High-impact / cross-surface changes get a **panel** (customer-seat · cohesion · simplicity lenses).

## Builder's responsibility (the proactive part — the whole point)

Before you decide *how* to build anything user-facing, **take the customer's seat first** and answer the
steward's lens yourself — surface the product concerns the human would have raised, in your approach, up front.
Don't wait to be asked. The product-steward is the backstop; proactive product judgment is the job.

---

## Part B — meow product specifics

### Who the customer is
JavaScript/TypeScript developers and teams building **modern services, edge workers, and libraries**, who are fluent with the existing toolchain and tired of assembling and reconciling a dozen tools (runtime, package manager, `tsc`, ESLint, Prettier, Jest, bundler, workspace orchestrator) that each re-parse the code and disagree at the seams. They are mid-to-senior: they value **startup/toolchain speed, determinism, no `node_modules` pain, no config sprawl, and tooling that tells the truth.** The **editor user is a first-class customer** — the LSP experience is how the zero-`node_modules` model becomes pleasant instead of mysterious. A secondary customer is the **security-/supply-chain-conscious team** that wants capabilities and provenance without friction. What they are actually doing: *run / install / check / test / bundle a TS project and get identical, explainable results everywhere* — not "adopt a philosophy."

### The product's mental model & lexicon
The product teaches **one project model** and a fixed vocabulary. Use these exact words; do not invent synonyms:
- **The graph / the pipeline** — the single canonical model. Stages: **Lossless Syntax Tree (CST) → Semantic Graph → (delegated) type info → Module Graph → Runtime IR** (CANON §7).
- **Modes:** `strict-web` (default) · `node-compat` · `legacy`. Modes widen what *dependencies* may do; they never permit first-party CJS.
- **Capabilities / grants / Trust Zones** — authority is granted, scoped, and enforced in tiers; a "Trust Zone" is the tier-2 per-package boundary (ADR-6).
- **Install modes:** `pnp` (default, no `node_modules`) · `vfs` · `materialize` · `vendor`. The **content-addressed cache** is the global store; **`meow.lock.jsonl`** is the lockfile.
- **Shadow configs** — `meow.config.ts` (`defineMeow`) is the only human-edited config; `.meow/tsconfig.json` + the root `package.json`/`tsconfig.json` shim are **generated** (ADR-8).
- **`meow:*` APIs** — runtime capabilities are imported behind the typed `meow:` namespace, never bolted onto globals.
- **Tasks** — `meow.tasks.ts` (`defineTasks`) with declared `inputs`/`outputs`.
- **Type stripping ≠ typechecking** — stripping runs your code (instant, native); `meow check` is delegated diagnostics. The in-process linter is a **"fast preview."**
- **Observability verbs:** `why-slow` · `why-large` · `why-dep` · `trace` · `profile` · `doctor`.
- **CLI grammar:** `meow <verb> [args] [--flags]` — short, predictable verbs; every command reads the one config, understands the workspace graph, and respects the same lockfile + capabilities.

### Cohesion anchors
Every surface should feel like one system, not a suite that shares a prefix:
- **One config, one lockfile, one graph** — a command that introduces its own config file, its own resolver, or its own parse has broken the product even if it passes tests (CONSTITUTION I-1).
- **Diagnostics explain causes and point at the fix.** The exemplar is the type-strip error — *"Enums emit runtime code and cannot be type-stripped. Use a `const` object instead."* Every error names the cause and the remedy; "something went wrong" is a defect.
- **Honest output.** Help text, `--help`, and docs state scope on every claim; `meow doctor` and the observability surfaces are the model — specific, actionable, true (CONSTITUTION I-11).
- **The `meow:` namespace is the only runtime-API shape**, and its types are always present in-editor without installing anything.

### Don't over-index on this
`meow` is a **runtime + toolchain**, not a security product or a benchmark trophy. Hold the line *for* the product, not against it:
- Honesty (I-11) is honored by **accurate, scoped claims**, never by stamping "secure" or "10×" on surfaces. Decorating the CLI/docs with principle-words is the over-index failure (A12).
- Capabilities must not make normal development painful (CANON §24.11): **good defaults and suggested grants beat maximal enforcement.** A correct sandbox no one can use has failed the product.
- Do not gold-plate Node-compat (CANON §24.9): first-party stays ESM; `legacy` is explicit and diagnostic-heavy, not the polish target.
- Do not build every tool surface before the shared graph works (CANON §24.10): the value is the shared model; breadth that re-derives the graph is negative work.