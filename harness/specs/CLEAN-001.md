---
spec_id: CLEAN-001
title: "Remove the unused rolldown dependency"
subsystem: crates/tool
status: drafting
blast_radius: low
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-10]
depends_on: [TOOL-003]
estimate: small
backend: any
---

## Goal
Remove the `rolldown` pin (TOOL-003 bundling is skeleton-only; zero `.rs` call sites) from `crates/tool/Cargo.toml` + the workspace `[workspace.dependencies]` TOOL-003 fence.

## Arc outcome
DONE in the cleanup arc. Verified zero `rolldown` usages in any `crates/**/*.rs`; build stays green.
