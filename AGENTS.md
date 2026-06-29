# 🐾 meow: Agent Architecture & Contribution Guidelines

This is the `meow` repository. `meow` is a drop-in replacement for Node.js, an ultra-fast package manager, a testing framework, and a unified toolchain, delivered as a single Rust binary.

It is built on the **"Floof & Teeth"** duality:
*   **The Floof:** Best-in-class, adorable, empathetic Terminal UX (Bento boxes, paw spinners, high-fidelity diagnostics).
*   **The Teeth:** Ruthless, zero-overhead systems engineering (Oxc, V8 snapshots, APFS cloning, Tokio parallelism).

This document defines the physical laws of the `meow` architecture. If you are an AI agent or contributor writing code for this repository, you must obey these invariants.

---

## 1. The Zero-Compromise Engineering Philosophy

Before you generate a single line of code, you must pass your proposed solution through this internal checklist:

1. **Does this create technical debt, or is it a shortcut just to get it to work now?** 
   If yes, it is an immediate violation. We do not ship half-baked features. If doing it the "right" way (e.g., integrating native Rolldown vs. doing naive string concatenation) takes longer, we take the longer path. 
2. **Does this compromise correctness to make a test pass?** 
   If yes, it is an immediate violation. We do not write hacks to satisfy broken tests. If a test is failing, fix the underlying JavaScript or Rust logic. We test against reality.
3. **Does this sacrifice fundamentals to make a benchmark look good?** 
   If yes, it is an immediate violation. We do not drop cryptographic supply-chain checks (SHA-512) to win cold-install speed benchmarks. We win benchmarks through superior systems engineering (SIMD, OS thread pooling, kernel syscalls).
4. **The Prime Directive: Amputate, Do Not Medicate.** 
   If a legacy system, upstream crate, or abstraction is fundamentally flawed, do not patch over it with `if` statements or `// TODO` comments. Rip out the root cause and replace it with a structurally sound, Rust-native implementation.

---

## 2. Architectural Invariants

### The "Parse-Once" Oxc Pipeline
*   `meow` uses `oxc` for all parsing, semantic analysis, TypeScript type-stripping, and JSX transformation. 
*   **SWC and `deno_ast` are strictly forbidden in this codebase.** If you need to transform or parse code, you use `oxc_allocator`, `oxc_parser`, `oxc_transformer`, and `oxc_codegen`. 
*   We do not emit sourcemaps or downlevel modern JavaScript. We strip TypeScript annotations in-place.

