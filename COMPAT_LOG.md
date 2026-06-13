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
