# DR-002 — PKG-002 version resolution uses the pinned semver crate, not node-semver

> Drafted from a Mission Control correction of `A-PKG002-5` (assumption) on 2026-06-08T01:39:24.767Z.

## Context

Range resolution uses the workspace semver crate; node-semver-only forms error honestly (NoMatchingVersion/UnsupportedRange) rather than mis-resolve.

**It assumed:** Avoids a node-semver reimplementation now; the honesty gate (I-11) covers the gap; a faithful engine is a later option.

**Already shipped in:** PKG-002

## Decision

Almost every project is unusable with meow, we actually need to support it fully, and this is adding on to my previous correction of drop-in compat for package.json

## Status

Resolved A-PKG002-5 → this DR. The next agent inherits this as settled knowledge.