### Package Management & Materialization
*   **No Symlinks for Packages:** We do not use symlinks to materialize `node_modules` packages, as this breaks V8 and Vite's `fs.realpath` resolution.
*   **The APFS / Hardlink Strategy:** Packages are materialized into project-local `node_modules` using the macOS `clonefile(2)` kernel syscall for O(1) cloning. On Linux/Windows, we fall back to recursive hardlinking. 
*   Dependency *edges* (the pointers inside a package's `node_modules` linking to another package) remain symlinks/junctions.

### Cryptography and Network I/O
*   **No Network Starvation:** Downloading tarballs and fetching metadata is network-I/O bound (Tokio async). Decompressing tarballs (`zlib-ng`) and validating SHA-512 integrity is CPU-bound. 
*   **Strict Offloading:** You must NEVER run SHA-512 hashing or tarball decompression on the Tokio async executor thread. Always wrap heavy CPU tasks in `tokio::task::spawn_blocking` to keep the network saturated.
*   **Deterministic URLs:** Do not trust `dist.tarball` URLs from the NPM registry. Always construct tarball URLs mathematically from the package name and version to prevent cache poisoning.

### The EMFILE Shield
*   Synchronous file operations in the JS ecosystem (e.g., Vite dependency optimization) will crash the OS with `EMFILE` (Too many open files).
*   The runtime mitigates this natively. If you add new host file I/O operations, they must acquire a permit from the global `FS_SEMAPHORE` in `crates/runtime/src/io/backend.rs` to backpressure the OS.

### Hermetic by Default
*   Tests and bare `meow run` executions are mathematically deterministic. 
*   The system clock is frozen, the RNG is seeded with ChaCha20, and the OS environment is hidden.
*   **Zero-FFI Proxies:** Do not pay the FFI tax for `Math.random` or `Date.now` if the user has explicitly bypassed security (e.g., via `--trust`). Check `op_hermetic_status` once at initialization, and route to the native V8 C++ intrinsics if the cage is open.

### Fragile Runtime, Loader, and Toolchain Seams
These are not suggestions. They are scars from real regressions.

*   **Snapshot safety is sacred.** Code in `crates/runtime/src/js/node_globals.js` runs both at V8 snapshot-build time and at real runtime. During snapshot warmup, do not touch lazy Node getters or load heavy/lazy Deno Node modules. In particular, do not eagerly call `core.loadExtScript("ext:deno_node/child_process.ts")`, and do not touch `process.stdout` / `process.stderr` while the snapshot placeholder process is active. Put runtime-only patches inside `globalThis.__meowRuntimeBootstrap` after `meowRunNodeBootstrap(info, false)` has installed real process state.
*   **Deno Node lazy modules must stay lazy.** `child_process`, crypto-heavy modules, and similar Deno Node polyfills are deliberately lazified by upstream. Loading them during snapshotting can recurse through process/body/bootstrap internals and fail with `RangeError: Maximum call stack size exceeded`. If you need child-process behavior, prefer host-side env/shim changes or runtime-only hooks after bootstrap.
*   **Process exit sentinels are internal control flow.** Objects like `{ "__meowProcessExit": true, "code": 1 }` must never appear in user stdout/stderr. The runtime should record the exit code and return it through the host. If a sentinel reaches output plumbing, suppress it there; do not serialize it for users.
*   **Node shim inheritance is a sharp edge.** `MEOW_NODE_SHIM=1` exists so `node` can enter meow when appropriate, but recursively forcing every child CLI back through meow breaks tools like Next.js, Vite, Prisma, Turbo, and Jest workers. Do not globally set bypass flags in the main runtime environment unless you have tested `child_process.fork` IPC workers. The generated shim may honor an explicit `MEOW_NO_SHIM=1`, but `meow run` must not blindly inject it into every app.
*   **TypeScript ESM imports require source fallback.** Modern TS projects often write `import "./foo.js"` while only `foo.ts`, `foo.tsx`, or `foo.mts` exists on disk. The resolver must transparently fall back from explicit JS extensions to TS source siblings before declaring module-not-found. Keep this in `meow-loader::Resolver`, not as a CLI or bundler-only trick.
*   **Rolldown and Oxc are one dependency generation.** Rolldown uses Oxc internally. If you add or upgrade Rolldown, align all workspace Oxc pins and vendored Oxc pins to Rolldown's generation, then update all API call sites (`errors` vs `diagnostics`, label shapes, hook trait requirements, etc.). Never allow duplicate Oxc AST generations in the dependency graph.
*   **Rolldown plugins must register hook usage.** The Rust `rolldown_plugin::Plugin` trait requires `register_hook_usage`; if a plugin implements `resolve_id` or `load`, include `HookUsage::ResolveId | HookUsage::Load`. Without this, the hooks may not run or the crate may not compile.
*   **`meow bundle` must use the meow resolver.** The bundler is not allowed to resolve with a separate Node algorithm. Rolldown resolution goes through `MeowResolverPlugin`, which delegates to `meow_loader::Resolver`, preserving the APFS `node_modules/.meow` graph and locking out phantom dependencies.
*   **CLI split ownership.** `crates/cli/src/cli.rs` intentionally owns clap definitions, `Command`, `normalize_argv`, shared UI helpers, and project-root helpers. Command behavior lives under `crates/cli/src/cli/commands/*.rs`. Do not move behavior back into `cli.rs`; do not add command-specific logic there unless it is truly parser/router/shared-context logic.
*   **Project root detection must not leak upward.** `find_project_root` must stop at the nearest independent package boundary (`meow.lock.jsonl`, non-dependency `package.json`, or `.git`). Running inside nested examples/apps must not climb into unrelated parent package manifests and inherit their dependencies.
*   **Framework lockfile compatibility is a boundary signal, not dependency authority.** `meow.lock.jsonl` remains the authoritative lockfile. Any `package-lock.json`/framework detector shim is only compatibility metadata for tools like Next.js/Turbopack; do not make npm lockfiles authoritative for resolution.

---

## 3. Development & Testing

### Building and Running
*   **Build:** `cargo build` (or `cargo build --release` for benchmarking).
*   **Run Development CLI:** `~/meow/target/debug/meow <command>`
*   **Fast Iteration:** Generating the V8 snapshot takes ~10 seconds. If you are iterating exclusively on JavaScript polyfills/shims, use `meow run dev --no-snapshot` to bypass the snapshot generation and evaluate the JS fresh.

### The Omni-Router
*   `meow` relies heavily on muscle memory. `normalize_argv` in `cli.rs` automatically routes implicit commands.
*   If a user types `meow build`, the router catches it and injects `meow run build`. Do not add redundant commands for framework-specific scripts.

### Testing Rules
*   **No `TrivialModuleLoader`:** The runtime tests must prove the actual engine boundaries. Use the real `MeowModuleLoader` for all tests in `crates/runtime/tests/`.
*   **Mock the Graph:** For localized runtime tests, instantiate a mock `Cache`, `Lockfile`, and `ResolutionGraph` to allow the loader to resolve local `file://` URLs.
*   **Test on macOS and Linux:** Be aware that `tmpfs` mounts on Linux `/tmp` directories will cause `EXDEV` errors for hardlinks. Run materialization benchmarks in user home directories.
*   **Cross-platform support:** We support all major platforms and operating systems, all tests should take that into account and pass remote CI/CD.
---

## 4. Code Style & The "Floof"

### Error Handling (CRAFT Part B)
*   **No Panics:** `unwrap()`, `expect()`, and `panic!` are strictly reserved for unrecoverable internal state violations. 
*   Any error reachable via user input, network payload, or JS execution MUST be captured in a `thiserror` enum and surfaced gracefully.

### Terminal UI (`meow-ui`)
*   Never use raw `println!` or `eprintln!` for CLI output.
*   Use the `meow-ui` facade:
    *   `purr("message")` for success, `hiss("message")` for errors, `pounce("message")` for WIP/progress.
*   Diagnostic errors must be rendered using `meow_ui::diagnostic::SourceDiagnostic` to provide compiler-grade, red-underlined code snippets.
*   **Do not duplicate envelopes.** If a diagnostic is already rendered via `meow-ui`, do not wrap it in another `hiss()`.