# craft gate · round 2 — GRAPH-001 (the canonical parse pipeline)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after the fix loop)
- **At:** 2026-06-07 · **Scope:** `crates/graph` (+ its root `Cargo.toml` fence)
- **Reviewer:** independent (not the author), against `harness/CRAFT.md`. One review round → 2 priority-1 blocks; both fixed + regression-tested.

## Findings (both BLOCK)
1. **`slot()` panicked on a retired `FileId`.** `remove_file`/re-`set_file` retire ids, so a caller holding an id across file-watch churn would crash the process — a no-panic-on-input violation on the canonical pipeline.
2. **`runtime_ir` reported success for un-runnable code.** Under the default permissive policy it returned `Ok(RuntimeIr{positions_preserved:true})` for un-stripped **TypeScript** and for files with **recovered parse errors** — a consumer couldn't tell the placeholder from real V8-ready output.

## Fixes (author) + verification
1. `cst`/`semantic`/`runtime_ir` now return `Option<…>` (None = unknown/retired id); `slot()` returns `Option`; no panic path. Test: retired id after `remove_file` → `None`.
2. Stage 5 is honest: parse/semantic errors → `ir:None` + a summary diagnostic; policy rejection → its diagnostics; under the permissive default a **TS** input is rejected with "type stripping not yet available (RT-003)" (kept behind the `// === RT-003 ===` fence); only clean error-free **JS** lowers to the identity IR. Tests: clean JS → `Some(Ok)`, TS → `Some(Err)` (RT-003), parse-error file → `Some(Err)`, rejecting policy → `Some(Err)`.

Re-confirmation of the binary findings was via the new regression tests (which assert the exact required behaviors) + integrator read of `ir.rs`/`integrity.rs`.

## Integration notes
- Hand-built incremental memo chosen over salsa (oxc artifacts are `!Send`/`!Sync`/`!Update`); isolated behind `GraphDb`.
- `Arc<Cst>` → `Rc<Cst>` (clippy `arc_with_non_send_sync`: the DB is single-threaded; atomic refcount was pointless).
- DIST-001's unused umbrella `oxc = "0.40"` removed; meow uses oxc **sub-crates** `0.134`.

## Result
Floor green: `clippy -D warnings` clean · `cargo test --workspace` **50 pass** (graph 5 / pkg 29 / config 11 / cli 5) · `fmt --check` · `principles-check` (P15 single-parser ✓).
