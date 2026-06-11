---
spec_id: PERF-003
title: "Remove the second transpiler (deno_ast)"
subsystem: crates/runtime, crates/graph
status: drafting
blast_radius: medium
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-1, I-10]
depends_on: [RT-003, RT-007]
estimate: medium
backend: any
---

## Goal
Retire the second transpiler `deno_ast` (currently wired as `extension_transpiler`) once the single oxc type-strip pipeline (I-1/RT-003) subsumes its role — footprint + startup win, one parser.

## Arc outcome
DEFERRED — held; `deno_ast` is currently load-bearing as `extension_transpiler` and was explicitly fenced off from the SAFE cleanup arc. Do not remove without the supervised pass.
