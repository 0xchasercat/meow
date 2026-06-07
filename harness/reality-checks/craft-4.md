# craft gate · round 4 — RT-003 (erasable type-strip)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after the fix loop)
- **At:** 2026-06-07 · **Scope:** `crates/graph` (strip.rs, ir.rs)
- **Reviewer:** independent (not the author), against `harness/CRAFT.md` + CANON §9. One round → 3 strip-correctness findings (1 block, 2 major); all fixed + regression-tested.

The strip is safety-critical (it produces the JS V8 runs, I-3), so the reviewer hunted for under/over-blanking with concrete inputs — and found real gaps:

1. **(BLOCK) TS-only class members under-stripped.** `class C { declare x: number }` left a runtime `x;`; `abstract g(): void` left `g;`; index signatures `[k:string]: number` leaked. **Fix:** `declare`/`abstract` members + `TSIndexSignature` are blanked whole. Tests: `erases_declare_field_entirely`, `erases_abstract_method_entirely`, `erases_index_signature_entirely`.
2. **(major) `implements` removed by substring scan** → `class C extends B<"implements"> implements I {}` blanked the string literal, left the real clause. **Fix:** span-based — blank `[heritage_end .. last_implements.end]`. Test: `implements_located_by_span_not_substring` (uses a runtime string arg the old scan would have destroyed).
3. **(major) `PermissivePolicy` could lower non-erasable TS** into silently-wrong JS. **Fix:** the erasability check is now **intrinsic** — `compute_runtime_ir` always runs it for TS before the installed policy. Test: `permissive_policy_cannot_lower_non_erasable_ts` (enum + param-property → `Some(Err)`).

## Result
Floor green: `clippy -D warnings` clean · `cargo test --workspace` **81 pass** (graph 16+6 / runtime 10 / pkg 29 / config 11 / cli 9) · `fmt` · `principles`. `strip-fidelity` gate green (`strip-fidelity-1.md`).
