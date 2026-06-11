//! P0 module loader (RT-001 · A3) — a deliberately small stand-in.
//!
//! This is NOT the real resolver. LOAD-001 replaces it with the shared,
//! content-addressed cache + module graph (I-1, I-5). Until then this serves
//! exactly two sources:
//!   - preloaded in-memory modules (via [`TrivialModuleLoader::with_module`]),
//!     backing from-string ESM and tests;
//!   - `file:` specifiers, read once from disk — the entry program the user
//!     explicitly named (an explicit read, not ambient fs authority).
//!
//! Anything else is a diagnostic error naming the unresolved specifier (no panic).

use std::collections::HashMap;

use deno_core::error::ModuleLoaderError;
use deno_core::{
    resolve_import, ModuleCodeString, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse,
    ModuleLoader, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};

/// The throwaway P0 loader. See module docs.
#[derive(Default)]
pub struct TrivialModuleLoader {
    modules: HashMap<ModuleSpecifier, String>,
}

impl TrivialModuleLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Preload a module by specifier. Builder-style so a runtime can be seeded
    /// with from-string sources before `run_main_module`.
    pub fn with_module(
        mut self,
        specifier: ModuleSpecifier,
        source: impl Into<ModuleCodeString>,
    ) -> Self {
        self.modules
            .insert(specifier, source.into().as_str().to_owned());
        self
    }
}

impl ModuleLoader for TrivialModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        resolve_import(specifier, referrer).map_err(ModuleLoaderError::from_err)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        ModuleLoadResponse::Sync(self.load_sync(module_specifier))
    }
}

impl TrivialModuleLoader {
    fn load_sync(&self, specifier: &ModuleSpecifier) -> Result<ModuleSource, ModuleLoaderError> {
        // 1. Preloaded in-memory sources win (explicit, type = JavaScript).
        if let Some(code) = self.modules.get(specifier) {
            return Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(code.clone().into()),
                specifier,
                None,
            ));
        }

        // 2. `file:` specifiers: read the named program off disk, once.
        if specifier.scheme() == "file" {
            return self.load_file(specifier);
        }

        // 3. Everything else is unresolved at P0 (no node_modules, no cache).
        Err(ModuleLoaderError::generic(format!(
            "meow: cannot resolve module {specifier} (the P0 loader only serves \
             preloaded and file: modules; the real resolver lands in LOAD-001)"
        )))
    }

    fn load_file(&self, specifier: &ModuleSpecifier) -> Result<ModuleSource, ModuleLoaderError> {
        let path = specifier.to_file_path().map_err(|_| {
            ModuleLoaderError::generic(format!("meow: invalid file path in specifier {specifier}"))
        })?;

        // TypeScript needs the type-strip + shared graph (RT-003); fail honestly
        // rather than feeding TS source to V8 and surfacing a confusing syntax error.
        match path.extension().and_then(|e| e.to_str()) {
            Some("ts" | "mts" | "cts" | "tsx") => {
                return Err(ModuleLoaderError::generic(format!(
                    "meow: TypeScript execution needs the type-strip (RT-003); \
                     {} is not plain JavaScript",
                    path.display()
                )));
            }
            Some("js" | "mjs") | None => {}
            Some(other) => {
                return Err(ModuleLoaderError::generic(format!(
                    "meow: unsupported module extension .{other} for {}",
                    path.display()
                )));
            }
        }

        let code = std::fs::read_to_string(&path).map_err(|err| {
            ModuleLoaderError::generic(format!("meow: cannot read {}: {err}", path.display()))
        })?;
        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            specifier,
            None,
        ))
    }
}
