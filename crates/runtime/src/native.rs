//! Embedded native-module sources (RT-005 / RT-007).
//!
//! The authored TypeScript under `src/js/meow/*.ts` and `src/js/node/*.ts` is the
//! single source of truth for the shipped native-module surface:
//! - the loader executes those embedded sources directly, and
//! - `meow types` emits the committed `meow:*` declarations from the same files.
//!
//! RT-007 adds a second namespace: `node:*` built-ins. Their runtime registry
//! lives here too so the resolver can ask the runtime for the one canonical
//! builtin-name set (I-5) instead of hard-coding it twice.

use std::sync::Arc;

/// The embedded `meow:*` module names in stable order. This order drives typegen
/// output and shadow-sync writes, so keep it byte-stable.
pub const NATIVE_MODULES: &[&str] = &["http", "ui"];

/// The common-surface `node:*` built-ins shipped by RT-007. Keep this list in a
/// stable order: resolver diagnostics and tests snapshot it.
///
/// The LOAD-003 / RT-007 expansion below lists the *rest* of Node's core
/// built-ins as "empty shims": Next.js's Webpack config marks all of them as
/// externals and statically `require()`s them, so the resolver must satisfy the
/// graph for every one of them. Real implementations stay in
/// `native_module_source`; everything else resolves to `js/node/empty.ts`.
pub const NODE_BUILTINS: &[&str] = &[
    // RT-007 (implemented)
    "assert",
    "buffer",
    "child_process",
    "crypto",
    "dns",
    "events",
    "fs",
    "fs/promises",
    "http",
    "https",
    "module",
    "os",
    "path",
    "process",
    "url",
    "util",
    // === LOAD-003 / RT-007 ===
    // Webpack/Next.js externals — empty shims so the static require graph
    // resolves. The order above remains the only canonical, "implemented"
    // surface; the entries below are deliberately grouped under a single
    // banner to keep the snapshot diff readable.
    "cluster",
    "dgram",
    "diagnostics_channel",
    "dns/promises",
    "domain",
    "http2",
    "inspector",
    "net",
    "perf_hooks",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "stream/consumers",
    "stream/promises",
    "stream/web",
    "string_decoder",
    "sys",
    "timers",
    "timers/promises",
    "tls",
    "trace_events",
    "tty",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
    // === /LOAD-003 / RT-007 ===
];

/// Loader-facing registry of embedded native-module sources.
pub trait NativeModuleSource: Send + Sync {
    fn modules(&self) -> &'static [&'static str];
    fn node_builtins(&self) -> &'static [&'static str];
    /// Return the embedded source for a canonical native-module name:
    /// `http` for `meow:http`, `node:fs` for the Node built-ins, etc.
    fn source(&self, name: &str) -> Option<&'static str>;
}

/// Zero-sized registry implementation backed by `include_str!`.
#[derive(Debug, Default, Clone, Copy)]
pub struct MeowNativeModules;

impl NativeModuleSource for MeowNativeModules {
    fn modules(&self) -> &'static [&'static str] {
        NATIVE_MODULES
    }

    fn node_builtins(&self) -> &'static [&'static str] {
        NODE_BUILTINS
    }

    fn source(&self, name: &str) -> Option<&'static str> {
        native_module_source(name)
    }
}

pub fn native_module_registry() -> Arc<dyn NativeModuleSource> {
    Arc::new(MeowNativeModules)
}

pub fn native_module_source(name: &str) -> Option<&'static str> {
    match name {
        "http" => Some(include_str!("js/meow/http.ts")),
        "ui" => Some(include_str!("js/meow/ui.ts")),
        // === LOAD-004 ===
        "internal/cjs" => Some(include_str!("js/meow/cjs.ts")),
        // === /LOAD-004 ===
        // === RT-007 ===
        "node:assert" => Some(include_str!("js/node/assert.ts")),
        "node:buffer" => Some(include_str!("js/node/buffer.ts")),
        "node:child_process" => Some(include_str!("js/node/child_process.ts")),
        "node:crypto" => Some(include_str!("js/node/crypto.ts")),
        "node:dns" => Some(include_str!("js/node/dns.ts")),
        "node:events" => Some(include_str!("js/node/events.ts")),
        "node:fs" => Some(include_str!("js/node/fs.ts")),
        "node:fs/promises" => Some(include_str!("js/node/fs_promises.ts")),
        "node:http" => Some(include_str!("js/node/http.ts")),
        "node:https" => Some(include_str!("js/node/https.ts")),
        "node:module" => Some(include_str!("js/node/module.ts")),
        "node:os" => Some(include_str!("js/node/os.ts")),
        "node:path" => Some(include_str!("js/node/path.ts")),
        "node:process" => Some(include_str!("js/node/process.ts")),
        "node:url" => Some(include_str!("js/node/url.ts")),
        "node:util" => Some(include_str!("js/node/util.ts")),
        // === LOAD-003 / RT-007 ===
        // Webpack/Next.js externals — empty shims so the static require graph
        // resolves. Each name listed in the shim section of `NODE_BUILTINS`
        // maps to the same `empty.ts` source.
        "node:cluster" => Some(include_str!("js/node/empty.ts")),
        "node:dgram" => Some(include_str!("js/node/empty.ts")),
        "node:diagnostics_channel" => Some(include_str!("js/node/empty.ts")),
        "node:dns/promises" => Some(include_str!("js/node/empty.ts")),
        "node:domain" => Some(include_str!("js/node/empty.ts")),
        "node:http2" => Some(include_str!("js/node/empty.ts")),
        "node:inspector" => Some(include_str!("js/node/empty.ts")),
        "node:net" => Some(include_str!("js/node/empty.ts")),
        "node:perf_hooks" => Some(include_str!("js/node/empty.ts")),
        "node:punycode" => Some(include_str!("js/node/empty.ts")),
        "node:querystring" => Some(include_str!("js/node/empty.ts")),
        "node:readline" => Some(include_str!("js/node/empty.ts")),
        "node:repl" => Some(include_str!("js/node/empty.ts")),
        "node:stream" => Some(include_str!("js/node/empty.ts")),
        "node:stream/consumers" => Some(include_str!("js/node/empty.ts")),
        "node:stream/promises" => Some(include_str!("js/node/empty.ts")),
        "node:stream/web" => Some(include_str!("js/node/empty.ts")),
        "node:string_decoder" => Some(include_str!("js/node/empty.ts")),
        "node:sys" => Some(include_str!("js/node/empty.ts")),
        "node:timers" => Some(include_str!("js/node/empty.ts")),
        "node:timers/promises" => Some(include_str!("js/node/empty.ts")),
        "node:tls" => Some(include_str!("js/node/empty.ts")),
        "node:trace_events" => Some(include_str!("js/node/empty.ts")),
        "node:tty" => Some(include_str!("js/node/empty.ts")),
        "node:v8" => Some(include_str!("js/node/empty.ts")),
        "node:vm" => Some(include_str!("js/node/empty.ts")),
        "node:wasi" => Some(include_str!("js/node/empty.ts")),
        "node:worker_threads" => Some(include_str!("js/node/empty.ts")),
        "node:zlib" => Some(include_str!("js/node/empty.ts")),
        // === /LOAD-003 / RT-007 ===
        _ => None,
    }
}

pub fn native_module_declaration(name: &str) -> Option<&'static str> {
    match name {
        "http" => Some(include_str!("../types/meow/http.d.ts")),
        "ui" => Some(include_str!("../types/meow/ui.d.ts")),
        _ => None,
    }
}
