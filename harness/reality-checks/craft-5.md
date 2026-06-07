# craft gate · round 5 — LOAD-001 (module loader: resolver + cache + strip)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after the fix loop)
- **At:** 2026-06-07 · **Scope:** `crates/loader` + the `meow run` wiring in `crates/cli`
- **Reviewer:** independent (not the author), against `harness/CRAFT.md` + I-1/I-5/I-7/I-6.

## Findings (fixed + tested)
1. **(BLOCK) project root taken from the entry's directory.** `meow run src/main.ts` looked for `src/meow.lock.jsonl`, missing the repo-root lockfile → pinned deps failed. **Fix:** `find_project_root` walks up from the entry's dir to the nearest `meow.lock.jsonl` (host-pure); used for both the lockfile read and the resolver root. Tests: `find_project_root_walks_up_…`, `run_finds_root_lockfile_from_a_nested_entry`.
2. **(major) duplicate package names silently collapsed.** A lockfile with `dep@1` + `dep@2` kept one version with no diagnostic. **Fix:** `load_bare_map` returns a typed `AmbiguousVersion` error (version selection lands in LOAD-003), non-zero exit. Test: `run_rejects_a_lockfile_with_duplicate_package_names`.
3. **(major) decorated `meow-cache:` URLs collapsed.** `…#a` / `…?q` parsed to the same hash under distinct specifiers (same blob, two identities). **Fix:** `decode` rejects any authority/query/fragment; only the exact opaque `meow-cache:<sri>` round-trips. Test: `rejects_decorated_meow_cache_urls`.

## End-to-end (orchestrator smoke)
`meow run app.ts` (`const greet: string = …; greet as string`) → strips → prints; `enum E{A}` → honest refusal ("Enums emit runtime code…"); **no `node_modules`** created (I-5). Cached-dep load is integrity-checked via `meow_pkg::Cache::read` (I-7).

## Result
Floor green: `clippy -D warnings` clean · loader+cli tests **33 pass** (5 new) · workspace tests all green · `fmt` · `principles` (P16 via the `host/` seam). The single resolver (I-1/I-5) is established; full runtime↔LSP `resolver-parity` awaits LSP-001 (P2).
