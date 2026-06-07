---
spec_id: LOAD-003
title: "Full Node module resolution algorithm (conditions, exports/imports, self-reference)"
subsystem: crates/loader
status: drafting
blast_radius: medium
plan_ref: "Build sequence · P1 Minimal Runtime; backlog Wave 2 (LOAD-003)"
constitution_ref: [I-1, I-5]
depends_on: [LOAD-001]
estimate: large
backend: omp
---

## Motivation

LOAD-001 stood up **THE single resolver** (`meow_loader::Resolver`, I-1/I-5) but deliberately shipped a *stand-in* for bare-specifier resolution: a constructor-provided `bare: HashMap<String, ContentHash>` where `name → one cached blob`, treating "one content hash = one module's bytes" (LOAD-001 Non-goals; `resolver.rs` docs name PKG-002 / LOAD-003 as the completion). That is not how packages work. A real dependency is a **tree of files** (its `package.json`, multiple modules, subpaths), and modern packages route imports through `exports`/`imports` condition maps rather than raw file paths.

LOAD-003 replaces the stand-in with the **full Node resolution algorithm** in that same one resolver (CANON §12.2: *"The resolver must implement the full Node resolution algorithm faithfully (ADR-4), and the LSP must use the same resolver"*; ADR-4 consequence: *"the runtime and the language server must share identical resolution logic, or the editor and the runtime will disagree"*). It resolves against the **content-addressed cache + lockfile only — never `node_modules`** (I-5): package `exports`/`imports` maps with conditions, subpath patterns, self-references, `package.json` `main`/`type`, and extension+index resolution, all reading package members straight from cached package archives.

This is the resolution half of P1's MVP: a small TS service that imports real dependencies (with conditional exports and nested deps) runs deterministically from the lockfile with no `node_modules` on disk. It sets up the **`resolver-parity`** gate (I-5): one algorithm, consumed identically by the runtime now and by the LSP (LSP-001) later.

Traceability: `plan_ref` → PLAN.md *Build sequence · P1 Minimal Runtime* + *backlog Wave 2* (`LOAD-003`). `constitution_ref` → **I-1** [`graph-integrity`] (the resolution algorithm exists exactly once; no second resolver), **I-5** [`resolver-parity`] (no `node_modules`; runtime ≡ LSP resolution).

## Acceptance criteria

Concrete, checkable Rust, extending the existing `crates/loader` crate. Every user-triggerable path returns `Result` — the resolver never panics on user input or on a malformed `package.json`/archive (CRAFT Part B). Errors are typed + causal and point at the fix (I-11: no claim without a check; missing/ambiguous resolution is an honest typed error, never a silent wrong file).

### `crates/loader/src/resolver.rs` — the resolver, now lockfile-driven (I-1, I-5)

The `bare: HashMap<String, ContentHash>` stand-in is **removed** (clean cutover, not deprecated alongside). The resolver is constructed from the lockfile and the project's pinned direct dependencies:

```rust
use std::collections::{BTreeMap, HashMap};
use std::cell::RefCell;
use std::sync::Arc;
use deno_core::url::Url;
use meow_pkg::{Cache, ContentHash, Lockfile, LockEntry, PackageName, Version};

pub struct Resolver {
    cache: Arc<Cache>,
    /// The execution contract (I-7): `(name, version) → LockEntry` (transitive deps,
    /// integrity). Read-only here; PKG owns all writes.
    lockfile: Arc<Lockfile>,
    /// The project's *direct* dependencies, pinned to exact versions by `meow install`
    /// (the root has no LockEntry of its own). First-party bare specifiers resolve through this.
    root_deps: BTreeMap<PackageName, Version>,
    /// Reverse index `package integrity hash → (name, version)`, built once from `lockfile`,
    /// so a cached referrer URL maps back to its owning `LockEntry` (→ its transitive deps,
    /// → self-reference name). O(entries) once; resolution stays O(1) per lookup.
    by_integrity: HashMap<ContentHash, (PackageName, Version)>,
    /// Lazily-materialized, memoized read-only view of each cached package's files, keyed by
    /// the package's integrity hash. Never writes to disk — no `node_modules` (I-5).
    packages: RefCell<HashMap<ContentHash, Arc<PackageFs>>>,
    /// `file://` base for first-party resolution; default referrer for the entry module.
    project_root: Url,
}

