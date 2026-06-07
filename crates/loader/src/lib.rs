//! `meow-loader` — THE module loader (LOAD-001): resolve → cache/disk read → graph
//! parse + type-erase → V8.
//!
//! Hosts the one [`Resolver`] the whole toolchain shares (I-1, I-5). The
//! [`deno_core::ModuleLoader`] impl is a thin adapter: deno_core's sync `resolve`
//! delegates to [`Resolver::locate`] (URL only, dedup-safe), and `load` re-enters
//! [`Resolver::resolve`] with the already-absolute URL to read the source, then
//! hands that text to the SHARED [`GraphDb`] and pulls back the memoized,
//! type-erased, V8-ready IR. The loader owns neither a parser (I-1: no
//! `oxc_parser` here) nor a second resolution path, and never touches
//! `node_modules` (I-5).

mod resolver;
mod url;

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::error::ModuleLoaderError;
use deno_core::url::Url;
use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};
use meow_graph::GraphDb;

pub use crate::resolver::{ModuleKind, ModuleLocator, ResolveError, ResolvedModule, Resolver};
pub use crate::url::{
    decode as decode_cache_url, encode as encode_cache_url, SCHEME as CACHE_SCHEME,
};

/// The single `deno_core::ModuleLoader` for `meow run` (and later the LSP's loader).
pub struct MeowModuleLoader {
    resolver: Resolver,
    /// The shared graph (I-1). The loader NEVER constructs a parser; it feeds text
    /// in and pulls the memoized runtime IR back out. `RefCell` because `load` is
    /// `&self` but `GraphDb::set_file` needs `&mut`; the runtime is single-threaded.
    graph: Rc<RefCell<GraphDb>>,
}

impl MeowModuleLoader {
    pub fn new(resolver: Resolver, graph: Rc<RefCell<GraphDb>>) -> MeowModuleLoader {
        MeowModuleLoader { resolver, graph }
    }
}

impl ModuleLoader for MeowModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        let referrer = parse_referrer(referrer, self.resolver.project_root());
        let (url, _locator) = self
            .resolver
            .locate(specifier, &referrer)
            .map_err(resolve_error)?;
        Ok(url)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        ModuleLoadResponse::Sync(self.load_sync(module_specifier, maybe_referrer))
    }
}

impl MeowModuleLoader {
    fn load_sync(
        &self,
        module_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleLoadReferrer>,
    ) -> Result<ModuleSource, ModuleLoaderError> {
        let referrer = maybe_referrer
            .map(|r| r.specifier.clone())
            .unwrap_or_else(|| self.resolver.project_root().clone());

        // Re-enter the ONE resolver with the already-resolved (absolute) URL → reads
        // the source from disk (file:) or the content-addressed cache (meow-cache:).
        let resolved = self
            .resolver
            .resolve(module_specifier.as_str(), &referrer)
            .map_err(resolve_error)?;

        // Hand the source to the SHARED graph; pull back the memoized, type-erased,
        // V8-ready IR (the RT-003 strip runs inside the graph). No parser here (I-1).
        let code = {
            let mut db = self.graph.borrow_mut();
            let graph_path = graph_path_for(&resolved.url, module_specifier);
            let fid = db.set_file(graph_path, resolved.source.clone());
            match db.runtime_ir(fid) {
                Some(Ok(ir)) => ir.code.to_string(),
                Some(Err(diagnostics)) => {
                    return Err(graph_error(module_specifier, diagnostics));
                }
                // We just interned the file; an unknown id here is an internal invariant
                // break, surfaced honestly rather than panicked.
                None => {
                    return Err(ModuleLoaderError::generic(format!(
                        "meow: internal: module {module_specifier} was not interned in the graph"
                    )));
                }
            }
        };

        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            module_specifier,
            None,
        ))
    }
}

/// Parse deno_core's referrer string into a `Url`, falling back to the project root
/// for the entry module / synthetic referrers (e.g. `"."`).
fn parse_referrer(referrer: &str, project_root: &Url) -> Url {
    Url::parse(referrer).unwrap_or_else(|_| project_root.clone())
}

/// Convert a typed [`ResolveError`] into deno_core's `ModuleLoaderError`. deno_core
/// only accepts its own `JsErrorClass` errors via `from_err`, so we preserve the
/// causal Display chain (e.g. a cache `IntegrityMismatch`, a `.cjs` refusal) as the
/// generic message — never a panic, never a swallowed cause.
fn resolve_error(err: ResolveError) -> ModuleLoaderError {
    ModuleLoaderError::generic(format!("meow: {err}"))
}

/// The path key + source-type hint handed to `GraphDb`. `file:` URLs carry their
/// real extension (so a `.ts` module is parsed as TypeScript and stripped); cached
/// modules are content-addressed ESM, labelled `.mjs` so they parse as ESM JS. The
/// key is independent of V8's module identity (`module_specifier`); it only drives
/// the graph's per-file memo + source-type detection.
fn graph_path_for(url: &Url, module_specifier: &ModuleSpecifier) -> std::path::PathBuf {
    match url.to_file_path() {
        Ok(path) => path,
        // Non-file (meow-cache:) → synthesize a unique, ESM-typed key from the URL.
        Err(()) => std::path::PathBuf::from(format!("{}.mjs", module_specifier.as_str())),
    }
}

/// Build an honest `ModuleLoaderError` from the graph's lowering diagnostics
/// (parse/semantic error or a non-erasable TS construct). Carries the GRAPH
/// diagnostic's own message + fix — no fabricated text.
fn graph_error(
    specifier: &ModuleSpecifier,
    diagnostics: &[meow_graph::StripDiagnostic],
) -> ModuleLoaderError {
    let detail = if diagnostics.is_empty() {
        "the module could not be lowered to runnable JavaScript".to_owned()
    } else {
        diagnostics
            .iter()
            .map(|d| {
                if d.help.is_empty() {
                    d.message.clone()
                } else {
                    format!("{} — {}", d.message, d.help)
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    ModuleLoaderError::generic(format!("meow: cannot load {specifier}: {detail}"))
}
