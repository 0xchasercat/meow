---
spec_id: CLEAN-002
title: "Retire / de-canonize the TrivialModuleLoader P0 stand-in"
subsystem: crates/runtime
status: drafting
blast_radius: low
plan_ref: "Post-Great-Harvest cleanup audit (read-only audit @ HEAD db8d95c); not in PLAN.md proper"
constitution_ref: [I-11]
depends_on: [RT-001, DR-003]
estimate: small
backend: any
---

## Goal
The P0 `TrivialModuleLoader` carries a `.cjs` refusal that asserts "meow is ESM-only (I-2/ADR-3)" — a claim the Great Harvest (DR-003: Deno owns CJS) now contradicts. Strip the obsolete refusal.

## Arc outcome
PARTIAL (done: the contradictory `.cjs` ESM-only refusal arm was removed — `.cjs` now falls through to the honest generic "unsupported extension" message; behavior unchanged: still `Err`). NOT deleted and NOT `#[cfg(test)]`-gated: the loader is live test-support infra (used by 4 integration test files: runtime/io/hermetic/web), and integration tests link the normally-compiled lib so `#[cfg(test)]` would break them. The `.ts` refusal is retained verbatim (it is asserted by `typescript_entry_fails_honestly`, must keep substring "RT-003", and is still accurate: this stand-in cannot type-strip). Full retire deferred to the supervised architectural pass.