impl Resolver {
    pub fn new(
        cache: Arc<Cache>,
        lockfile: Arc<Lockfile>,
        root_deps: BTreeMap<PackageName, Version>,
        project_root: Url,
    ) -> Resolver;

    pub fn project_root(&self) -> &Url;

    /// Pure resolution — the single algorithm. `specifier` + `referrer` → resolved URL +
    /// locator, reading only `package.json` members from cached archives (no source I/O on the
    /// resolved module). Both `resolve()` and deno_core's sync `resolve` (URL dedup) call this.
    pub fn locate(&self, specifier: &str, referrer: &Url) -> Result<(Url, ModuleLocator), ResolveError>;

    /// `locate` + read the resolved module's bytes (disk for `file:`, the package archive member
    /// for `meow-cache:`). Cache reads recompute + verify the hash (I-7); integrity failures
    /// propagate as `ResolveError::Cache`, never swallowed.
    pub fn resolve(&self, specifier: &str, referrer: &Url) -> Result<ResolvedModule, ResolveError>;
}
```

`ModuleLocator` and `ModuleKind` gain the package-member shape and format classification:

```rust
#[derive(Debug, Clone)]
pub enum ModuleLocator {
    LocalFile(PathBuf),
    /// A specific member file *inside* a cached package archive (no `node_modules`).
    /// `member` is the package-relative path, e.g. `"dist/index.js"`.
    Cached { package: ContentHash, member: String },
}

/// Resolution outcome's module format, decided by ESM_FILE_FORMAT (extension + nearest
/// `package.json` `type`). Drives `deno_core::ModuleType` and graph source-type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Esm,
    Json,
    /// A *dependency* CJS module (`.cjs`, or `.js` under `"type":"commonjs"`). LOAD-003 resolves
    /// its LOCATION + format faithfully; synthetic ESM wrapping/execution is LOAD-004 (P5). Until
    /// then the loader surfaces `ResolveError::CjsDependencyUnsupported` (honest boundary, I-11).
    Cjs,
}
```

### `crates/loader/src/package.rs` (new) — `PackageJson` model + `PackageFs` (cached-archive view)

```rust
use std::collections::BTreeMap;
use std::sync::Arc;
use serde::Deserialize;

/// The subset of `package.json` resolution consults. `#[serde(default)]` everywhere — a
/// package may legally omit any of these. Unknown keys are ignored (forward-compatible).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PackageJson {
    #[serde(default)] pub name: Option<String>,
    #[serde(default)] pub version: Option<String>,
    /// `"module"` | `"commonjs"` (absent ⇒ treated as commonjs per Node, but `.mjs`/`.cjs`
    /// extensions always override).
    #[serde(rename = "type", default)] pub package_type: Option<String>,
    #[serde(default)] pub main: Option<String>,
    /// `exports` is the union: bare string, conditions object, subpath map, or subpath patterns.
    #[serde(default)] pub exports: Option<Exports>,
    /// `imports` — internal `#`-specifiers; conditions + patterns, same target grammar as exports.
    #[serde(default)] pub imports: Option<BTreeMap<String, ExportsTarget>>,
}

/// `exports` field grammar (RFC: Node "Package entry points").
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Exports {
    /// `"exports": "./index.js"` — sugar for `{ ".": "./index.js" }`.
    Single(ExportsTarget),
    /// `"exports": { ".": …, "./sub": …, "./feat/*": … }` OR a bare conditions object
    /// `{ "import": …, "require": …, "default": … }`. Disambiguated by whether the first key
    /// starts with `.` (subpath map) vs is a condition name (Node's documented rule).
    Map(BTreeMap<String, ExportsTarget>),
}

