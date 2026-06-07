# craft gate · round 1 — wave 1b (PKG-001, CFG-001)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after the fix loop)
- **At:** 2026-06-07 · **Scope:** `crates/pkg`, `crates/config`, the `meow sync` CLI wiring
- **Reviewer:** independent (not the author); two rounds, against `harness/CRAFT.md`.

## Round 1 → would-reject (3 findings)
1. `Cache::store` trusted any existing file at the hash path → a corrupt/truncated blob could not be repaired by re-storing the correct bytes.
2. `Version`/`VersionReq` were unvalidated `String` newtypes → malformed versions (`"not-a-version"`) crossed into the lockfile execution contract (I-7).
3. A bad `integrity` SRI was reported as `LockError::Json` ("invalid JSON") instead of a hash/field error → misleading diagnostic.

## Fixes applied (author)
1. `store` re-hashes an existing blob and only no-ops on a match; a mismatch falls through to the atomic rewrite — the cache **self-heals**. Test: `store_repairs_a_corrupt_blob`.
2. Added the `semver` crate; `Version::parse`/`VersionReq::parse` + custom `Deserialize` validate untrusted text and reject garbage. Tests: `version_parse_accepts_semver_rejects_garbage`, `version_req_parse_accepts_range_rejects_garbage`.
3. `diagnose_line` (error path only) pins the bad field and returns typed `LockError::HashField{line,..}` / `VersionField{line,..}`, falling back to `Json` only for structural errors. Tests: `parse_bad_integrity_is_hash_field_error`, `parse_bad_version_is_version_field_error`.

## Round 2 → findings 1 & 3 RESOLVED; finding 2 STILL-OPEN (1 block)
The **producer** hole remained: public unchecked `Version::new`/`VersionReq::new` let the crate *emit* a malformed lockfile even though the *parse* side now rejected one.

**Fix:** removed the unchecked constructors from production — they are now `#[cfg(test)]` only; production constructs `Version`/`VersionReq` solely via the validated `parse`/`Deserialize`. The lockfile contract now holds on **both read and write**. Verified mechanically: clippy's dead-code error confirmed `new` had no production caller; `cfg(test)` removes it; floor stays green.

## Result
All findings resolved. Floor green: `clippy -D warnings` clean · `cargo test --workspace` 45 pass (pkg 29 / config 11 / cli 5) · `fmt --check` · `principles-check`.
