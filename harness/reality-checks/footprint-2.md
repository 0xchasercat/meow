# footprint gate · round 2 — RT-001 (V8 linked: the real measurement)

- **Gate:** `footprint` (CONSTITUTION I-10) · **verdict:** `green`
- **At:** 2026-06-07 · **Spec:** RT-001 (embed V8 via deno_core 0.403 / v8 149.2.0)

## What was re-derived
This is the **meaningful** I-10 measurement — round 1 (DIST-001) was the V8-less skeleton (gzip 392 KB). RT-001 links real V8, so this is the number that matters.

```
footprint: meow on-disk=60102800 gzip=18356212 budget=62914560 status=ok
```

- on-disk: **57 MB** · gzip (the budgeted compressed download, CANON §26.1): **18.3 MB** · budget: **60 MB** → **ok** (18.3 MB ≪ 60 MB).
- Honest note: on-disk (57 MB) is near the raw 60 MB figure, but the I-10 budget is explicitly the **compressed download** (CANON §26.1), where 18.3 MB has comfortable headroom.
- Manifest audit: `deno_core = "0.403"` is a plain crates.io version pin (no `git =` fork); `v8` is consumed transitively via deno_core's re-export (no direct/forked v8). No `rustc`/`cargo`/clang embedded. `principles-check.sh [I-10]` clean.
