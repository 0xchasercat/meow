# craft gate · round 3 — RT-001 (the runtime)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after the fix loop)
- **At:** 2026-06-07 · **Scope:** `crates/runtime` + the `meow run` wiring in `crates/cli`
- **Reviewer:** independent (not the author), against `harness/CRAFT.md`.

## No-panic-on-user-JS contract — PASSED
The load-bearing RT-001 bar (an uncaught exception / syntax error / rejected TLA / missing-or-`.ts` entry / event-loop failure each becomes a typed `RuntimeError`, never a Rust panic) drew **no findings** — the reviewer accepted the error handling. Regression tests cover the throw / syntax-error / rejected-TLA / honest-`.ts`-failure paths.

## Findings (fixed + tested)
1. **(BLOCK) first-party `.cjs` was accepted** — the P0 loader treated `.cjs` as JavaScript, so `meow run app.cjs` would execute locally-authored CommonJS: a direct **I-2 / ADR-3** violation. **Fix:** the loader now refuses `.cjs` ("first-party CommonJS is not supported — meow is ESM-only"). Test: `run_refuses_first_party_cjs` (stdout has no program output; stderr says CommonJS).
2. **(major) `meow run … -- foo bar` silently dropped the args** while the CLI help claimed forwarding. **Fix:** `cmd_run` rejects non-empty trailing args honestly ("forwarding … is not yet supported") and the `argv` help no longer over-claims (forwarding is a later spec). Test: `run_rejects_unwired_argv_instead_of_dropping_it`.

## Result
Floor green: `clippy -D warnings` clean · `cargo test --workspace` **64 pass** (runtime 10 / cli 9 / pkg 29 / config 11 / graph 5) · `fmt --check` · `principles-check` (I-6/P16 + I-10). `footprint` green with V8 linked (gzip 18.3 MB ≤ 60 MB — `footprint-2.md`).

## P0 exit progress
"a trivial ESM file runs through V8" is now **met**: `meow run hello.mjs` executes through V8 and prints (test `run_executes_a_trivial_mjs_module`).
