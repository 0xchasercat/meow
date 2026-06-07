# Changelog

Auto-appended, one line per merge — `YYYY-MM-DD · <spec> merged · <what> · gates <verdict>`.
This is NOT a hand-curated narrative (that was the 258KB/139KB HANDOFF smell). The "where are we"
view is *derived* by Mission Control's Digest from `state.json`; this file is just the durable
changelog. Rotated to `changelog.archive/` past 200 entries.

<!-- 0xos:changelog:top -->
2026-06-07 · planning complete — foundation set; autonomous execution begins. CONSTITUTION/PRODUCT/PLAN/PRINCIPLES/GATES/CRAFT Part B + CANON.md filled, traceable (I-1..I-11 ↔ 11 gates ↔ P12-P17); phase → P0 Foundations.
2026-06-07 · DIST-001 merged · Cargo workspace + meow CLI skeleton (18 honest stubs, EXIT_UNIMPLEMENTED=3; upstream V8 pins declared-not-consumed) · gates: floor green (build/fmt/clippy/test 4·4/principles), footprint green (gzip 392KB ≤ 60MB).
2026-06-07 · PKG-001 merged · meow-pkg: meow.lock.jsonl (strictly-sorted JSONL, semver-validated) + content-addressed cache with hash-integrity + self-heal (I-7) · gates: floor green, craft green (2 rounds, 3 findings fixed).
2026-06-07 · CFG-001 merged · meow-config: defineMeow serde model + shadow .meow/tsconfig.json gen + root shim (ADR-8) + wired `meow sync` · gates: floor green, craft green.
2026-06-07 · GRAPH-001 merged · meow-graph: Oxc parse pipeline (lossless CST / semantic / runtime-IR seam) as a hand-built incremental query DB — the single parser entrypoint (I-1); honest stage-5 rejects TS/errors until RT-003 (I-3/I-11) · gates: floor green (50 tests), craft green (2 blocks fixed: no-panic FileId, honest runtime_ir).
2026-06-07 · RT-001 merged · meow-runtime: embeds V8 (deno_core 0.403 / v8 149), isolate + event loop + op layer; `meow run` executes ESM through V8 (P0 exit "trivial ESM" met); never panics on user JS (I-6); refuses first-party .cjs (I-2) · gates: floor green (64 tests), craft green (2 fixed: .cjs refusal, honest argv), footprint green (V8 linked: gzip 18.3MB ≤ 60MB).
2026-06-07 · RT-003 merged · meow-graph: erasable-only TS type-strip (whitespace-blank via oxc_ast_visit — positions byte-stable, no codegen/sourcemap, I-3/§9); ErasablePolicy rejects enum/namespace/param-properties/import=/export= and is now the GraphDb default; erasability gate unconditional for TS · gates: floor green (81 tests), craft green (3 strip bugs fixed), strip-fidelity green.
2026-06-07 · LOAD-001 merged · meow-loader: the single resolver (I-1/I-5) + content-addressed cache read (integrity-checked, I-7) + strip wiring; `meow run` now strips-and-runs TS end-to-end and resolves deps from the cache with NO node_modules (P0 exit met) · gates: floor green (33 loader+cli tests), craft green (3 fixed: project-root discovery, duplicate-name ambiguity, meow-cache URL strictness).
