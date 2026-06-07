---
spec_id: LOAD-001
title: "ESM loader + content-addressed cache read (single dependency)"
subsystem: crates/loader
status: merged
blast_radius: medium
plan_ref: "Initial spec backlog · Wave 1 (LOAD-001); Build sequence · P0 Foundations"
constitution_ref: [I-1, I-5]
depends_on: [GRAPH-001, PKG-001]
estimate: medium
backend: omp
---

## Motivation

`meow` runs ESM with **no `node_modules`** (ADR-4, I-5): the module resolver intercepts lookups in-memory and points at the global content-addressed cache. This spec stands up `crates/loader` — the subsystem CANON §6.2 calls the *Module loader* — as a `deno_core::ModuleLoader` that the RT-001 runtime plugs in. It hosts **THE single resolver** (I-1 "one parse, one graph"; I-5 "one resolver"): the same `Resolver` the LSP will later consume (§20, ADR-4 — "the runtime and the language server must share identical resolution logic, or the editor and the runtime will disagree"). It is the P0 exit condition *"one dependency resolves from the content-addressed cache."*

Scope is deliberately the **minimum** that proves the spine end-to-end: resolve local relative/absolute ESM specifiers **and** one bare specifier whose bytes already live in the PKG-001 cache, read those bytes **by content hash**, hand the source to GRAPH-001 for parse + type-erase (stage 5 runtime IR; RT-003 does the strip inside the graph), and return the module to V8. The full Node resolution algorithm is LOAD-003 (P1) — see Non-goals.

Traceability: `plan_ref` → PLAN.md *Initial spec backlog · Wave 1* (`LOAD-001`) + *Build sequence · P0 Foundations* (exit: "one dependency resolves from the content-addressed cache"). `constitution_ref` → **I-1** [`graph-integrity`] (no second parser/resolver; loader feeds the shared graph, never `oxc_parser::Parser::new`), **I-5** [`resolver-parity`] (no `node_modules`; one resolver shared with the LSP).

## Acceptance criteria

Concrete, checkable Rust. New crate `crates/loader` (crate name `meow_loader`). All user-triggerable paths return `Result` — the runtime never panics on user input (CRAFT.md). Errors are typed + causal and point at the fix.

### `crates/loader/src/resolver.rs` — THE single resolver (I-1, I-5)

```rust
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;
use pkg::{Cache, ContentHash, Lockfile};   // re-exported from crates/pkg — depend, never redefine

/// The one module resolver for the whole toolchain. The runtime's `ModuleLoader`
/// and (later, LSP-001) the language server both consume this exact type — there is
/// no second resolver anywhere (I-1, I-5).
pub struct Resolver {
    cache: Arc<Cache>,
    lockfile: Arc<Lockfile>,
    /// `file://` base for first-party (local) resolution; also the default referrer
    /// for the entry/main module.
    project_root: Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    /// LOAD-001 handles ESM only. CJS wrapping is LOAD-004 (P5); first-party CJS
    /// rejection is LOAD-002 (P1). See Non-goals.
    Esm,
}

/// *Where* a resolved module's bytes live — the result of pure resolution, before any
/// source I/O. Keeps the resolution algorithm separable from reading (so dedup never
/// reads bytes twice, and the LSP can resolve without loading).
#[derive(Debug, Clone)]
pub enum ModuleLocator {
    /// First-party file on disk, addressed by its `file://` URL.
    LocalFile(PathBuf),
    /// Cached dependency, addressed by content hash in the PKG-001 cache (no `node_modules`).
    Cached(ContentHash),
}

/// A fully resolved **and read** module: the contract returned to the loader and to
/// every non-deno_core consumer (tests, future LSP).
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub url: Url,
    pub source: Arc<str>,   // matches GraphDb::set_file(FileId, Arc<str>)
    pub kind: ModuleKind,
}

impl Resolver {
    pub fn new(cache: Arc<Cache>, lockfile: Arc<Lockfile>, project_root: Url) -> Self;

    /// THE resolution entrypoint (the name the assignment + LSP-001 target). Resolves
    /// `specifier` against `referrer`, reads source from disk (local) or the
    /// content-addressed cache (bare), and returns it. An already-absolute specifier
    /// (`file://` / `meow-cache://`) short-circuits `locate` and reads directly — this is
    /// how `ModuleLoader::load` re-enters the single resolver with deno_core's pre-resolved URL.
    pub fn resolve(&self, specifier: &str, referrer: &Url) -> Result<ResolvedModule, ResolveError>;

    /// Pure resolution: `specifier` + `referrer` → resolved URL + locator, **no source I/O**.
    /// `resolve()` calls this then reads the locator; deno_core's sync `resolve` calls this
    /// directly (URL dedup must not read bytes). The single resolution algorithm lives here.
    pub fn locate(&self, specifier: &str, referrer: &Url) -> Result<(Url, ModuleLocator), ResolveError>;

