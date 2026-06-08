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

        if resolved.kind == ModuleKind::Json {
            return Ok(ModuleSource::new(
                ModuleType::Json,
                ModuleSourceCode::String(resolved.source.as_ref().to_owned().into()),
                module_specifier,
                None,
            ));
        }

        if resolved.kind == ModuleKind::Cjs {
            return Ok(cjs_module_source(module_specifier));
        }

        let graph_path = graph_path_for(&resolved.url, module_specifier);
        let mut db = self.graph.borrow_mut();
        let fid = db.set_file(graph_path, resolved.source.clone());
        let code = runtime_ir_code(&db, fid, module_specifier)?;
        let analysis = {
            let cst = db.cst(fid).ok_or_else(|| {
                internal_loader_error(format!(
                    "module {module_specifier} was not interned in the graph"
                ))
            })?;
            meow_graph::analyze_cjs(cst)
        };
        if analysis.has_commonjs_syntax && !analysis.has_esm_syntax {
            let filename = resolved
                .url
                .to_file_path()
                .unwrap_or_else(|_| graph_path_for(&resolved.url, module_specifier))
                .to_string_lossy()
                .into_owned();
            let dirname = PathBuf::from(&filename)
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Ok(cjs_inline_module_source(
                module_specifier,
                resolved.url.as_str(),
                &filename,
                &dirname,
                &code,
            ));
        }
        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            module_specifier,
            None,
        ))
    }
}

fn cjs_module_source(module_specifier: &ModuleSpecifier) -> ModuleSource {
    let specifier = serde_json::to_string(module_specifier.as_str())
        .expect("serializing module specifier as JS string");
    javascript_module(
        module_specifier,
        format!(
            "import {{ runCjsModule }} from \"meow:internal/cjs\";\nconst __meowCjsExports = runCjsModule({specifier});\nexport default __meowCjsExports;\nexport {{ __meowCjsExports as __meow_cjs_exports__ }};\n",
        ),
    )
}

fn cjs_inline_module_source(
    module_specifier: &ModuleSpecifier,
    url: &str,
    filename: &str,
    dirname: &str,
    source: &str,
) -> ModuleSource {
    let url = serde_json::to_string(url).expect("serializing module URL as JS string");
    let filename = serde_json::to_string(filename).expect("serializing filename as JS string");
    let dirname = serde_json::to_string(dirname).expect("serializing dirname as JS string");
    let source = serde_json::to_string(source).expect("serializing source as JS string");
    javascript_module(
        module_specifier,
        format!(
            "import {{ runCjsModuleText }} from \"meow:internal/cjs\";\nconst __meowCjsExports = runCjsModuleText({url}, {filename}, {dirname}, {source});\nexport default __meowCjsExports;\nexport {{ __meowCjsExports as __meow_cjs_exports__ }};\n",
        ),
    )
}

fn javascript_module(module_specifier: &ModuleSpecifier, code: String) -> ModuleSource {
    ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(code.into()),
        module_specifier,
        None,
    )
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
        let native_member = url.path().trim_start_matches('/');
        let native_member = if native_member.is_empty() {
            "index"
        } else {
            native_member
        };
        return PathBuf::from("meow-native").join(format!("{}.ts", native_member));
    }
    // === RT-007 ===
    if url.scheme() == "node" {
        let native_member = url.path().trim_start_matches('/');
        let native_member = if native_member.is_empty() {
            "index"
        } else {
            native_member
        };
        return PathBuf::from("node-native").join(format!("{}.ts", native_member));
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
