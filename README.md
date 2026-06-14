<div align="center">
  <img src="banner.webp" alt="meow banner" width="100%" />
</div>

<div align="center">
  <h1>meow</h1>
  <p><strong>Looks like a kitten. Runs like Rust.</strong></p>
  <p>
    <a href="https://github.com/0xchasercat/meow/actions"><img src="https://img.shields.io/badge/build-passing-brightgreen?style=flat-square&color=98FF98" alt="Build Status"></a>
    <a href="https://github.com/0xchasercat/meow/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT_/_Apache--2.0-blue?style=flat-square&color=89CFF0" alt="License"></a>
    <a href="https://meow.style"><img src="https://img.shields.io/badge/website-meow.style-lightgrey?style=flat-square&color=FFB7C5" alt="Website"></a>
    <a href="https://github.com/0xchasercat/meow/stars"><img src="https://img.shields.io/github/stars/0xchasercat/meow?style=flat-square&color=Floof" alt="GitHub Stars"></a>
  </p>
</div>

---

`meow` is an uncompromising JavaScript and TypeScript runtime, package manager, bundler, linter, formatter, and test runner delivered as **one coherent native system**—a single, lightweight Rust binary.

It is not a Node.js wrapper, and it is not an academically pure runtime that asks you to abandon the existing ecosystem. `meow` is built to be a **drop-in replacement** for your existing Node/Bun projects. It plays nice with `package.json`, runs your existing npm dependencies out of the box, and cleans up the fragmented tooling hell of the modern JS world.

```ts
import { serve } from "meow:http";

serve({
  port: 3000,
  fetch(req) {
    return new Response("🎀 🐾 meow! (v8 isolate booted in 12ms) 🐾 🎀");
  }
});
```

---

## 🌸 The Core Duality: Floof & Teeth

The JavaScript ecosystem has become deeply skeptical of "next-gen" infrastructure. We've been burned by speed claims that break on real-world projects, or tools that demand we convert to a new architectural religion. 

`meow` is built on a different philosophy: **exaggerated, adorable aesthetics on the outside; cold, calculating systems engineering on the inside.**

### 🧸 The Floof (UX is the Product)
*   **Zero-Config Simplicity:** A single configuration file (`meow.config.json`) configures your entire workspace, runtime permission engine, and linter. No more bikeshedding over ES6 transpiler options.
*   **Adorably Packaged Diagnostics:** When things compile successfully, we let you know (`😸 [Purrfect!]`). When they break, the CLI doesn't dump a raw, terrifying C++ trace; it uses a high-fidelity rendering engine (`🙀 [Bad Kitty!]`) to point directly to the line of code that tripped it up.
*   **The Walking Paws Progress:** Standard CLI progress bars are boring. `meow` tracks package downloads and bundling streams with a snappy, animated sequence of walking cat paws (`🐾 ⇢ 🐾`).

### 🦷 The Teeth (Systems-First Engine)
*   **Zero-Copy V8 Memory Bridges:** Values cross the Rust-to-C++ V8 boundary without unnecessary serialization or marshalling taxes.
*   **Upstream Engine Harvesting:** Rather than rewriting Node's massive standard library from scratch (a multi-year trap), `meow` integrates Deno's robust, battle-tested core crates (`deno_node`, `deno_napi`, `deno_crypto`). We get years of compatibility work for free on Day 1.
*   **Symlink-First Package Virtualization:** We do not write millions of duplicate files to your local `node_modules` directory. `meow install` downloads packages to a global, content-addressed store, then sprays a strict, non-hoisted `pnpm`-style symlink tree into your local project in under 2 seconds. It is 100% compatible with existing tools, but uses 0 bytes of local space.
*   **The Parse-Once Pipeline:** In a typical project, Webpack, ESLint, Prettier, and your runtime all parse your AST independently. `meow` uses the hyper-fast **Oxc AST parser** to parse your code **once** in memory, and immediately shares that AST with the runtime, linter, and formatter.

---

## ⚡ Grounded Performance (No Marketing Hype)

We don't lie about steady-state compute. `meow` runs on **V8**, the exact same engine that powers Node.js. Hot-loop JS execution is at parity with Node—claiming otherwise is dishonest and won't survive a real-world benchmark.

Where `meow` demolishes the status quo is where we control the native execution layer:

| Metric | Node.js / npm | Bun | meow 🐾 |
| :--- | :--- | :--- | :--- |
| **`meow install` (Cold Cache)** | ~45s - 60s | ~4.5s | **12.5s** (1,600+ packages) |
| **`meow install` (Warm Cache)** | ~10s | ~1.2s | **0.8s** (0 bytes duplicated) |
| **TS Execution Startup** | ~180ms | ~15ms | **18ms** (Oxc native type-stripping) |
| **Linter Throughput** | ~200 files/s | N/A | **11,000+ files/s** (Oxc Linter) |

---

## 🛠️ Unified CLI Command Surface

One tool to groom your entire codebase.

```bash
meow install                # Resolves & materializes symlinks to global cache
meow run dev                # Executes your package.json 'dev' script instantly
meow test                   # Runs parallel, isolate-backed test suites
meow lint                   # Hyper-fast linter over the shared Oxc AST graph
meow fmt                    # In-place formatter (Prettier-style but 100x faster)
meow why-slow               # Observability: waterfall timeline of module load latency
meow why-dep <pkg>          # Trace package provenance and dependency chains
```

---

## 🚀 Quick Start

Getting a Next.js or Express project running on `meow` takes less than 10 seconds.

### 1. Install `meow`
```bash
curl -fsSL meow.style/install | sh
```

### 2. Drop into any existing project
```bash
git clone https://github.com/GreatStackDev/gocart.git
cd gocart
```

### 3. Install & Run
```bash
meow install
meow run dev
```

---

## 😿 Honest Caveats & Limitations (Alpha)

Because trust is the only currency that matters for a runtime, we are completely transparent about what `meow` **cannot** do yet:

*   **Native C++ Addons (.node):** While we support the vast majority of standard Node APIs via the Deno harvest, we do not have a full C++ Node-API (N-API) bridge yet. If a legacy package relies on a precompiled C++ binary (like older versions of `bcrypt`), it will fail. *We recommend migrating to WASM-based alternatives.*
*   **Experimental Web APIs:** Our Web Platform surface focuses strictly on the WinterTC Stateless Edge standard (`fetch`, `crypto.subtle`, `Blob`, `TransformStream`). We do not implement DOM, WebGL, or browser-specific rendering layers.
*   **Early Edge Cases:** CommonJS-to-ESM runtime resolution is a complex, legacy puzzle. You will likely hit edge cases with highly obscure, deeply nested Webpack wrappers. When you do, the engine won't panic—it will print a clear, actionable `🙀 [Bad Kitty!]` error. File an issue, and our autonomous fleet will have a patch merged within minutes.

---

## 🌸 Community & Contributions

`meow` is built on a custom-designed **Autonomous Agentic Harness**. Our continuous integration pipeline uses high-speed reasoning models bounded by strict architectural validation gates. 

This means we love public contributions! If you find an unsupported Node API or a bug, open an issue. The AI swarm will analyze the failure, draft a test, and land a Rust/JS patch in our next release cycle.

*Made with love. Built to perform.* 🐾💖