    pub fn project_root(&self) -> &Url;
}
```

Resolution algorithm (LOAD-001 minimal — `locate`):
1. If `specifier` is relative (`./`, `../`) or rooted (`/`) or a `file://`/`meow-cache://` URL → `Url::options().base_url(Some(referrer)).parse(specifier)`. `file://` → `LocalFile(url.to_file_path())`; `meow-cache://` → decode SRI → `Cached(hash)`.
2. Otherwise it is a **bare** specifier (`name` or `name/subpath`). Split off the package name; look it up in the lockfile. For the single P0 dependency, select the sole `LockEntry` for that name, take `entry.integrity: ContentHash`, mint a deterministic virtual URL `meow-cache://<sri>` (see `url.rs`) → `Cached(entry.integrity.clone())`. No disk walk, no `node_modules`. Missing entry → `ResolveError::BareSpecifierNotInLockfile` (fix: run `meow install`). Multi-version / range selection is LOAD-003.

Reading (`resolve`): `LocalFile(p)` → `tokio::fs`/`std::fs` read + `String::from_utf8` (errs `ResolveError::Io { path, source }`); `Cached(h)` → `cache.read(&h)?` (PKG-001 recomputes + verifies the hash **before** returning bytes — integrity is PKG's job, I-7; the loader propagates `CacheError`, never swallows it).

```rust
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("could not resolve {specifier:?} from {referrer}")]
    SpecifierNotFound { specifier: String, referrer: Url },
    #[error("bare specifier {name:?} is not in meow.lock.jsonl — run `meow install`")]
    BareSpecifierNotInLockfile { name: String },
    #[error("reading {path}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
    #[error("module source is not valid UTF-8: {url}")]
    NotUtf8 { url: Url },
    #[error("malformed virtual cache URL: {0}")]
    InvalidVirtualUrl(Url),
    #[error(transparent)]
    Cache(#[from] pkg::CacheError),
}
```

### `crates/loader/src/url.rs` — virtual cache URL (deterministic, host-path-free)

```rust
/// Encode a content hash as a stable virtual module URL `meow-cache://<sri>` where
/// `<sri>` is the SRI form (`sha256-<base64>`). Deterministic across machines (depends
/// only on content), never leaks `$HOME` — protects I-6 module identity. Avoids the
/// reserved `meow:` native-API namespace.
pub fn encode(hash: &pkg::ContentHash) -> Url;
pub fn decode(url: &Url) -> Result<pkg::ContentHash, ResolveError>;
```

### `crates/loader/src/lib.rs` — the `ModuleLoader` impl (resolve → cache-read → graph-parse)

```rust
use std::cell::RefCell;
use std::rc::Rc;
use deno_core::{
    ModuleLoader, ModuleLoadResponse, ModuleSource, ModuleSourceCode, ModuleSpecifier,
    ModuleType, RequestedModuleType, ResolutionKind,
};
use graph::{FileId, GraphDb};
use crate::resolver::{ModuleKind, Resolver};

pub struct MeowModuleLoader {
    resolver: Resolver,
    /// The shared graph (I-1). The loader NEVER constructs a parser; it feeds text in and
    /// pulls the memoized runtime IR back out.
    graph: Rc<RefCell<GraphDb>>,
}

impl MeowModuleLoader {
    pub fn new(resolver: Resolver, graph: Rc<RefCell<GraphDb>>) -> Self;
}

impl ModuleLoader for MeowModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, deno_core::error::ModuleLoaderError> {
        let referrer = parse_referrer(referrer, self.resolver.project_root());
        let (url, _locator) = self.resolver.locate(specifier, &referrer)?;
        Ok(url)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
        _requested_module_type: RequestedModuleType,
    ) -> ModuleLoadResponse {
        let referrer = maybe_referrer
            .cloned()
            .unwrap_or_else(|| self.resolver.project_root().clone());
        // re-enter the single resolver with the already-resolved (absolute) URL → reads source
        let resolved = match self.resolver.resolve(module_specifier.as_str(), &referrer) {
            Ok(m) => m,
            Err(e) => return ModuleLoadResponse::Sync(Err(e.into())),
        };
        debug_assert_eq!(resolved.kind, ModuleKind::Esm);

        // hand source to the SHARED graph; pull the memoized, type-erased, V8-ready IR (RT-003
        // strip happens inside graph). No oxc_parser here (P15 / I-1).
        let mut db = self.graph.borrow_mut();
        let fid: FileId = db.intern_file(&resolved.url);
        db.set_file(fid, resolved.source.clone());
        let code = match db.runtime_ir(fid) {
            Ok(ir) => ir.code().to_string(),   // erasable-only, source positions preserved (RT-003)
            Err(e) => return ModuleLoadResponse::Sync(Err(graph_err(e))),
        };

        let source = ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            module_specifier,
            None, // code_cache
        );
        ModuleLoadResponse::Sync(Ok(source))
    }
}
```

Acceptance behaviors:
- A local ESM `main.ts` importing one bare specifier resolved from the cache **runs to completion** through `meow_runtime::Runtime` with `MeowModuleLoader` installed, and the dependency's exported behavior is observable.
- Both deno_core `resolve` and `load` route through the one `Resolver` (`locate`/`resolve`); there is exactly one `Resolver` type and one resolution algorithm in the binary.
- No `node_modules` directory is created or consulted anywhere on the run.

## Non-goals

- **Full Node resolution algorithm** — conditions, `exports`/`imports` maps, self-references, `package.json` walks, multi-version/range selection → **LOAD-003 (P1)** (ADR-4, CANON §12.2). LOAD-001 is local files + one pre-populated cached dependency only.
- **CJS interop** — first-party CJS rejection is **LOAD-002 (P1, I-2)**; CJS→ESM synthetic wrappers (`cjs-module-lexer`) are **LOAD-004 (P5)**.
- **Install / lockfile generation / npm registry / integrity-write** — **PKG-002+ (P2)**. LOAD-001 only *reads* an already-populated cache + lockfile.
- **VFS / `--materialize` / `--vendor` modes** — **PKG-003 / PKG-004 (P2)**. PnP in-memory only.
- **Async/streaming module load + `io_uring`** — folds in with RT-002; LOAD-001 reads synchronously for P0 (`ModuleLoadResponse::Sync`).
- **Dynamic `import()` graph edges, HMR, multi-file packages with internal subpaths** — later (one hash = one module's bytes here).
- **LSP shadow `node_modules` symlink map (§20.1)** — **LSP-001**. LOAD-001 only guarantees the `Resolver` *is* the shared entrypoint LSP-001 will construct.

## Interface

The contract is the public surface above:
- `meow_loader::Resolver` with `new`, `resolve(&str, &Url) -> Result<ResolvedModule, ResolveError>`, `locate(&str, &Url) -> Result<(Url, ModuleLocator), ResolveError>`, `project_root`.
- `meow_loader::{ResolvedModule, ModuleKind, ModuleLocator, ResolveError}`.
- `meow_loader::MeowModuleLoader::new(Resolver, Rc<RefCell<GraphDb>>)` implementing `deno_core::ModuleLoader`.
- Virtual URL scheme `meow-cache://<sri>` for cached modules; `file://` for first-party.
- Consumes (do not redefine): `pkg::{Cache, ContentHash, Lockfile, LockEntry, CacheError}`, `graph::{GraphDb, FileId, RuntimeIr, GraphError}`, `deno_core::ModuleLoader`.

