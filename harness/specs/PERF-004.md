---
spec_id: PERF-004
title: "V8 code-cache reuse for user modules"
subsystem: crates/runtime
status: drafting
blast_radius: medium
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [CANON-26, I-11]
depends_on: [PERF-001]
estimate: medium
backend: any
---

## Goal
Reuse a V8 compile/code cache for user modules across runs so warm invocations skip reparse/recompile (the cold-start battleground from the perf thesis).

## Arc outcome
PROPOSED — draft only; scope not yet sourced/confirmed against the audit. Not actioned in the cleanup arc.
