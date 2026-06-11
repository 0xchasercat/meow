---
spec_id: CLEAN-005
title: "Harness hygiene: refresh state.json + mark CFG-002 superseded"
subsystem: harness
status: drafting
blast_radius: low
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-11]
depends_on: [DR-003, CFG-003]
estimate: small
backend: any
---

## Goal
Refresh `harness/state.json` to post-Harvest reality (phase -> P2.5 Drop-In Core per DR-003/CFG-003; `updated_at` -> HEAD commit time) and mark the superseded `CFG-002` spec (legacy root package.json projection) as superseded by `CFG-003`.

## Arc outcome
DONE. Sourced from DR-003 + CHANGELOG + CFG-003 `plan_ref`; no invented state.