/// A resolution target: a relative file, a nested conditions object, an ordered fallback array,
/// or `null` (explicit "blocked subpath").
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ExportsTarget {
    Path(String),                                   // "./dist/x.js"
    Conditions(BTreeMap<String, ExportsTarget>),    // { "import": …, "node": …, "default": … }
    Fallback(Vec<ExportsTarget>),                   // [ "./a.js", "./b.js" ] — first valid wins
    Blocked,                                        // null
}

/// A read-only, in-memory view of ONE cached package's files. Built once per integrity hash from
/// `Cache::read(integrity)` (which verifies the hash, I-7), then memoized on the `Resolver`. Never
/// extracts to disk — the algorithm walks this view, not `node_modules` (I-5).
pub struct PackageFs {
    manifest: PackageJson,
    /// package-relative path → file bytes (e.g. `"package.json"`, `"dist/index.js"`).
    members: BTreeMap<String, Arc<[u8]>>,
}

impl PackageFs {
    /// Decode the cached archive blob into the member map + parse the top-level `package.json`.
    /// A blob that is not a valid archive, or has no `package.json`, is a typed error — never a panic.
    pub fn from_archive(bytes: &[u8]) -> Result<PackageFs, ResolveError>;
    pub fn manifest(&self) -> &PackageJson;
    pub fn contains(&self, member: &str) -> bool;
    pub fn read(&self, member: &str) -> Option<Arc<[u8]>>;
    /// Nearest `package.json` walking up from `member` (for `type` / nested-package detection).
    pub fn nearest_manifest(&self, member: &str) -> &PackageJson;
}
```

### The algorithm (faithful to Node ESM resolution; `// === LOAD-003 ===` private fns in `resolver.rs`)

`locate` dispatches by specifier shape, then runs the standard procedures. Named to match the spec so the LSP and any auditor can trace 1:1:

1. **Absolute URL** (`file:`/`meow-cache:` re-entry from deno_core) → `locate_url` (unchanged classification + format detection).
2. **Relative** (`./`, `../`, `/`) → `referrer.join(specifier)`; the joined URL stays in the referrer's namespace, so a relative import *inside* a cached package resolves to a **sibling member** of the same archive (`meow-cache://<pkg>/dir/x.js` + `./y.js` → `meow-cache://<pkg>/dir/y.js`). Then `finalize_file` (extension + index probing, format detection).
3. **`#`-prefixed** → `PACKAGE_IMPORTS_RESOLVE` against the **owning package's** `imports` map (owning package = `by_integrity[referrer-pkg]`, or `root_deps`/project `package.json` for a first-party referrer).
4. **Bare** (`name` / `@scope/name` / `name/subpath`):
   - **Self-reference:** if `name` equals the owning package's own `name`, resolve the subpath against *its own* `exports` (Node self-reference).
   - Otherwise look up the **exact version**: first-party referrer → `root_deps[name]`; cached referrer → `lockfile.get(owner_name, owner_version).dependencies[name]`. Missing ⇒ `ResolveError::BareSpecifierNotInLockfile { name }` (fix: `meow install`).
   - `lockfile.get(name, version)` → `entry.integrity` → `package_fs(entry.integrity)`.
   - If the package has `exports` → **`PACKAGE_EXPORTS_RESOLVE`** (the map is authoritative; a subpath not in `exports` is `ResolveError::SubpathNotExported`, *never* a fallback file probe — matching Node's encapsulation). Else → legacy **`LOAD_AS_DIRECTORY`**: `main` (→ file probe) else `index` probe.

Procedures (all pure, reading only `PackageFs` manifests/members):

- `package_exports_resolve(pkg, subpath, conditions)` — `"."` for the bare name, `"./x"` for subpaths; supports exact keys and **subpath patterns** (`"./feat/*"` → target with a single `*`), selecting the **longest matching pattern key** (`PATTERN_KEY_COMPARE`), expanding the captured segment into the target's `*`.
- `package_target_resolve(pkg, target, capture, conditions)` — resolves an `ExportsTarget`: `Path` → validated relative member (must start `./`, no `..` escape — `ResolveError::InvalidPackageTarget`); `Conditions` → first matching condition in **insertion order**; `Fallback` → first array entry that resolves; `Blocked`(null) → `ResolveError::SubpathBlocked`.
- `conditions()` — the active set for the ESM loader, matched in this priority: **`["meow", "import", "node", "default"]`**. `"require"` is intentionally **excluded** here (it belongs to CJS `require()` context, LOAD-004); a `require`-only export with no `default`/`import`/`node` for an imported package surfaces `ResolveError::NoMatchingCondition` naming the conditions tried (honest, I-11).
- `esm_file_format(member, pkg)` — `.mjs`→Esm, `.cjs`→Cjs, `.json`→Json, `.js`→ (nearest `package.json` `type`: `"module"`→Esm else Cjs). Drives `ModuleKind` → `deno_core::ModuleType`.
- `finalize_file(pkg, member)` — extension + index resolution for relative specifiers and the legacy `main`/directory fallback: try the literal member, then `+.js/.mjs/.cjs/.json`, then `<member>/index.{js,mjs,cjs,json}`. **Not** applied to `exports`/`imports` targets (those are exact). meow applies probing to relative + legacy-directory resolution as a *documented superset* of strict Node ESM (matches `tsc nodenext`/bundler expectations) — flagged honestly in Operator notes, not advertised as byte-for-byte Node ESM.

### `ResolveError` additions

```rust
#[error("{subpath:?} is not exported by {package:?} (check its package.json \"exports\")")]
SubpathNotExported { package: String, subpath: String },
#[error("{subpath:?} is explicitly blocked (null) by {package:?}'s exports/imports")]
SubpathBlocked { package: String, subpath: String },
#[error("no export of {package:?} matched conditions {tried:?} for {subpath:?}")]
NoMatchingCondition { package: String, subpath: String, tried: Vec<&'static str> },
#[error("internal import {specifier:?} is not defined in {package:?}'s \"imports\"")]
ImportNotDefined { package: String, specifier: String },
#[error("invalid package target {target:?} in {package:?} (must be a relative \"./\" path, no \"..\" escape)")]
InvalidPackageTarget { package: String, target: String },
#[error("malformed package.json in {package:?}: {reason}")]
InvalidManifest { package: String, reason: String },
#[error("malformed package archive for {package:?}: {reason}")]
InvalidArchive { package: String, reason: String },
#[error("dependency {package:?} module {member:?} is CommonJS; CJS interop lands in LOAD-004")]
CjsDependencyUnsupported { package: String, member: String },
```

### Behaviors (the acceptance bar)

- A dependency exposing **conditional exports** (`{ ".": { "import": "./esm/x.js", "require": "./cjs/x.cjs", "default": "./x.js" } }`) imported from first-party ESM resolves to `./esm/x.js` (the `import` condition wins) and runs.
- A **subpath pattern** export (`"./features/*": "./src/features/*.js"`) resolves `import "pkg/features/a"` → `meow-cache://<pkg>/src/features/a.js`.
- A **self-reference** (`import "this-pkg/util"` from inside `this-pkg`) resolves against `this-pkg`'s own `exports`.
- A **nested dependency** (dep A depends on dep B at an exact version) resolves B from A's `LockEntry.dependencies` → `Lockfile::get` → B's archive; two packages depending on different versions of B each get their own version (multi-version is inherent in the `(name,version)` key).
- **`main`/`type`/extensionless:** a legacy package with only `"main": "lib/index.js"` resolves the bare name to it; `import "pkg/sub"` with no exports resolves `sub` → `sub.js`/`sub/index.js`; `"type":"module"` makes a `.js` member ESM.
- **No `node_modules`** is created or consulted on any path; every cached member's URL is `meow-cache://…`.
- **Ambiguity / missing → typed honest error** (the `ResolveError` variants above), never a wrong-file fallback or a panic.

## Non-goals

- **Install / registry fetch / lockfile generation / archive population** — **PKG-002 (P2)**. LOAD-003 only *reads* an already-populated cache + lockfile. It does not download, unpack-to-disk, or write the cache.
- **CJS dependency execution** — LOAD-003 *classifies* CJS members (`ModuleKind::Cjs`) and resolves their location faithfully, but synthetic-ESM wrapping (`cjs-module-lexer`) and running them is **LOAD-004 (P5)**. First-party CJS stays refused (I-2, LOAD-001/LOAD-002).
- **The LSP itself** — **LSP-001**. LOAD-003 guarantees the resolver *is* the one the LSP will consume (same `Resolver::locate`); the shadow-`node_modules` symlink map (§20.1) and editor wiring are LSP-001.
- **VFS / `--materialize` / `--vendor`** — **PKG-003/004 (P2)**. PnP in-memory only.
- **Dynamic `require(variable)` deep requires, `legacy` mode quirks** — diagnostics/wrapping are LOAD-004.
- **Async/`io_uring` archive reads** — synchronous read is acceptable for P1 (`ModuleLoadResponse::Sync`); not a `Resolver` contract change.

## Interface

Public surface of `meow_loader` after LOAD-003:
- `Resolver::new(Arc<Cache>, Arc<Lockfile>, BTreeMap<PackageName, Version>, Url)` + `resolve`/`locate`/`project_root` (signatures above). The `bare` HashMap constructor param is gone.
- `meow_loader::{ResolvedModule, ModuleKind (Esm|Json|Cjs), ModuleLocator (LocalFile | Cached{package,member}), ResolveError}` (expanded variants).
- `meow_loader::package::{PackageJson, Exports, ExportsTarget, PackageFs}` (new module; `PackageFs` may stay `pub(crate)` if no external consumer needs it — LSP-001 consumes via `Resolver`, so default to `pub(crate)`).
- Virtual URL scheme evolves to carry **package identity + member path**: `meow-cache://<algo>-<lowerhex>/<member/path.js>` (authority = the package integrity hash, host-legal lower-hex; path = the package-relative member). This supersedes LOAD-001's single-blob opaque `meow-cache:<sri>` form (clean cutover — the one-hash-one-module form is removed).
- Consumes (do not redefine): `meow_pkg::{Cache, ContentHash, Lockfile, LockEntry, PackageName, Version, CacheError}`, `meow_graph::{GraphDb, FileId}`, `deno_core::ModuleLoader`.

New crate deps: a tar reader + gzip decoder for archive decode (e.g. `tar` + `flate2`, or `astral-tokio-tar`) — pick the smallest that keeps I-10 footprint healthy; `serde`/`serde_json` for `package.json`. No CLI surface of its own.

## Implementation sketch

Extends `crates/loader` (no new workspace member). Files: `src/resolver.rs` (algorithm + lockfile wiring), `src/package.rs` (new: `PackageJson`/`Exports`/`PackageFs`), `src/url.rs` (authority-form scheme), `src/lib.rs` (`load` member read + `ModuleType` from `ModuleKind`), `tests/resolution_corpus.rs` (new).

Shared-file edits — comment-marker fences (`// === LOAD-003 ===`):
- `crates/loader/src/resolver.rs` — remove the `bare` stand-in (and its `package_name` bare-only branch is folded into the new bare procedure); fence the new constructor + algorithm.
- `crates/loader/src/url.rs` — fence the authority-form `encode(pkg, member)` / `decode → (ContentHash, member)`; the old single-arg `encode(hash)` is removed.
- `crates/loader/src/lib.rs` — fence: `load` reads the resolved `Cached{package,member}` via `Resolver::resolve`; `graph_path_for` uses the member's real extension; map `ModuleKind::Json` → `deno_core::ModuleType::Json`, `Cjs` → the `CjsDependencyUnsupported` error until LOAD-004.
- `crates/pkg/src/hash.rs` — fence `// === LOAD-003 ===`: add `ContentHash::to_url_host(&self) -> String` (`"<algo>-<lowerhex>"`, reuses `to_hex`) and `from_url_host(&str) -> Result<ContentHash, ParseHashError>` (hex round-trip), so the URL authority round-trips without leaking base64 `/`+`=` into a host. (Small, additive; flagged for Main — see Operator notes.)
- The run path (`meow run`) where `Resolver::new` is called — fence: build `root_deps` from the project config/lockfile + load the `Lockfile`, drop the `bare` map.

Flow: deno_core `resolve` → `Resolver::locate` (algorithm, reads only manifests) → URL; `load` → `Resolver::resolve` (re-enter, read the member bytes from the memoized `PackageFs`) → `GraphDb::set_file` + `runtime_ir` → `ModuleSource`. One resolver, one graph, one algorithm.

## Tests required

`crates/loader/tests/resolution_corpus.rs` (integration) + unit tests in `resolver.rs`/`package.rs`/`url.rs`. Proves **I-1** [`graph-integrity`] and **I-5** [`resolver-parity`]; sets up the **`resolver-parity`** gate (the corpus is the gate's fixture).

Each corpus case: build a `PackageFs` from a synthetic in-memory archive (or `Cache::store` a real gzip-tar), write a `meow.lock.jsonl` pinning it (+ nested deps), construct the `Resolver`, assert `locate`/`resolve` lands on the exact expected member URL.

- **Conditional exports:** `import`/`node`/`default` priority; `import` chosen over `require`; a `require`-only package import → `NoMatchingCondition` (names tried conditions).
- **Subpath patterns:** `"./feat/*"` longest-match over a competing `"./feat/special"` exact key; `*` capture expanded into the target; `..`-escaping target → `InvalidPackageTarget`.
- **Self-reference:** importing own name resolves via own `exports`; a subpath not in own `exports` → `SubpathNotExported`.
- **Nested deps + multi-version:** A→B@1, C→B@2 each resolve their own B archive; B reached only via the owner's `LockEntry.dependencies` (no global flattening).
- **`main`/`type`/extensionless:** legacy `main` resolves the bare name; extensionless `./x` → `x.js`/`x/index.js`; `"type":"module"` makes `.js` ESM, absent `type` makes `.js` Cjs → `CjsDependencyUnsupported`; `.json` → `ModuleKind::Json` → `ModuleType::Json`.
- **`imports`:** `#internal` resolves via the owning package's `imports`; undefined `#x` → `ImportNotDefined`.
- **Encapsulation:** a real member that exists in the archive but is not listed in `exports` is **not** reachable (→ `SubpathNotExported`), proving `exports` is authoritative.
- **No `node_modules` (I-5, `resolver-parity`):** after a full corpus run, assert no path containing `node_modules` was opened and none exists; every resolved dep URL is `meow-cache://…`.
- **Single resolver / parity (I-1, I-5):** assert relative, bare, self-ref, and `#import` all route through the one `Resolver::locate`; structurally the only `ModuleLoader` impl delegates to it and never references `oxc_parser`. (LSP-001 will reuse this exact entrypoint; the corpus becomes the shared `resolver-parity` fixture.)
- **Robustness (no panic):** malformed `package.json`, truncated/non-archive blob, `exports` with a non-`./` target, integrity mismatch on the package blob → the matching typed `ResolveError` (e.g. `InvalidManifest`, `InvalidArchive`, `ResolveError::Cache`), never a panic.
- **End-to-end run:** a `main.ts` importing a conditional-exports dep that itself imports a nested dep runs to completion through `meow_runtime::Runtime` with `MeowModuleLoader` installed; the transitive dep's behavior is observed.

GATES: `resolver-parity` (I-5 — the corpus *is* this gate's fixture; the LSP later resolves the same corpus → identical results), `graph-integrity` (I-1). Floor: `cargo test/clippy/fmt`, `principles-check.sh`.

## Rollout

Extends an existing crate; no new workspace member, no persisted state, no migration. The cache + lockfile stay **read-only** to this crate (PKG owns writes). It is a **breaking change to `Resolver::new`** and the `meow-cache:` URL form — but both are internal (one run-path call site; the URL is never user-visible), so the cutover is a single-wave edit, not a deprecation. **Reversibility:** revert the `crates/loader` + `meow_pkg::hash` fences and the run-path fence; LOAD-001's stand-in resolver returns. No data to roll back (nothing is written).

Behind the flow it is additive to the user: `meow run` simply resolves *real* packages now instead of the single pre-seeded blob.

## Operator notes

Integration points (named):
- **LOAD-001:** `meow_loader::Resolver`/`MeowModuleLoader` are extended in place; `ModuleLocator`/`ModuleKind`/`ResolveError`/`url.rs` evolve (see Interface). The `bare` HashMap is removed.
- **PKG-001:** `Cache::read(&ContentHash) -> Result<Vec<u8>, CacheError>` (verifies hash, I-7), `Lockfile::get(&PackageName, &Version) -> Option<&LockEntry>`, `LockEntry.{integrity, dependencies}`, `ContentHash`. **Ask:** add `ContentHash::{to_url_host, from_url_host}` to `crates/pkg/src/hash.rs` (lower-hex host round-trip) — small additive helper for the URL authority; flagged for Main to place in PKG vs. a local loader helper.
- **GRAPH-001:** `GraphDb::{intern_file, set_file, runtime_ir}` — unchanged; the loader still feeds text in and pulls IR out (no `oxc_parser`, I-1).

Assumptions / forks (local defaults — flagged for Main, not baked into `decisions.json`):
1. **Cached package blob format = npm gzip tarball** (members under a `package/` prefix), decoded in-memory by `PackageFs`. This is the central LOAD-003↔PKG-002 contract: PKG-002 decides what `LockEntry.integrity` addresses (whole tarball vs. an unpacked per-file manifest). `PackageFs::from_archive` is the single seam — if PKG-002 chooses a manifest model, only that constructor changes; the resolution algorithm is untouched. **Needs Main/PKG-002 confirmation.**
2. **Active ESM condition set = `["meow", "import", "node", "default"]`**, `"require"` excluded (CJS context, LOAD-004). Faithful to Node's import-context defaults plus a `meow` condition for runtime-specific entry points. Reversible (internal constant).
3. **Extension + index probing applies to relative + legacy-directory resolution but NOT to `exports`/`imports` targets** — a deliberate, documented *superset* of strict Node ESM (matches `tsc nodenext`/bundler ergonomics). **Honest boundary (I-11):** docs must say "Node resolution semantics for exports/imports/conditions + Node's directory resolution, with extensionless relative specifiers supported as a meow ergonomic" — not "byte-for-byte identical to Node ESM."
4. **`root_deps`** (project direct deps, pinned) source = `meow install` output / `meow.config.ts` projection. If a P1 root-entry mechanism isn't finalized, the resolver accepts the map from the run path; producing it is the caller's job (PKG/CFG).

**Honest algorithm coverage vs. full Node** (report): COVERED — `exports`/`imports` maps, conditions (priority + insertion-order nesting), subpath exact keys + `*` patterns (longest match), self-reference, fallback arrays, `null` blocking, `main`/`type`, ESM_FILE_FORMAT, legacy directory+extension+index, nested/multi-version via lockfile. GAPS (honest, by design): no registry/install (PKG-002), no CJS execution/wrapping (LOAD-004 — LOAD-003 classifies + boundary-errors), no `legacy`-mode CJS quirks or dynamic `require(var)`, no VFS/materialize, single hash algo (sha256, per PKG-001). The extensionless-relative support is a documented superset, not a Node-fidelity gap.
