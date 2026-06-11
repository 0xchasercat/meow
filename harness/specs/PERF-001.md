---
spec_id: PERF-001
title: "V8 startup snapshot + lazy node builtins"
subsystem: crates/runtime
status: drafting
blast_radius: high
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-11, CANON-26]
depends_on: [RT-001, DR-003]
estimate: large
backend: any
---

## Goal
Ship a V8 startup snapshot (+ code cache) and lazily initialize `node:*` builtins so cold-start cost is pre-parsed/pre-init heap state rather than per-process parse.

## Arc outcome
DEFERRED — architectural; held for a separate supervised pass.
