---
spec_id: CLEAN-004
title: "Remove dead deno_url + deno_console direct pins"
subsystem: crates/runtime
status: drafting
blast_radius: low
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-10]
depends_on: [RT-004, RT-007]
estimate: small
backend: any
---

## Goal
At the `deno_core 0.403` generation URL/URLPattern/console live in `deno_web`; `deno_url`/`deno_console` have no `deno_url::`/`deno_console::` call sites in `crates/runtime/src`. Remove the direct pins from `crates/runtime/Cargo.toml` + the workspace RT-004 fence IF they are not required transitively (build must stay green).

## Arc outcome
See the cleanup-arc report (kept-or-reverted is gated empirically: removed only if `cargo build --release` + node_compat stay green; otherwise retained as transitive Node-graph deps and noted).
