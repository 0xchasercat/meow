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
