---
spec_id: LOAD-004
title: Symlink tree bridge for Deno Node CommonJS
subsystem: crates/loader, crates/pkg, crates/runtime, crates/cli, crates/lsp
status: drafting
blast_radius: high
plan_ref: "PLAN.md P2.5 · Drop-In Core (Amendments 001/002/003 — adoption spine, Tier-1)"
constitution_ref: [Amendment-001, Amendment-002, Amendment-003, I-1, I-5, I-7, I-9, I-10, I-11]
depends_on: [LOAD-003, PKG-003, PKG-004, LSP-001, RT-007]
estimate: large
backend: any
---

## Motivation

LOAD-004 originally planned to translate CommonJS into synthetic ESM. That direction is superseded by Amendment 003. The observed failure mode is exactly why: Webpack/Next dynamic `require` obfuscation and CJS cycles need native Node semantics, not a static AST wrapper.

LOAD-004 now owns the bridge between meow's PnP package graph and Deno's Node resolver / require loader. Deno owns CommonJS execution. Meow owns which package/version/member is selected and materializes it into a strict pnpm-style, project-local `node_modules` symlink tree rooted in the global unpacked store.

## Acceptance criteria

### A1 — no CJS-to-ESM AST wrapper remains

The loader MUST NOT implement CommonJS by generating synthetic ESM executor/registry companions. Any existing `build_plan`, `executor_source`, static dynamic-require prewalk, or named-export synthesis machinery is removed or quarantined as obsolete.

CJS execution uses Deno's Node/CommonJS loader semantics.

### A2 — PnP selects packages; Deno sees real package folders

The bridge feeds Deno path-backed package folders selected from meow's PnP `ResolutionGraph`:

- root direct dependencies come from `package.json` + `meow.lock.jsonl` via the same graph inputs as runtime/LSP;
- transitive dependencies follow the referrer's exact locked package edge;
- each resolve step maps `(specifier, referrer)` to one exact `Cached { package, member }` graph vertex;
- package contents live in `meow_pkg::UnpackedStore` / equivalent immutable extracted cache paths, as `dir_for(package).join(member)`;
- default install exposes those contents through the strict pnpm-style project-local `node_modules` symlink tree, and Deno consumes that tree rather than copied package files.

This is the key design point: Deno's Node resolver is filesystem/path-oriented, so meow supplies stable symlink-backed real paths to Deno rather than forcing `meow-cache://` through Deno internals.

### A3 — implement the Deno resolver service bridge

Implement the service types required by `deno_node::NodeExtInitServices` using meow graph/cache inputs:

- `node_resolver::NpmPackageFolderResolver`
  - `resolve_package_folder_from_package(package_name, referrer)` returns the materialized symlink-tree package folder for the exact package/version selected by `ResolutionGraph` for this `referrer`.
  - `resolve_types_package_folder(...)` returns the matching `@types/*` symlink-tree folder when present in the graph, else `None` honestly.
- `node_resolver::InNpmPackageChecker`
  - true for URLs/paths inside the generated `node_modules` tree and their global unpacked-store targets;
  - false for first-party local files and meow-native modules.
- `deno_node::NodeRequireLoader`
  - reads text from local files or symlinked package paths backed by the unpacked cache;
  - reports maybe-CJS vs ESM by consulting nearest package.json / extension in the exposed package folder;
  - returns real paths for `__filename` / `__dirname`, including `member` paths derived from the selected `Cached` locator.
- `node_resolver::PackageJsonResolver` / `NodeResolverSys`
  - use a filesystem view that can read local project files, generated `node_modules` links, and unpacked-store targets.

Exact type names/generics may follow the upstream crate, but all selection decisions must trace back to the same `ResolutionGraph`.

### A4 — native require handles cycles and dynamic require

The following run without meow-specific CJS wrappers:

- local `.cjs` entry;
- local `.js` under absent / `"type":"commonjs"` package.json;
- cached dependency CJS selected by `main`, `exports.require`, extensionless, or directory index;
- dynamic `require(expr)` in Webpack/Next-style code when Node would resolve it;
- circular CJS graphs observe partial `module.exports` rather than ESM TDZ failures.

