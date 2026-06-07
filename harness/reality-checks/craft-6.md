# craft gate · round 6 — RT-002 (async I/O layer)

- **Gate:** `craft` (CONSTITUTION A10 / P9) · **verdict:** `green` (after the fix loop)
- **At:** 2026-06-07 · **Scope:** `crates/runtime/src/io/**` + RT-002 fences in `lib.rs`
- **Reviewer:** independent (not the author), against `harness/CRAFT.md` + ADR-9 + I-6/I-8/I-11.

## Finding (BLOCK) — raw I/O ops were exposed on every runtime
`Runtime::new` installed the I/O extension unconditionally, so ordinary user JS could call `Deno.core.ops.op_read_file("/etc/hosts")` / `op_tcp_connect(...)` directly. With the default `AllowAll` seam that is **unmediated host filesystem/network access from any executed module** (I-6/I-8) — and it contradicted RT-002's own scope (the ops are internal plumbing for later mediated specs, not a default user API).

**Fix:** `Runtime::new` no longer installs the I/O extension — the default set is only the console/print extension. The I/O ops are **opt-in** via `RuntimeOptions.extensions` (for tests + future mediated consumers: meow:fs + SEC/P6). Regression test `io_ops_absent_by_default` (a default runtime sees `op_read_file` as `undefined`/uncallable) + orchestrator smoke: a default `meow run` module probing `op_read_file("/etc/hosts")` prints **"blocked-by-default"**.

## Other RT-002 craft (accepted)
- Capability seam: every I/O op consults `CapabilityCheck::check` before host I/O; `AllowAll`-default is honestly marked a seam, not a security boundary; deny-policy test proves the hook.
- **Zero performance claims** (I-11). `io_uring` genuinely deferred — single tokio backend, no speculative Linux-only code (ADR-9).
- Typed `RuntimeIoError` surfaced as rejected JS promises; no panic on bad path/addr/refused connect.

## Result
Floor green: `clippy -D warnings` clean · `cargo test -p meow-runtime` **16 pass** · workspace tests green · `fmt` · `principles`. `footprint` green (gzip 18.9 MB ≤ 60 MB).
