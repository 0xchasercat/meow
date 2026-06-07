//! Embedded `meow:*` module sources + committed declarations (RT-005).
//!
//! The authored TypeScript under `src/js/meow/*.ts` is the single source of truth:
//! the loader executes those embedded sources, and `meow types` emits the shipped
//! declarations from the same files into `types/meow/*.d.ts`.

use std::sync::Arc;

/// The embedded `meow:*` module names in stable order. This order drives typegen
/// output and shadow-sync writes, so keep it byte-stable.
pub const NATIVE_MODULES: &[&str] = &["http"];

/// Loader-facing registry of embedded native-module sources.
pub trait NativeModuleSource: Send + Sync {
    fn modules(&self) -> &'static [&'static str];
    fn source(&self, name: &str) -> Option<&'static str>;
}

/// Zero-sized registry implementation backed by `include_str!`.
#[derive(Debug, Default, Clone, Copy)]
pub struct MeowNativeModules;

impl NativeModuleSource for MeowNativeModules {
    fn modules(&self) -> &'static [&'static str] {
        NATIVE_MODULES
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
        _ => None,
    }
}

pub fn native_module_declaration(name: &str) -> Option<&'static str> {
    match name {
        "http" => Some(include_str!("../types/meow/http.d.ts")),
        _ => None,
    }
}
