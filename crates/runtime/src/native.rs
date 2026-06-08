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
pub const NODE_BUILTINS: &[&str] = &[
    "assert",
    "buffer",
    "events",
    "fs",
    "fs/promises",
    "os",
    "path",
    "process",
    "url",
    "util",
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
        "node:events" => Some(include_str!("js/node/events.ts")),
        "node:fs" => Some(include_str!("js/node/fs.ts")),
        "node:fs/promises" => Some(include_str!("js/node/fs_promises.ts")),
        "node:os" => Some(include_str!("js/node/os.ts")),
        "node:path" => Some(include_str!("js/node/path.ts")),
        "node:process" => Some(include_str!("js/node/process.ts")),
        "node:url" => Some(include_str!("js/node/url.ts")),
        "node:util" => Some(include_str!("js/node/util.ts")),
        // === /RT-007 ===
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
