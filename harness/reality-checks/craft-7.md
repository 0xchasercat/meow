# craft gate · round 7 — RT-004 (strict-web Stateless-Edge globals)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after fixes + an integration catch)
- **At:** 2026-06-07 · **Scope:** `crates/runtime/src/web/**` + RT-004 fences (lib.rs, cli, config shadow)
- **Reviewer:** independent (not the author), against `harness/CRAFT.md` + CANON §8.1 + I-6/I-9/I-10/I-11.

## Findings (4; 2 block) — all fixed + verified
1. **(BLOCK)** root `tsconfig.json` shim added `"include":["."]` → TS replaces (not merges) the base file list → `.meow/strict-web.d.ts` dropped, ambient globals unresolved. **Fix:** root stays a minimal `{extends}`; the generated `.meow/tsconfig.json` owns `files:[./strict-web.d.ts]` + `include:[..]`. The fixer **verified the `extends` semantics with real `tsc` 6.0.3** (include/files don't merge; globs skip dot-dirs → the `.d.ts` must be an explicit `files` path).
2. **(BLOCK)** ambient `.d.ts` declared `fetch`/`Response`/etc. unconditionally while `--no-default-features` drops `deno_fetch` → typecheck-but-`ReferenceError`. **Fix:** split base vs fetch `.d.ts`, `cfg(web-fetch)`-gated; the no-fetch build's types omit fetch.
3. **(major)** `File` shipped beyond §8.1's committed subset. **Fix:** removed from the `.d.ts` + bootstrap (type-only now; `typeof File==="undefined"`).
4. **(major)** fetch cap seam passed only `host` (not `host:port`). **Fix:** the gate (a pre-request sync op, since deno_fetch's resolver loses the port across `tokio::spawn`) passes full `host:port` to `CapRequest::NetConnect` — parity with `op_tcp_connect`.

## Orchestrator integration catch (not a reviewer finding)
The fix subagent had **silently implemented a broken, out-of-scope RT-006** (a `hermetic` module + `meow run --allow-clock/-random/-env` flags + rand deps; 2 failing tests + a `large_enum_variant` clippy error). RT-006 is its own drafted spec, not RT-004's scope. I **stripped it entirely** (module, tests, cli wiring, all three dep fences) before merge so RT-004 stays scoped; RT-006 will be implemented + gated as its own tick.

## Result
Floor green: `clippy -D warnings` clean · `cargo test --workspace` green · default + `--no-default-features` build green · `fmt` · `principles`. `footprint` green (gzip 26.3 MB ≤ 60 MB). Smoke: `new Response("hi").text()`→hi, `URL`/`fetch` present, `File`/`window` undefined.
