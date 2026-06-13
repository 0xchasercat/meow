//! Embedded native-module sources (RT-005 / RT-007).
//!
//! The authored TypeScript under `src/js/meow/*.ts` is the single source of truth
//! for the shipped `meow:*` native-module surface:
//! - the loader executes those embedded sources directly, and
//! - `meow types` emits the committed `meow:*` declarations from the same files.
//!
//! Node built-ins are Deno-owned; this registry intentionally exposes no
//! TypeScript `node:*` polyfill sources.

use std::sync::Arc;

/// The embedded `meow:*` module names in stable order. This order drives typegen
/// output and shadow-sync writes, so keep it byte-stable.
pub const NATIVE_MODULES: &[&str] = &["http", "ui"];

/// Node built-ins are Deno-owned. Meow keeps this empty so the resolver never
/// treats TypeScript shims as the source of truth for `node:*` modules.
pub const NODE_BUILTINS: &[&str] = &[];

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
