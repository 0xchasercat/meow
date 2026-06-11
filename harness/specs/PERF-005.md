---
spec_id: PERF-005
title: "Linux io_uring fast-path for host I/O"
subsystem: crates/runtime
status: drafting
blast_radius: medium
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-11, ADR-9]
depends_on: [RT-002]
estimate: medium
backend: any
---

## Goal
Add an io_uring fast-path on Linux behind the existing async host-I/O capability seam (RT-002 / ADR-9: tokio is the portable layer, io_uring is the platform accelerator) — honest, reproducible-benchmark gated.

## Arc outcome
PROPOSED — draft only; scope not yet sourced/confirmed against the audit. Not actioned in the cleanup arc.