### A5 — ESM/CJS interop follows Deno/Node behavior

Interop semantics are delegated to Deno Node. Meow must not promise custom named-export synthesis beyond what Deno/Node support. If a real project depends on an unsupported edge, the gap is labelled as Deno-upstream, meow-bridge, or Node-bug-parity residual.

### A6 — path identity is honest

- Local modules expose their real local path.
- Cached packages expose stable paths in the materialized `node_modules` symlink tree, whose leaves point to global unpacked-store folders.
- `meow-cache://` remains allowed for diagnostics/internal identity, but Deno Node and N-API paths come from the real symlinked filesystem view.
- No synthetic `node_modules` pseudo-paths are surfaced as resolver outputs.
- runtime/tooling must report real filesystem paths for resolved package files.

### A7 — LSP/runtime parity is graph-selection parity
After Amendment 003, parity means runtime, LSP, and node tooling select the same package/version/member from the same `ResolutionGraph` and runtime resolves through that same graph-backed materialized `node_modules` view. Deno owns detailed Node resolver behavior inside the runtime bridge. `.meow/deps` remains a tooling projection for editors/TS/foreign tools only.

## Non-goals

- A meow-owned CommonJS parser/executor.
- Static CJS named-export synthesis owned by meow.
- Forcing every internal identity to be `meow-cache://`.
- Copying entire `node_modules` contents into project tree by default.
- N-API `.node` loading; RT-008 owns the native addon loader, though it consumes the same unpacked-store path seam.
- Full bug-for-bug parity claims before the drop-in corpus proves them.

## Interface

Add a compatibility bridge crate/module surface, conceptually:

```rust
pub struct DenoNodeSymlinkBridge {
    pub graph: Arc<ResolutionGraph>,
    pub cache: Arc<Cache>,
    pub unpacked: UnpackedStore,
    pub project_root: PathBuf,
    pub project_node_modules: PathBuf,
}

impl node_resolver::NpmPackageFolderResolver for DenoNodeSymlinkBridge { ... }
impl node_resolver::InNpmPackageChecker for DenoNodeSymlinkBridge { ... }
impl deno_node::NodeRequireLoader for DenoNodeSymlinkBridge { ... }
```

The bridge is constructed at the binary/runtime edge from the same objects already used to build the meow loader and LSP resolver.

## Implementation sketch

1. Remove obsolete synthetic CJS wrapper code from loader/runtime docs and implementation.
2. Introduce `DenoNodeSymlinkBridge` over `ResolutionGraph`, `Cache`, `UnpackedStore`, and the project `node_modules` tree.
3. Map first-party, generated-node_modules, and unpacked-store referrers back to graph package identity.
4. Return real symlink-tree package folder paths for Deno's resolver, with symlink leaves pointing into `UnpackedStore::ensure(...)`.
5. Register the resulting `NodeExtInitServices` in RT-007's Deno Node extension assembly.
6. Keep `meow_loader::Resolver` for ESM/local/package graph selection where it remains the right abstraction; do not duplicate package selection logic.

## Tests required

- first-party `.cjs` entry runs;
- dependency CJS runs from the cache/unpacked store and the generated project-local `node_modules` symlink tree;
- dynamic `require(expr)` resolves in a Webpack-style fixture;
- circular CJS fixture does not throw TDZ and observes partial exports;
- `require("fs")` / `require("node:fs")` use Deno Node built-ins;
- package `exports.require` branch is selected correctly;
- `__dirname` / `__filename` point at local or materialized symlink-tree real paths;
- runtime and LSP agree on package/version/member selection for the same graph;
- no stale synthetic-wrapper symbols remain.

Gates: `drop-in`, `compat`, `resolver-parity`, `lockfile-integrity`, `honesty`, `footprint`.

## Rollout
LOAD-004 should land after RT-007 has registered Deno Node built-ins. It replaces temporary native-CJS, path, and filesystem shims with Deno-owned behavior; any temporary debt must be removed before declaring parity and only accepted with explicit exception notes.