No CLI surface of its own; surfaced via `meow run` once the run path wires `MeowModuleLoader` into `RuntimeOptions.module_loader`.

## Implementation sketch

New crate `crates/loader` (workspace member). Files: `src/lib.rs` (`MeowModuleLoader` + `deno_core::ModuleLoader` impl), `src/resolver.rs` (`Resolver` + types + `ResolveError`), `src/url.rs` (virtual scheme), `tests/load_cached_dep.rs`. Deps: `deno_core`, `url`, `pkg`, `graph`, `thiserror`, `tokio` (for fs in P0; sync read acceptable).

Shared-file edits (comment-marker fences):
- Root `Cargo.toml` workspace `members` — add `"crates/loader"` behind `// === LOAD-001 ===`.
- Run path (CLI / `meow run`) where `meow_runtime::Runtime` is constructed — build `Rc::new(MeowModuleLoader::new(resolver, graph))` and set `RuntimeOptions.module_loader = Some(loader)` behind `// === LOAD-001 ===` (this overrides RT-001's default `TrivialEsmLoader`, which lives behind RT-001's `// === RT-001 ===` fence in `Runtime::new`).

Flow: deno_core calls `resolve` (→ `Resolver::locate`, URL only, dedup-safe) then `load` (→ `Resolver::resolve` re-entered with the absolute URL → cache-read/disk-read → `GraphDb::set_file` + `runtime_ir` → `ModuleSource`). Single resolver, single graph; the loader owns neither a parser nor a second resolution path.

## Tests required

`crates/loader/tests/load_cached_dep.rs` (integration) + unit tests in `resolver.rs`/`url.rs`. Proves **I-1** [`graph-integrity`] and **I-5** [`resolver-parity`].

