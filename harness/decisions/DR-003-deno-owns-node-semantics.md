---
dr: DR-003
title: Deno owns Node compatibility; meow owns the PnP bridge
date: 2026-06-08
status: accepted
supersedes: null
constitution_ref: [Amendment-003, I-1, I-5, I-7, I-10, I-11]
---

## Context

The drop-in mandate exposed two compatibility failures in the meow-owned Node approach: synthetic CJS-to-ESM wrappers do not preserve native CommonJS behavior for dynamic `require` and circular graphs, and hand-authored Node built-ins duplicate a maintained implementation already available in Deno's extension crates.

## Decision

Use upstream Deno crates for Node semantics: `deno_node` for built-ins/globals/CommonJS, `deno_napi` for N-API addons, and the matching Deno crypto/fetch/net/fs/io support crates for that `deno_core` generation. Meow's responsibility is the bridge: select packages from the PnP `ResolutionGraph` (runtime truth), verify/cache them, materialize immutable package paths in `UnpackedStore`, and keep UX/policy/tooling coherent.

## Consequences

`ResolutionGraph` is the single runtime-resolution authority; all runtime and internal tool resolution starts from the same graph and selected referrer edges. `UnpackedStore` is the required real filesystem backing for Deno Node/N-API and any `node:*` resolver work. `.meow/deps/` is a flat, symlink-first editor/TS/LSP projection over root/package names only, with multi-version collisions recorded and `--materialize` retained as the explicit on-disk `node_modules` escape hatch for foreign tools requiring exact traversal.
`meow-cache://` stays internal only for diagnostics/internal identity; it is never the path handed to Deno's resolver/loader.

Reversal would require proving Deno's Node stack cannot be adapted to meow's PnP graph without unacceptable footprint or correctness costs, then accepting a maintained meow-owned Node compatibility fork.

## Alternatives considered

- **Continue meow-owned CJS wrapper and polyfills.** Rejected: already hit dynamic `require` and CJS cycle/TDZ failures; long-term maintenance duplicates Deno.
- **Force `meow-cache://` through Deno's Node resolver.** Rejected: upstream resolver/N-API APIs are path-oriented and native addons require real files for `dlopen` / `LoadLibrary`.
- **Default to project `node_modules`.** Rejected: violates the PnP/default no-`node_modules` architecture; `--materialize` remains the explicit escape hatch.
