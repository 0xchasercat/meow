---
spec_id: CLEAN-003
title: "Cut cjs.ts over to Deno NodeRequireLoader"
subsystem: crates/runtime, crates/loader
status: drafting
blast_radius: high
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-1, I-5, DR-003]
depends_on: [DR-003]
estimate: large
backend: any
---

## Goal
Replace the synthetic-ESM CJS bridge (`cjs.ts`) with Deno`s real `NodeRequireLoader` so CJS executes through Deno`s require machinery over unpacked cache paths.

## Arc outcome
DEFERRED — architectural; held for a separate supervised pass (explicitly out of scope for the SAFE cleanup arc).
