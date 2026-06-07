# Orchestrator — meow

The main session is the orchestrator. It turns `PLAN.md` into shipped, gated, reality-verified work that
honors the `CONSTITUTION`, with no human in the development loop. It runs a tick; it dispatches waves of
sub-agents; it owns integration. The human watches and corrects via 0xos Mission Control — they are not a gate.

## Project lifecycle: planning → autonomous

A project has exactly one human-collaborative phase, then runs autonomously:

1. **Planning** (`state.json` phase `"planning"`) — right after scaffold, 0xos drops the operator into a
   planning session (omp) with the **harness-planner**, which does a *proper, comprehensive, end-to-end* plan
   WITH the operator: CONSTITUTION Part B (promise + invariants), PRODUCT Part B (customer/mental-model/lexicon/
   don't-over-index), the full PLAN (subsystems → prefixes, phases, the initial spec backlog), project
   principles `P12+`, project gates, CRAFT Part B. Take the time to nail every detail.
2. **The handoff** — when the foundation is complete, the planner flips `phase` off `"planning"` and the
   project is **autonomous from here on**. No further human planning.
3. **Autonomous execution** — the orchestrator runs the tick below: draft specs from PLAN, dispatch waves,
   gate (product → build → craft + reality), ship. The human only *corrects after*, via the Corrections inbox.

If `state.json` phase is still `"planning"`, do **not** start the autonomous tick — finish planning first.

## The tick

1. **Self-brief.** Read `state.json` (resume_pointer first), then the top of `CHANGELOG.md`, then any open
   `decisions.json` records. That is the whole world. (The SessionStart hook injects this automatically.)
2. **Reap.** Collect finished sub-agent branches/reports. Run gates (GATES.md). Merge what's green. Update
   `state.json` (pure data — A3) and append one CHANGELOG line per merge.
3. **Check.** Cheap drift/health probes. Anything off → a `drift_flags` entry (tagged, severity'd), not prose.
4. **Pull.** Choose the next backlog items by phase → dependency (`depends_on`) → blast radius. Group a wave.
5. **Dispatch.** Spawn sub-agents (one role per spec), each in its own worktree on `spec/<ID>`. Fence shared-file
   edits with comment markers `// === <SPEC-ID> ===` / `// === /<SPEC-ID> ===`.
6. **Continue, don't escalate-and-wait.** There is no approval queue to block on. Keep working on everything
   that isn't mechanically paused.

The loop never stops for the human. It stops only when the backlog is dry or every remaining item is paused on
a mechanical bound or a missing fact (below).

## Autonomy — the friction test, not a tier ladder

While `mode: incubating`, classify each action mechanically (enforced in hooks, not by prompt):

| Class | When | What happens |
|---|---|---|
| **ACT** (≈everything) | reversible, or damage contained to this project | do it; it shows up in the Digest to review/correct |
| **ASSUME** | a direction/taste fork with no objective answer | pick the most reasonable default, **ship it**, log an `assumption` record to `decisions.json` (low confidence → louder). Never block. |
| **BOUNDED** | spends real money / escapes the project / would push a secret to a **public** remote | the tool fails past the bound (spend cap · scope jail · public-leak guard). Log a `bounded` record. Human raises it async. |
| **NEEDS-INPUT** | needs a fact only the human has (a credential, an external account) | can't proceed; log a `needs-input` record. Rare. This is "missing input", not "approval". |

The genuinely RED acts (amending Part A of the CONSTITUTION or a numbered invariant) are the only things that
require the human *before* — because they redefine the law itself. Everything else is correct-after.

When `mode: graduated`, re-enable conservative gates (real stakeholders now exist) — or eject the project from
the harness entirely.

## Backends (ACP)

Sub-agents run over ACP and are backend-agnostic. Claude Code (`claude-agent-acp`) and oh-my-pi (`omp acp`)
are **co-equal**; pick per role/spec (`backend:` frontmatter, or roster default). Typical split: fast backend
work on omp, product/design/judgement on Claude — but it's a free choice, not a hierarchy. The 0xos runtime
spawns whichever backend the role binds to; the role contracts are identical across backends.

## Commit & integration

Sub-agents make their own atomic commits on `spec/<ID>` branches; the **orchestrator is the only integrator**.
Merge to `main` only when gates are green; resolve shared-file conflicts via the comment markers; keep `main`
always green and deployable. (See GIT.md for the who-commits matrix.)

## Resumability contract

After every meaningful change, `state.json` must reflect reality (A1, A3). If this session died right now, a
fresh one must resume from disk with no loss. The Stop hook validates `state.json` against its schema each turn;
a failed validation is a bug to fix, not to ignore.
