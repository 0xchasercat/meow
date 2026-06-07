# footprint gate · round 1 — DIST-001

- **Gate:** `footprint` (CONSTITUTION I-10) · **verdict:** `green`
- **At:** 2026-06-07 · **Spec:** DIST-001 (Cargo workspace + meow CLI skeleton)

## What was re-derived
Measured the release binary independently via `scripts/footprint.sh` (the gate's own probe) and audited the manifest for the no-fork / no-embedded-toolchain claim.

```
footprint: meow on-disk=972048 gzip=391720 budget=62914560 status=ok
```

- on-disk: **972 KB** · gzip (the budgeted compressed figure, CANON §26.1): **392 KB** · budget: **60 MB** → **ok**.
- Manifest audit (`Cargo.toml`): `deno_core`/`v8` are declared as plain crates.io version pins (`deno_core = "0.340"`, `v8 = "130"`), **no `git =` fork source** → upstream-only (I-10, ADR-1, CANON §26.2). `principles-check.sh [I-10]` confirms: `no forked V8 source`.
- No `rustc`/`cargo`/clang embedded; the heavy externals are *declared, not consumed* by this skeleton (only `clap` compiles), so the build pulled none of V8.

## Honest boundary
This figure is for the **V8-less skeleton** — trivially green. The ≤ 60 MB budget becomes meaningful at **RT-001**, when `rusty_v8`'s prebuilt static V8 actually links in. Footprint must be re-run there; that is the real test of I-10.
