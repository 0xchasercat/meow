# meow Node-compat log

Branch: feat/node-compat-perf. Test app: gocart (Next 15.5.19, App Router).
Iteration note: crates/runtime/src/js/*.js load from disk at runtime -> JS edits
need NO rebuild. Only Rust / crate edits rebuild.

## ISSUE 1 - `meow run build`: mkdir + all FS ops fail with `TypeError: invalid_argument`

### Symptom
`next build` (webpack) failed: `TypeError: invalid_argument` at
`Object.mkdir (ext:deno_fs/30_fs.js)`, then `Module not found: ... package.json
(directory description file): TypeError: invalid_argument` for async readFile.

### Root cause (ROOT, systemic)
deno_core (00_infra.js) pre-registers ONLY 6 JS-builtin error classes
(Error/TypeError/RangeError/ReferenceError/SyntaxError/URIError). meow never
registered Deno's own error classes (NotFound, AlreadyExists, ...). So for any
FS op that fails with e.g. NotFound, `error.to_v8_error()` calls JS
`buildCustomError("NotFound", msg)` -> `errorMap["NotFound"]` is undefined ->
returns `undefined`.
 - SYNC ops then throw `undefined` (meow's node_globals harness already caught
   `error === undefined` for a few READ ops -> ENOENT, which is why stat/readFileSync worked).
 - ASYNC ops reject with `undefined`; deno_core's __opRejectHandler then runs
   `Error.captureStackTrace(undefined,...)`, which V8 rejects, surfacing the
   misleading `TypeError: invalid_argument`. This hit EVERY async fs error path
   (readFile, open, unlink, mkdir, rm, ...), breaking webpack's enhanced-resolve
   (treats missing package.json description files as fatal instead of ENOENT)
   and mkdirp (needs ENOENT to create parents, EEXIST to treat as done).

### Fix (JS, no rebuild) - crates/runtime/src/js/node_globals.js
1. ROOT FIX: register the Deno error classes with deno_core's errorMap, by
   explicit name (Deno.errors props are non-enumerable so Object.keys is empty)
   matching deno_error's std::io::Error get_class output (NotFound,
   AlreadyExists, PermissionDenied, ...). deno_error also attaches the errno
   `code` (ENOENT/EEXIST/...) as an additional property, so built errors carry
   the right Node code, which deno_node maps correctly. Mirrors what stock Deno
   does at startup.
2. Defensive: extended the existing fs harness with mkdir/mkdirSync errno repair
   (re-derives ENOENT/EEXIST from the filesystem if a garbage error ever slips
   through). With fix #1 this is a no-op fallback.

### Verified (reality)
Probe matrix on minted runtime: promises.readFile/open/unlink/stat/rm missing ->
code=ENOENT; mkdir missing-parent -> ENOENT; mkdir existing(noRec) -> EEXIST;
mkdir existing(recursive) -> success; rm missing(force:true) -> swallowed.

## ISSUE 2 - `meow run build`: Rust panic in node:vm (no V8 snapshot)

### Symptom
Build worker SIGABRT: panic at deno_node ops/vm.rs:726 `Option::unwrap() on
None` -> `v8::Context::from_snapshot(scope, VM_CONTEXT_INDEX).unwrap()`.
Webpack (compiled/webpack/bundle5.js) calls `vm.createContext(sandbox)` ->
op_vm_create_context -> ContextifyContext::attach, which hardcodes
ContextInitMode::UseSnapshot. meow ships no V8 startup snapshot, so
from_snapshot(0) returns None -> unwrap panics.

### Fix (deno_node patch; pending durable [patch.crates-io] vendoring)
Changed ContextifyContext::attach to use ContextInitMode::ForSnapshot (the
branch that builds a fresh contextify context WITH the global template /
interceptors via v8::Context::new) instead of UseSnapshot. This makes node:vm
work without a startup snapshot while preserving contextify semantics. Applied
in ops/vm.rs (2 occurrences inside attach). Backup at /tmp/vm.rs.bak.*.
NOTE: still independent of the Phase-2 startup snapshot; remains correct even
if a snapshot is later added.

### Verified
(pending rebuild + build re-run)

### Verified (ISSUE 2 fix)
vm panic gone; build advanced. (deno_node vendored at vendor/deno_node via
[patch.crates-io] in Cargo.toml; .rs changes there need a rebuild, but the .ts
polyfills load FROM DISK at runtime so .ts edits apply with NO rebuild.)

## ISSUE 3 - `meow install` resolves EXACT npm versions as caret (resolver bug)

### Symptom
sharp failed to load libvips: installed @img/sharp-libvips-darwin-arm64@1.3.0
(libvips 8.18.3) but sharp-darwin-arm64@0.34.5 pins exactly "1.2.4" (8.17.3).
Also next resolved to 15.5.19 despite package.json pinning "15.3.5".

### Root cause
crates/pkg/src/hash.rs normalize_version_req_arm left a bare "1.2.4" untouched,
and semver::VersionReq::parse("1.2.4") means CARET (^1.2.4) in Cargo -> matched
1.3.0. npm treats a bare "1.2.4" as EXACT (=1.2.4); "1.2" as >=1.2.0 <1.3.0; etc.

### Fix
Added normalize_bare_version(): bare x.y.z -> "=x.y.z"; x.y -> ">=x.y.0,<x.(y+1).0";
x -> ">=x.0.0,<(x+1).0.0"; wildcards/`*`/x -> ranges; operator-prefixed tokens
(^ ~ > < = |) untouched. Unit test npm_bare_version_is_exact_not_caret added
(cargo test -p meow-pkg: 42 passed).

### Verified
Fresh install: @img/sharp-libvips-darwin-arm64 -> 1.2.4 (libvips-cpp.8.17.3.dylib
on disk); next -> 15.3.5 (the pinned version).

## ISSUE 4 - `meow install` never installs peerDependencies

### Symptom
recharts (es6/util/ReactUtils.js) `Module not found: Can't resolve 'react-is'`.
recharts@3.8.1 declares react-is as a REQUIRED peerDependency; meow installed no
react-is at all (npm 7+ auto-installs non-optional peers).

### Root cause
crates/pkg had zero peerDependencies handling.

### Fix
registry.rs: parse peerDependencies + peerDependenciesMeta (PeerDependencyMeta
{optional}). install.rs: resolve non-optional peers, but DEDUPE against
already-selected versions (a `selected: BTreeMap<name,set<Version>>`) so
singletons like react are reused (never duplicated); only truly-absent peers
(react-is) are installed fresh. Best-effort: unresolvable peers are skipped.

### Verified
Fresh install: react-is 19.2.7 materialized under recharts/node_modules/react-is;
react stays single (19.2.7) - no duplicate React.

## ISSUE 5 - build worker IPC teardown crashes parent (readLoop)

### Symptom
Uncaught "Interrupted: operation canceled" at deno_node internal/child_process.ts
readLoop; build worker IPC torn down -> parent process aborts before finalizing.

### Root cause
readLoop unrefs the pending IPC read; at channel teardown deno_core cancels it
with Canceled (class Interrupted, message "operation canceled"). readLoop caught
only `instanceof Deno.errors.Interrupted/BadResource`; the canceled error slipped
through and was re-emitted as an uncaught 'error'.

### Fix (vendor/deno_node child_process.ts, loaded from disk - no rebuild)
Broadened readLoop's catch to also treat err.name Interrupted/BadResource and
message containing "operation canceled" as graceful channel teardown.

### Verified
gocart `meow run build` completes: BUILD_ID written, prerender-manifest +
routes-manifest present, prerendered HTML (index/admin/pricing/...), static
chunks, "Compiled successfully". REAL production build. PHASE 1 BUILD = DONE.

## ISSUE 6 - gocart `meow run dev` runaway memory in next-server (ARCH FORK, partial)

### Symptom
Dev (both --turbopack AND plain webpack) prints "Next.js ... / Starting..." then
the forked next-server child grows memory unbounded and V8-OOMs ("Fatal
 JavaScript out of memory") at the heap ceiling, never reaching "Ready"; curl
:3000 -> HTTP 000. Same failure in both bundler modes => not turbopack-specific.

### Findings
- meow set NO V8 heap limit; V8 defaulted ~1.4GB (Node sizes to RAM). FIXED:
  crates/runtime/src/lib.rs now sets create_params heap_limits(0, 4GB) so the
  ceiling matches Node. This stopped the premature 1.4GB OOM but NOT the runaway.
- With 4GB: next-server RSS climbs 485->787->1175->1372->1687 (t=6..31s) then
  2606MB @50s, still "Starting...", still climbing -> heads to OOM ~4GB ~80s.
- `sample` of the hung child: deep recursive stat/readdir/opendir/getdirentries
  (a directory walk), plus napi-addon frames. Memory grows with the walk.
- meow's symlink fs is CORRECT: lstat types symlinks, realpath resolves to the
  global store (~/.meow/cache/unpacked/sha256-...); readdir has no `.`/`..`.
- The walk's realpaths land OUTSIDE any node_modules dir (the global content-
  addressed store), so webpack `snapshot.managedPaths` / watcher `**/node_modules/**`
  ignore heuristics don't apply -> Next/webpack deeply scans/watches meow's store.

### Root cause (architectural)
meow materializes packages as symlinks to a GLOBAL store whose realpaths contain
no `node_modules` segment (unlike pnpm's project-local node_modules/.pnpm/...).
Next dev's persistent file scanning/watching follows these into the global store
and fails to treat them as managed/ignored, traversing unboundedly -> OOM.
(`next build` is unaffected: it does one-shot, bounded module tracing.)

### Fix direction (not yet implemented - larger materialize.rs change)
Make materialized package realpaths contain a `node_modules` segment, e.g. a
pnpm-style project-local virtual store (node_modules/.meow/.../node_modules/<pkg>
as real/hardlinked content) OR relocate the unpacked store under a path with a
`node_modules` component, so webpack/watcher ignore heuristics skip it. Kept the
heap-limit fix; this is logged as an architectural fork per instructions and
other fronts (clerk, taxonomy builds, Phase 2 perf) proceeded.

## ISSUE 7 - import() of CJS exposes only `default` (no named exports) [FIXED]

### Symptom (clerk / Next 16 build)
`next/dist/bin/next` does `import('../cli/next-build.js').then(m=>m.nextBuild(...))`
and crashed: `TypeError: mod.nextBuild is not a function`. `require()` of that
file exposes nextBuild fine, but `import()` gave only {__esModule, default}.

### Root cause
meow's ESM facade for CJS gets named exports from meow's own Oxc analyzer
(crates/graph/src/cjs.rs analyze_cjs). It detected `exports.x=`,
`module.exports.x=`, `Object.defineProperty(exports,"x",..)` (Next 15 style) but
NOT Next 16's swc output, which declares names via `_export(exports, {...})` and
emits the canonical static hint `0 && (module.exports = { nextBuild: null, ... })`.
Note: the analyzer is an Oxc Visit walker (no dead-code elimination), so it DOES
visit the `0 &&` AssignmentExpression; the gap was that inspect_assignment never
read the object-literal keys off the RHS.

### Fix
cjs.rs: inspect_assignment now extracts object-literal keys from
`module.exports = { ... }` (push_object_literal_keys), covering the swc/babel
`0 && (module.exports = {...})` reexport hint (same signal cjs-module-lexer uses).
Unit test detects_module_exports_object_literal_reexport_hint added (cargo test
-p meow-graph green: the test uses the exact `0 &&` form, proving the walker
reaches it). Verified: clerk `meow run build` now resolves mod.nextBuild.

## ISSUE 8 - worker_threads (op_create_worker) unimplemented [GAP + workaround]

### Symptom (clerk / Next 16 build)
`TypeError: op_create_worker is not a function` at deno_node worker_threads.ts
(new NodeWorker), from jest-worker ExperimentalWorker -> turbopackBuildWithWorker.
Next 16 turbopack build hardcodes enableWorkerThreads:true for its build worker.

### Root cause
meow provides the node:worker_threads JS surface (deno_node) but not the host
`op_create_worker` op (deno_runtime's web-worker host: spawn a worker isolate +
MessagePort). Implementing it is a large runtime feature (worker isolates,
module loading in workers, message ports) - logged as a gap, not done here.

### Workaround (verified)
NEXT_TURBOPACK_USE_WORKER=0 makes Next run the turbopack build IN-PROCESS
(build/index.js: turbopackBuild(useWorker=false)). With that, clerk
`meow run build` COMPLETES + is reality-verified: BUILD_ID written,
prerender-manifest + routes-manifest present, server HTML/RSC chunks,
"Compiled successfully". The page-data/static-generation phases still use
jest-worker CHILD_PROCESS workers (not threads) which work via the ISSUE 5
readLoop fix. Full drop-in would need op_create_worker implemented.

## ISSUE 9 - no node_modules/.bin shims => compound scripts fail [GAP]

### Symptom (taxonomy build)
build script is `contentlayer build && next build`. meow ran it via `sh -c` and
failed: `sh: contentlayer: command not found` (EXIT 127).

### Root cause
meow creates NO node_modules/.bin/* shims (verified: .bin empty for gocart AND
taxonomy). Single-command scripts work because meow resolves the first token via
resolve_package_bin (manifest `bin` field) -- that's why gocart's bare
`next build` ran. But compound scripts (A && B, pipes, etc.) are handed to the
shell (cli.rs ~2365 Command::new("sh")), and the shell has no node_modules/.bin
on PATH, so bin names (contentlayer, etc.) don't resolve. npm/pnpm/yarn all
create .bin shims AND prepend node_modules/.bin to the script PATH.

### Fix direction (not implemented)
(1) materialize: for each installed package with a `bin` field, create
node_modules/.bin/<name> (executable shim/symlink to the bin file) including for
nested .meow/<pkg>/node_modules/.bin. (2) run: prepend the project (and ancestor)
node_modules/.bin to PATH for script execution (both the native and sh paths).
This is a broad PM correctness win (any app whose scripts call installed bins or
use && / pipes). taxonomy install itself works (1080 pkgs, 2.7GB).

---

# SUMMARY (this session)

PHASE 1 (correctness):
- gocart `meow run build`  => WORKS, reality-verified (real .next, BUILD_ID,
  prerendered HTML, manifests, "Compiled successfully").
- clerk  `meow run build`  => WORKS, reality-verified (BUILD_ID + manifests +
  HTML/RSC), with NEXT_TURBOPACK_USE_WORKER=0 (routes around the worker_threads
  gap; child_process workers used for page-data/static-gen work fine).
- taxonomy install => WORKS (1080 pkgs); build blocked by ISSUE 9 (.bin shims).
- gocart/clerk `meow run dev` => blocked by ISSUE 6 (next-server runaway memory;
  architectural, materialize symlink layout). Heap limit raised to Node-like 4GB.

Fixes landed (all committed on branch feat/node-compat-perf, tests green):
  1 Deno error-class registration (systemic: async fs ops -> real ENOENT/EEXIST)
  2 mkdir errno repair (defensive)
  3 node:vm without a V8 snapshot (vendored deno_node ForSnapshot, [patch])
  4 npm-exact semver (bare 1.2.4 = exact, not caret) + unit test
  5 peerDependencies install with dedup (react-is; react stays singleton)
  6 child_process readLoop: canceled IPC teardown treated as graceful
  7 4GB V8 heap limit (Node-comparable)
  8 CJS->ESM named exports from `0 && (module.exports={...})` hint + unit test

Remaining compat tail (documented, with fix directions): ISSUE 6 (dev runaway),
ISSUE 8 (worker_threads/op_create_worker), ISSUE 9 (.bin shims).

PHASE 2 (perf): not started (Phase 1 build/dev tail still open). Baselines
observed: install gocart ~11s / clerk ~29s / taxonomy ~ (1080 pkgs); release
LTO link ~2m40s. Recommended next: V8 startup snapshot (also de-risks vm),
install parallelism, module-resolution/fs hot-path profiling.