- **End-to-end cached-dep run (I-5, I-1):** temp project dir; `Cache::store(dep_bytes)` → `ContentHash`; write a `meow.lock.jsonl` line with that integrity for the dep; `main.ts` imports the bare dep and calls its export; build `Resolver` + `GraphDb` + `MeowModuleLoader`, install into `meow_runtime::RuntimeOptions.module_loader`, run the main module; **assert the dep's exported behavior is observed** (e.g. captured output / returned value).
- **No `node_modules` (I-5, `resolver-parity`):** after the run, assert `!project_root.join("node_modules").exists()`; assert no path containing `node_modules` was opened (the cached module's `url` is `meow-cache://…`, never a `node_modules` path).
- **Single resolver entrypoint (I-1, `graph-integrity`):** assert both a relative (`./a.ts`) and the bare specifier resolve through `Resolver::locate`/`resolve` (one type, one algorithm); structurally, the only `deno_core::ModuleLoader` impl delegates to `Resolver` and the loader never references `oxc_parser` (loaded source goes through `GraphDb`).
- **Resolution unit tests:** `./a.ts` from `file:///proj/main.ts` → `file:///proj/a.ts`; bare specifier absent from lockfile → `ResolveError::BareSpecifierNotInLockfile` (not a disk walk); `url::{encode,decode}` round-trips a `ContentHash`; malformed `meow-cache://` → `InvalidVirtualUrl`.
- **Integrity propagation:** tampered cache bytes → `Cache::read` returns `IntegrityMismatch` → loader surfaces `ResolveError::Cache` (never swallowed). (PKG-001 owns the `lockfile-integrity` gate; here we only assert the loader propagates.)
- **Graph integration (I-1):** a `.ts` module with a type annotation loads, is type-erased by the graph (RT-003), and runs — asserting the loader feeds the graph rather than executing raw `.ts`.

GATES: `resolver-parity` (I-5), `graph-integrity` (I-1).

## Rollout

Additive: a new crate plus two fenced edits. The default runtime path keeps RT-001's `TrivialEsmLoader` until the run path sets `RuntimeOptions.module_loader = Some(MeowModuleLoader)`. No persisted state, no migration; the cache + lockfile are **read-only** to this crate (PKG owns all writes). **Reversibility:** revert the run-path fence (pass `None`) → runtime falls back to the trivial loader; remove the `crates/loader` member line. No data to roll back.

## Operator notes

Integration points (named, confirmed with the dependency drafters):
- **RT-001 (RT001):** `meow_runtime::RuntimeOptions { module_loader: Option<Rc<dyn deno_core::ModuleLoader>>, .. }` in `crates/runtime/src/lib.rs`; default `TrivialEsmLoader`-when-`None` lives behind `// === RT-001 ===` in `Runtime::new`. LOAD-001 supplies `Some(Rc::new(MeowModuleLoader::new(..)))`. Plain options struct, no Builder.
- **PKG-001 (PKG001):** `pkg::Cache::read(&ContentHash) -> Result<Vec<u8>, CacheError>` (recomputes + verifies hash **before** returning bytes, I-7; `IntegrityMismatch` on tamper) and `pkg::Cache::open(&Path)`; `pkg::Lockfile::get(&PackageName, &Version) -> Option<&LockEntry>` with `LockEntry.integrity: ContentHash`. `ContentHash` is re-exported from `crates/pkg` — **depend on it, do not redefine** (`Display` = SRI `sha256-<base64>`).
- **GRAPH-001 (GRAPH001):** `graph::GraphDb::intern_file(&Url) -> FileId`, `set_file(FileId, Arc<str>)`, `runtime_ir(FileId) -> Result<&RuntimeIr, GraphError>` (memoized; `RuntimeIr` = type-erased, V8-ready text + spans). The loader **must not** call `oxc_parser::Parser::new` (P15 / I-1) — feed text in, pull IR out.

Assumptions / forks (CANON resolved most; these are local defaults — flagged for Main, not baked into `decisions.json`):
1. **Virtual URL scheme for cached modules** = `meow-cache://<sri>` (deterministic, content-only, host-path-free). Chosen over `file://` cache paths, which would leak `$HOME` into module identity and threaten I-6 determinism, and over the reserved `meow:` native-API namespace. Reasonable default; reversible (scheme is internal).
2. **Bare-specifier → version selection** for the single P0 dependency: resolve the sole `LockEntry` for the package name. PKG-001's public `Lockfile::get` is keyed by `(name, version)`; the name-only selection used here may want a tiny `Lockfile` name→entry helper from PKG-001. Flagged for Main to consolidate; full multi-version/range selection is LOAD-003 regardless.
3. **Synchronous reads** (`ModuleLoadResponse::Sync`) for P0; async/`io_uring` module loading folds in with RT-002 wiring — not a contract change to the `Resolver`.
