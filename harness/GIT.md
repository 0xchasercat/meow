# Git — meow

> Worktree-per-spec, atomic sub-agent commits, **one integrator**. `main` is always green and deployable.
> This is the mechanical backbone behind ORCHESTRATOR.md → "Commit & integration".
>
> **Repo shape:** `monorepo` (single Cargo workspace; subsystems are crates — see PLAN.md prefix map). The monorepo column below applies.

---

## Branches

| Branch | For | Created by |
|--------|-----|-----------|
| `main` | the only integration branch; always green & deployable | — |
| `spec/<ID>` | one spec's work (e.g. `spec/W-014`) | orchestrator, per dispatch |
| `h/<slug>` | harness chores: hooks, CI, docs, tooling (e.g. `h/add-perf-gate`) | orchestrator |

No long-lived feature branches, no shared dev branch. A branch lives only as long as its spec/chore.

---

## Worktrees

Each dispatched sub-agent gets its **own worktree** so waves run truly concurrently without stepping on
each other's working tree:

```
.claude/worktrees/<ID>/        # worktree for spec/<ID>, checked out on that branch
```

The orchestrator creates the worktree at dispatch and removes it after the branch is merged or abandoned.
Shared-file edits inside a worktree are fenced with comment markers `// === <ID> ===` / `// === /<ID> ===`
(PRINCIPLES.md P5) so the integrator can resolve overlaps along known boundaries.

---

## Who commits — the matrix

| Actor | Commits | Branches | Merges to `main` | Pushes |
|-------|---------|----------|------------------|--------|
| **sub-agent** | yes — atomic commits on its own branch | works only on its `spec/<ID>` or `h/<slug>` | **never** | to its own branch only |
| **orchestrator** | merge commits / integration fixes | creates `spec/*`, `h/*` | **yes — the sole integrator, only when gates green** | pushes `main` and branches |
| **human** | rarely (corrects via the Corrections inbox, not commits) | — | no | no |

The **orchestrator owns integration**. Sub-agents never touch `main`. Merge happens only after the Tier-1
floor is green and (per cadence) the Tier-2 ceiling is green (GATES.md). Each merge auto-appends one
CHANGELOG line and updates `state.json` (A3).

---

## Monorepo vs polyrepo

- **Monorepo** — one repo, subsystems are directories (prefix → path, see PLAN.md map). `spec/<ID>`
  branches and worktrees live in the single repo. Comment-marker fences carry the concurrency load.
- **Polyrepo** — one repo per subsystem (or service). The orchestrator runs a worktree **per repo
  touched** by a spec; a spec that crosses repos opens a `spec/<ID>` branch in each and the integrator
  merges them as a coordinated set. `state.json.subsystems` still keys on prefix regardless of repo
  layout. Cross-repo contract changes get a `depends_on` ordering so the producer merges before the consumer.

---

## Secrets policy (explicit)

Secrets are **free on disk** and **free in private git** by design (CONSTITUTION A6) — they are the #1
friction source and agents handle them better than a human round-trip. Commit `.env`, keys, and config to a
**private** remote without ceremony.

The **only** secret stop is the **public-leak guard**: pushing a secret to a **public** remote fails the
push (a BOUNDED event → `decisions.json`). There is no "review your diff for secrets" gate on private work.
Before making any repo public, the orchestrator runs a full-history secret scan and rotates anything exposed.

---

## Invariants

- `main` is always green and deployable — a red merge is a bug to revert, not to leave.
- Every commit on a `spec/*` branch is atomic and traces to its spec (the spec traces the spine).
- The integrator is the only writer of `main`; concurrency is safe because worktrees + fences make
  overlaps explicit and resolvable.
