//! `meow-loader` — THE module loader: resolve → cache/disk read → graph lowering → V8.
//!
//! Hosts the one [`Resolver`] the whole toolchain shares (I-1, I-5). The
//! [`deno_core::ModuleLoader`] impl is a thin adapter: deno_core's sync `resolve`
//! delegates to [`Resolver::locate`] (identity only), and `load` re-enters
//! [`Resolver::resolve`] with the already-absolute URL to read the source.

pub mod package;
// === LOAD-004 ===
mod cjs;
// === /LOAD-004 ===
mod resolver;
mod url;

use std::cell::RefCell;
use std::collections::HashMap;
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
    // === LOAD-004 ===
    cjs_plans: RefCell<HashMap<String, cjs::CjsPlan>>,
    // === /LOAD-004 ===
}

impl MeowModuleLoader {
    pub fn new(resolver: Resolver, graph: Rc<RefCell<GraphDb>>) -> MeowModuleLoader {
        MeowModuleLoader {
            resolver,
            graph,
            // === LOAD-004 ===
            cjs_plans: RefCell::new(HashMap::new()),
            // === /LOAD-004 ===
        }
    }
}

impl ModuleLoader for MeowModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        if let Ok(url) = Url::parse(specifier) {
            if cjs::parse_companion_url(&url).is_some() {
                return Ok(url);
            }
        }
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
        // === LOAD-004 ===
        if let Some((original_url, kind)) = cjs::parse_companion_url(module_specifier) {
            self.ensure_cjs_plan(&original_url)?;
            let plans = self.cjs_plans.borrow();
            let key = original_url.as_str().to_owned();
            let plan = plans.get(&key).ok_or_else(|| {
                internal_loader_error(format!(
                    "missing CommonJS plan for companion {module_specifier}"
                ))
            })?;
            return Ok(cjs::companion_module_source(module_specifier, kind, plan));
        }
        // === /LOAD-004 ===

        let referrer = maybe_referrer
            .map(|r| r.specifier.clone())
            .unwrap_or_else(|| self.resolver.project_root().clone());
        let resolved = self
            .resolver
            .resolve(module_specifier.as_str(), &referrer)
            .map_err(resolve_error)?;

        if resolved.kind == ModuleKind::Json {
            return Ok(ModuleSource::new(
                ModuleType::Json,
                ModuleSourceCode::String(resolved.source.as_ref().to_owned().into()),
                module_specifier,
                None,
            ));
        }

        let graph_path = graph_path_for(&resolved.url, module_specifier);
        let mut db = self.graph.borrow_mut();
        let fid = db.set_file(graph_path.clone(), resolved.source.clone());
        let analysis = {
            let cst = db.cst(fid).ok_or_else(|| {
                internal_loader_error(format!(
                    "module {module_specifier} was not interned in the graph"
                ))
            })?;
            meow_graph::analyze_cjs(cst)
        };

        // === LOAD-004 ===
        if cjs::should_wrap_cjs(&resolved, &graph_path, &analysis) {
            let lowered = runtime_ir_code(&db, fid, module_specifier)?;
            drop(db);
            let plan = cjs::build_plan(&self.resolver, &resolved, &analysis, lowered)?;
            let source = cjs::wrapper_module_source(module_specifier, &plan);
            self.cjs_plans
                .borrow_mut()
                .insert(plan.original_url.as_str().to_owned(), plan);
            return Ok(source);
        }
        // === /LOAD-004 ===

        let code = runtime_ir_code(&db, fid, module_specifier)?;
        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            module_specifier,
            None,
        ))
    }

    // === LOAD-004 ===
    fn ensure_cjs_plan(&self, module_specifier: &ModuleSpecifier) -> Result<(), ModuleLoaderError> {
        if self
            .cjs_plans
            .borrow()
            .contains_key(module_specifier.as_str())
        {
            return Ok(());
        }

        let referrer = self.resolver.project_root().clone();
        let resolved = self
            .resolver
            .resolve(module_specifier.as_str(), &referrer)
            .map_err(resolve_error)?;
        let graph_path = graph_path_for(&resolved.url, module_specifier);
        let mut db = self.graph.borrow_mut();
        let fid = db.set_file(graph_path.clone(), resolved.source.clone());
        let analysis = {
            let cst = db.cst(fid).ok_or_else(|| {
                internal_loader_error(format!(
                    "module {module_specifier} was not interned in the graph"
                ))
            })?;
            meow_graph::analyze_cjs(cst)
        };
        if !cjs::should_wrap_cjs(&resolved, &graph_path, &analysis) {
            return Err(internal_loader_error(format!(
                "module {module_specifier} is not CommonJS"
            )));
        }
        let lowered = runtime_ir_code(&db, fid, module_specifier)?;
        drop(db);
        let plan = cjs::build_plan(&self.resolver, &resolved, &analysis, lowered)?;
        self.cjs_plans
            .borrow_mut()
            .insert(plan.original_url.as_str().to_owned(), plan);
        Ok(())
    }
    // === /LOAD-004 ===
}

fn parse_referrer(referrer: &str, project_root: &Url) -> Url {
    Url::parse(referrer).unwrap_or_else(|_| project_root.clone())
}

fn resolve_error(err: ResolveError) -> ModuleLoaderError {
    ModuleLoaderError::generic(format!("meow: {err}"))
}

fn internal_loader_error(message: String) -> ModuleLoaderError {
    ModuleLoaderError::generic(format!("meow: internal: {message}"))
}

fn runtime_ir_code(
    db: &GraphDb,
    fid: meow_graph::FileId,
    specifier: &ModuleSpecifier,
) -> Result<String, ModuleLoaderError> {
    match db.runtime_ir(fid) {
        Some(Ok(ir)) => Ok(ir.code.to_string()),
        Some(Err(diagnostics)) => Err(graph_error(specifier, diagnostics)),
        None => Err(internal_loader_error(format!(
            "module {specifier} was not interned in the graph"
        ))),
    }
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
    // === RT-007 ===
    if url.scheme() == "node" {
        return PathBuf::from("node-native").join(format!("{}.ts", url.path()));
    }
    // === /RT-007 ===
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
