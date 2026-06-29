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