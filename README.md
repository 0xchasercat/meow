<div align="center">
  <img src="banner.webp" alt="meow banner" width="100%" />
</div>

<div align="center">
  <h1>meow</h1>
  <p><strong>Purrs like a kitten. Runs like Rust.</strong></p>
  <p>
    <a href="https://github.com/0xchasercat/meow/actions"><img src="https://img.shields.io/badge/build-passing-brightgreen?style=flat-square&color=98FF98" alt="Build Status"></a>
    <a href="https://github.com/0xchasercat/meow/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT_/_Apache--2.0-blue?style=flat-square&color=89CFF0" alt="License"></a>
    <a href="https://meow.style"><img src="https://img.shields.io/badge/website-meow.style-lightgrey?style=flat-square&color=FFB7C5" alt="Website"></a>
    <a href="https://github.com/0xchasercat/meow/stars"><img src="https://img.shields.io/github/stars/0xchasercat/meow?style=flat-square&color=Floof" alt="GitHub Stars"></a>
  </p>
</div>

---

# 🐾 meow

> **The last JavaScript runtime.** > *Purrs like a kitten. Runs like Rust.*

`meow` is an adorable, all-in-one JavaScript/TypeScript runtime, blazing-fast package manager, deterministic test runner, and unified quality-assurance toolchain delivered as a single, self-contained Rust binary. 

We didn't set out to reinvent the wheel or add yet another competing standard to a fractured landscape. Instead, `meow` is built as the ultimate **connective tissue** for modern web development. By leveraging the battle-tested, ironclad runtime layers engineered by the Deno team and marrying them directly to the ultra-fast Oxc parsing pipeline, `meow` collapses your entire workspace stack into a unified, secure-by-default environment. 

One AST parsed exactly once in memory. Zero redundant allocations. Zero configurations. Complete engineering harmony.

---

## ⚡ Brutal Performance. Adorable UX.

* **The Parse-Once Pipeline:** Webpack, ESLint, Prettier, and Jest all drag your code through separate parsers, melting your CPU. `meow` maps your codebase **exactly once** in memory using the Oxc parser, natively feeding that single AST to the runtime, linter, formatter, typechecker, and bundler simultaneously.
* **Soft Paws, Zero Waste Installs:** Packages download to a global content-addressed cache exactly once and instantly project into your workspace via Copy-on-Write (`clonefile` on macOS APFS) or highly parallel hardlinking (Linux/Windows). You get millisecond warm installs, zero messy symlink loops, and **0 bytes of duplicated disk space**.
* **Fast by Math, Not by Cheating:** We don't skip cryptographic supply-chain signatures just to win Twitter speed benchmarks. `meow` executes full, ironclad **SHA-512 verification** by offloading heavy hashing to background OS threads so your network never stalls.
* **Hermetic & Deterministic by Default:** Third-party execution utilities (like `npx` or dynamic imports) are a massive supply-chain security hazard. `meow` executes in a strict isolation sandbox by default: the system clock is frozen, environmental variables are hidden, and randomness is seeded. 
* **Framework Ready from Day 1:** No magic, no toy examples. Powered by a highly tuned V8 engine, `meow` natively boots Next.js 15, Astro, Vite, Playwright, and Puppeteer right out of the box with full support for CommonJS and Node built-ins.

---

## 🏗️ Architectural Layout

`meow` is architected with strict structural separation to isolate side effects from core compiler and runtime execution states, driven by a cooperative, single-threaded async scheduler:


```

```
              ┌───────────────────────────────────┐
              │         Main OS Thread            │
              │   (tokio LocalSet Execution)      │
              └─────────────────┬─────────────────┘
                                │
     ┌──────────────────────────┼──────────────────────────┐
     ▼                          ▼                          ▼

```

┌──────────────────┐       ┌──────────────────┐       ┌──────────────────┐
│   Main Isolate   │       │ Worker Isolate 1 │       │ Worker Isolate 2 │
│ (JsRuntime !Send)│       │ (JsRuntime !Send)│       │ (JsRuntime !Send)│
└──────────────────┘       └──────────────────┘       └──────────────────┘

```

* **`meow-graph` (Incremental Oxc Pipeline):** The single parsing and semantic analysis entrance for the workspace. Manages lossless syntax trees (CST), scopes, and references as lazy, invalidatable queries.
* **`meow-runtime` (V8 Embedding):** Manages V8 isolate orchestration. Implements a cooperative, single-threaded, multi-isolate event loop allowing workers (like Svelte/Vite parallel bundling pipelines) to interleave perfectly without the heavy context-switching overhead of OS threads.
* **`meow-pkg` (Package & Cache Layer):** Models the `meow.lock.jsonl` schema (strictly sorted, git-merge resistant JSON-lines) and coordinates fast, semaphore-guarded filesystem materialization to completely eliminate `EMFILE` crashes.
* **`meow-ui` (Terminal UX Engine):** A dependency-free terminal rendering engine that turns cryptic compiler traces into beautifully structured panels, line gutters, and inline carets, gracefully degrading to plain text in CI pipelines.

---

## 🚀 Getting Started

### 1. Install meow
Bring the engine to your machine instantly:
```bash
curl -fSL [https://meow.sh/install](https://meow.sh/install) | sh

```

### 2. Initialize a Project

Scaffold a clean workspace:

```bash
meow init

```

This writes your `package.json`, generates a unified `meow.config.json`, and sets up editor shims automatically.

### 3. Add Dependencies

Add packages securely with background-threaded verification:

```bash
meow add lodash-es
meow add -D svelte

```

### 4. Execute and Build

Run a TypeScript entry file, dev server, or build pipeline directly:

```bash
meow run main.ts
meow dev
meow run build

```

---

## 🐾 Command Catalog

`meow` bundles all ambient developer capabilities into clean, lightning-fast verbs:

```
RUN
  run       Execute a file or a package.json script
  dev       Start the dev script (meow run dev)
  task      Run a typed task from meow.tasks.ts
  test      Run the isolate-backed, deterministic test runner

PACKAGES
  install   Resolve and install dependencies from the lockfile
  add       Add a dependency and update the lockfile
  remove    Remove a dependency
  why-dep   Explain precisely why a package exists in the dependency tree

QUALITY
  check     Typecheck the project via tsc shims
  lint      Analyze source files over the shared Oxc AST pipeline
  fmt       Format source files natively with white-space preservation
  bundle    Bundle the module graph via embedded Rolldown pipelines

INSIGHT
  why-slow  Visualize module-load timelines and cold-start drag
  why-large Analyze the heaviest modules and duplicate packages in the tree
  doctor    Verify environment, config, and lockfile health checks
  sync      Regenerate shadow configurations and types
  ls        List active dev servers and processes running in the workspace

```

---

## 🛡️ Security Boundaries & Opt-Outs

To protect you against malicious npm updates, `meow` isolates script execution by default:

```bash
🐾 Executing create-next-app in strict isolation.
Set MEOW_DANGEROUSLY_DISABLE_SECURITY=1 or pass --trust to bypass.

```

We treat you like an adult. If you want to take the training wheels off completely and skip the flag typing, just run:

```bash
echo "export MEOW_DANGEROUSLY_DISABLE_SECURITY=1" >> ~/.zshrc

```

Power users can haul ass with zero nag screens; security-conscious CI environments stay completely locked down.

---

## ✦ The Equation

0 config + 0 duplicated bytes + 1 binary = meow