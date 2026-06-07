# footprint gate · round 3 — RT-004 (web + fetch extensions linked)

- **Gate:** `footprint` (CONSTITUTION I-10) · **verdict:** `green`
- **At:** 2026-06-07 · **Spec:** RT-004 (strict-web globals via deno_web/deno_crypto/deno_fetch)

```
footprint: meow on-disk=79711392 gzip=26327861 budget=62914560 status=ok
```

- gzip (the budgeted compressed download, CANON §26.1): **26.3 MB** · budget: **60 MB** → **ok**.
- Grew from 18.9 MB (RT-002) → 26.3 MB: RT-004 links `deno_fetch` (hyper + rustls + aws-lc) for `fetch`. Still comfortably under the **compressed** budget.
- **Honest watch point:** on-disk is now **79.7 MB** (raw, not the budgeted figure). The budget is the compressed download (§26.1), where 26.3 MB has headroom — but the raw size is climbing; the `web-fetch` cargo feature is a live lever (verified `--no-default-features` builds) to drop the heaviest tree (fetch) as **RT-004a** if the compressed figure ever approaches 60 MB. Track per `DIST` change.
