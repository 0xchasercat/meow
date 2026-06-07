---
spec_id: PREFIX-NNN
title: <one line>
subsystem: <path or service>
status: drafting          # drafting | approved | in_progress | in_review | merged  (flips at merge via git hook)
blast_radius: medium      # low | medium | high
plan_ref: §X.Y            # REQUIRED, non-empty — where in PLAN.md this traces
constitution_ref: [I-1]   # REQUIRED, non-empty — invariant ids and/or DR ids
depends_on: []            # other spec ids
estimate: medium          # small | medium | large
backend: any              # claude | omp | any  (preferred agent for this work)
---

## Motivation
Why this exists. Trace it to `PLAN.md §X.Y` and the constitution refs above. If you can't trace it, it shouldn't be built.

## Acceptance criteria
Concrete and checkable — real route signatures, SQL DDL, function stubs, exact behaviors. (Depth bar: this section is where specs earn their keep.)

## Non-goals
What this deliberately does not do.

## Interface
The contract: endpoints, types, schemas, CLI surface.

## Implementation sketch
The intended approach. Name the comment-marker fences for any shared-file edits: `// === PREFIX-NNN ===`.

## Tests required
Name the invariants this proves and which GATES apply. Unit / integration / red-team / property as relevant.

## Rollout
Migration, flags, deploy notes. Reversibility: how this is undone.

## Operator notes
Anything the human should know. Assumptions made → log them to `decisions.json` as `assumption` records, don't bury them here.
