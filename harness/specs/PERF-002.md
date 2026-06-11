---
spec_id: PERF-002
title: "Release build tuning + pin toolchain"
subsystem: workspace, harness
status: drafting
blast_radius: low
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-10]
depends_on: [DIST-001]
estimate: small
backend: any
---

## Goal
Add a root `[profile.release]` (`lto=true`, `codegen-units=1`, `strip=true`; deliberately NOT `panic="abort"` — it changes semantics) and a `rust-toolchain.toml` pinning the stable channel currently in use.

## Arc outcome
DONE in the cleanup arc (build-tuning only; the transpiler change is out of scope). Release build verified to still succeed; binary size recorded before/after in the report.
