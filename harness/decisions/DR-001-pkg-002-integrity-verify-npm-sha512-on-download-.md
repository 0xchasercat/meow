# DR-001 — PKG-002 integrity: verify npm sha512 on download, content-address by meow sha256 (cache+lockfile key)

> Drafted from a Mission Control correction of `A-PKG002-1` (assumption) on 2026-06-08T01:37:27.332Z.

## Context

npm dist.integrity (sha512) is the supply-chain check at download; the meow sha256 is the cache key + LockEntry.integrity + I-7 anchor. sha512 verified-then-discarded at P2.

**It assumed:** PKG-001 cache is sha256; npm SRI is sha512. Verifying npm then content-addressing by sha256 reuses the existing cache without extending HashAlgo; provenance (persist sha512) deferred to SEC-002.

**Already shipped in:** PKG-002

## Decision

You cannot fight package.json. It isn't just a config file; it is the database of the entire JavaScript ecosystem.
If we strictly enforce ADR-8 (where meow actively overwrites package.json and forces everyone to use meow.config.json), the friction to adopt meow in an existing Next.js, SvelteKit, or Express project is too high. Developers want to clone their existing repo, type meow install, and have it just work.
What "Drop-In Compat" Means for meow
To make meow a true drop-in replacement, we need to adjust the architecture so that it treats package.json as a first-class citizen, not a generated shadow.
Here is what we need the Orchestrator to wire up:
Bidirectional meow install: If a user types meow install svelte in a directory with a package.json, meow reads the existing package.json, resolves the dependency, and writes "svelte": "^5.56.3" directly into package.json (just like npm/bun/pnpm).
First-Class Scripts: meow run dev or meow task build must natively parse the "scripts" block inside package.json. We can keep meow.tasks.ts for advanced, typed task graphs, but standard npm run dev workflows must work out of the box.
Transparent Resolution: The LOAD-003 resolver we just built needs to treat package.json's dependencies and devDependencies as the root of the module graph if meow.config.json doesn't explicitly override it.

## Status

Resolved A-PKG002-1 → this DR. The next agent inherits this as settled knowledge.
