//! `meow-loader` — THE module loader: resolve → cache/disk read → graph lowering → V8.
//!
//! Hosts the one [`Resolver`] the whole toolchain shares (I-1, I-5). The
//! [`deno_core::ModuleLoader`] impl is a thin adapter: deno_core's sync `resolve`
//! delegates to [`Resolver::locate`] (identity only), and `load` re-enters
//! [`Resolver::resolve`] with the already-absolute URL to read the source.

pub mod package;
mod resolver;
mod url;

use std::cell::RefCell;
use std::path::PathBuf;
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

pub struct MeowModuleLoader {
    resolver: Resolver,
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
        let resolved = self
            .resolver
            .resolve(module_specifier.as_str(), &referrer)
            .map_err(resolve_error)?;

        match resolved.kind {
            ModuleKind::Json => {
                return Ok(ModuleSource::new(
                    ModuleType::Json,
                    ModuleSourceCode::String(resolved.source.as_ref().to_owned().into()),
                    module_specifier,
                    None,
                ));
            }
            ModuleKind::Cjs => {
                let err = match decode_cache_url(&resolved.url) {
                    Ok((package, member)) => ResolveError::CjsDependencyUnsupported {
                        package: self.resolver.package_label(&package),
                        member,
                    },
                    Err(_) => ResolveError::CjsDependencyUnsupported {
                        package: resolved.url.to_string(),
                        member: resolved.url.path().to_owned(),
                    },
                };
                return Err(resolve_error(err));
            }
            ModuleKind::Esm => {}
        }

        let code = {
            let mut db = self.graph.borrow_mut();
            let graph_path = graph_path_for(&resolved.url, module_specifier);
            let fid = db.set_file(graph_path, resolved.source.clone());
            match db.runtime_ir(fid) {
                Some(Ok(ir)) => ir.code.to_string(),
                Some(Err(diagnostics)) => {
                    return Err(graph_error(module_specifier, diagnostics));
                }
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

fn parse_referrer(referrer: &str, project_root: &Url) -> Url {
    Url::parse(referrer).unwrap_or_else(|_| project_root.clone())
}

fn resolve_error(err: ResolveError) -> ModuleLoaderError {
    ModuleLoaderError::generic(format!("meow: {err}"))
}

fn graph_path_for(url: &Url, module_specifier: &ModuleSpecifier) -> PathBuf {
    if let Ok(path) = url.to_file_path() {
        return path;
    }
    if let Ok((package, member)) = decode_cache_url(url) {
        return PathBuf::from("meow-cache")
            .join(package.to_url_host())
            .join(member);
    }
    // === RT-005 ===
    if url.scheme() == "meow" {
        return PathBuf::from("meow-native").join(format!("{}.ts", url.path()));
    }
    // === /RT-005 ===
    PathBuf::from(format!("{}.mjs", module_specifier.as_str()))
}

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
