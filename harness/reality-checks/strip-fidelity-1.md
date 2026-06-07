# strip-fidelity gate · round 1 — RT-003 (I-3)

- **Gate:** `strip-fidelity` (CONSTITUTION I-3) · **verdict:** `green`
- **At:** 2026-06-07 · **Spec:** RT-003 (erasable TypeScript type-strip)

## What was re-derived
The gate's reality check (GATES.md): *property test — `strip → reparse` equals the source AST minus types, source positions stable; non-erasable fixtures error with a fix message; no downlevel JS.* This is exercised by the `crates/graph` strip corpus **and** independently craft-reviewed for correctness.

- `strip_fidelity_corpus` (18-entry erasable-TS corpus): each stripped output (a) reparses as **valid JS with zero errors**, (b) is a pure **blanking** of the source — identical byte length, every retained byte unchanged, newlines preserved (so byte/line/column positions are stable), (c) type tokens gone, value tokens kept.
- Per-construct rejection tests: `enum`/`const enum`, runtime `namespace`, parameter properties, `import =`, `export =` each produce the exact fix-pointing `ErasablePolicy` diagnostic; ambient `declare` forms are allowed.
- Class-member coverage (added after craft round 4): `declare`/`abstract` members + index signatures are erased entirely; `implements` is located by span (survives `extends B<"implements">`).
- Erasability is **unconditional** for TS (no policy can lower non-erasable TS).
- Approach honors §9: whitespace-blanking via `oxc_ast_visit`, **no codegen, no sourcemap**.

## Honest boundary
This verifies the strip **capability + fidelity at the GRAPH level**. End-to-end `meow run x.ts` additionally needs the module loader to route TS source through this strip — that wiring lands in **LOAD-001**.
