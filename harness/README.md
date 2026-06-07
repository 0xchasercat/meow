# meow — harness

The autonomous-build harness for **meow** (rust). This `harness/` directory is the
machine-and-human substrate the agent fleet runs on. You don't drive it from here — you drive it from
**0xos Mission Control** (the browser control plane). This README is the map.

## The spine

```
CONSTITUTION  →  PLAN  →  spec  →  commit  →  gate  →  surface
   (law)        (map)   (contract) (work)  (reality) (human view)
```

Nothing exists that can't trace back along this spine. Every spec carries a `plan_ref` and
`constitution_ref` (enforced by `principles-check.sh`). The orchestrator turns the Plan into gated,
reality-verified commits; Mission Control renders the result for the human, who corrects — never gates.

## What's here

| Path | Role |
|------|------|
| `CONSTITUTION.md` | operational law. Part A = 0xos baseline (shared), Part B = meow specifics. Top of the spine. |
| `PLAN.md` | the map: subsystem→prefix table, phases, build sequence. Drifts; the intent-gate flags drift. |
| `ORCHESTRATOR.md` | the tick: how the main session dispatches waves, integrates, and stays autonomous. |
| `GATES.md` | definition of done: Tier-1 floor + Tier-2 adversarial ceiling (`reality` + `intent`). |
| `PRINCIPLES.md` | standing decisions, each with a mechanical enforcement. |
| `GIT.md` | worktree-per-spec workflow, who-commits matrix, secrets policy. |
| `state.json` | pure-data resume anchor + Mission Control's primary feed. Validated every write. |
| `decisions.json` | the corrections feed: `assumption` / `bounded` / `needs-input` records. |
| `CHANGELOG.md` | auto-appended, one line per merge (rotated to `changelog.archive/`). |
| `schemas/` | JSON Schemas: `state`, `decisions`, `spec.frontmatter` — the contracts. |
| `specs/` | the specs (`PREFIX-NNN.md`) + `_TEMPLATE.md` (the depth bar). |
| `decisions/` | immutable `DR-NNNN` decision records + `_TEMPLATE.md`. |
| `reality-checks/` | dated audit docs the gates point at via `evidence_ref` (`round-N.md`, `intent-N.md`). |
| `agents/` | the roster (implementer · shipper · deployer · spec-drafter · auditor · intent-auditor), backend-parameterized. |
| `principles-check.sh` | the grep gate: spine traceability + fence balance + banned patterns. |
| `changelog.archive/` | rotated CHANGELOG entries past 200. |
| `.claude/hooks/` | `session-start` (orient), `turn-check` (Stop self-check), `emit-event` (→ Mission Control), `guard-irreversible` (PreToolUse bound). |

## Resumability contract

If the session dies, a fresh one resumes **from disk alone** (CONSTITUTION A1). The whole world is three
files, read in order:

1. `state.json` — `resume_pointer` first, then phase / subsystems / gates / in-flight agents.
2. `CHANGELOG.md` — the top (most recent merges).
3. `decisions.json` — open `bounded` / `needs-input` records (the only ones that pause work).

The SessionStart hook injects this brief automatically; the Stop hook re-validates `state.json` against its
schema every turn. A failed validation is a bug to fix, not ignore.

## How 0xos Mission Control reads this

The app watches each registered harness and **derives** the human surfaces — it never hand-maintains them:

- **Corrections inbox** ← `decisions.json` (assumptions to review, bounds to raise).
- **Digest** ("what changed since I last looked") ← state-diff over `state.json` + `CHANGELOG.md`.
- **Fleet** (live agent swim-lanes) ← lifecycle events from the hooks (SQLite event store).
- **Specs** (traceability matrix + dependency DAG) ← spec frontmatter + `state.json.subsystems`.

Machine artifacts are terse JSON/markdown; humans read rich generated HTML projections (CONSTITUTION A9).
Edits round-trip back to source.

## Fresh-machine bootstrap

```sh
git clone <this repo> && cd meow
# tools: a git with worktree support, the rust toolchain, and an ACP backend
#        (claude-agent-acp and/or omp acp — co-equal, pick per spec)
# private secrets live in git by design (GIT.md); pull them with the repo
~/0xos/template/init.sh   # only if the harness isn't scaffolded yet
```

Then open **0xos** and drive meow from there — the app is the interface; the terminal is not.
