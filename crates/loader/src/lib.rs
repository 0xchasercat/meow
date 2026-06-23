//! `meow-loader` — THE module loader: resolve -> cache/disk read -> graph lowering -> V8.
//!
//! Hosts the one [`Resolver`] the whole toolchain shares (I-1, I-5). The
//! [`deno_core::ModuleLoader`] impl is a thin adapter: deno_core's sync `resolve`
//! delegates to [`Resolver::locate`] (identity only), and `load` re-enters
//! [`Resolver::resolve`] with the already-absolute URL to read the source.
//!
//! CommonJS execution is delegated to Deno's upstream `node:module` /
//! `NodeRequireLoader`: a CJS module is served as a tiny `createRequire` ESM
//! facade (see [`cjs_facade_source`]) instead of a hand-rolled synthetic wrapper.

pub mod package;
mod resolver;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use deno_core::error::ModuleLoaderError;
use deno_core::url::Url;
use deno_core::{
    extension, op2, Extension, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse,
    ModuleLoader, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, OpState,
    ResolutionKind,
};
use meow_graph::GraphDb;
use serde::Serialize;

pub use crate::resolver::{ModuleKind, ModuleLocator, ResolveError, ResolvedModule, Resolver};

#[derive(Clone)]
struct CjsResolveState {
    resolver: Resolver,
}

#[derive(Serialize)]
struct CjsLoadedModule {
    url: String,
    filename: String,
    dirname: String,
    source: String,
    kind: &'static str,
}

/// CommonJS resolution op seam.
///
/// CJS execution itself is owned by Deno's upstream `node:module` /
/// `NodeRequireLoader` (the loader emits a `createRequire` ESM facade — see
/// [`cjs_facade_source`]); this op is retained as the resolver-backed
/// `require.resolve` seam the CLI/runtime wiring installs.
pub fn cjs_resolve_extension(resolver: Resolver) -> Extension {
    let state = CjsResolveState { resolver };
    let mut ext = meow_cjs_resolver::init();
    ext.op_state_fn = Some(Box::new(move |op_state| {
        op_state.put(state.clone());
    }));
    ext
}

#[op2]
#[serde]
fn op_cjs_resolve_and_load(
    state: &mut OpState,
    #[string] specifier: &str,
    #[string] referrer: &str,
) -> Result<CjsLoadedModule, deno_error::JsErrorBox> {
    let state = state.borrow::<CjsResolveState>();
    let referrer_url = parse_referrer(referrer, state.resolver.project_root());
    let resolved = state
        .resolver
        .resolve_require(specifier, &referrer_url)
        .map_err(|err| deno_error::JsErrorBox::generic(format!("meow: {err}")))?;
    if resolved.kind == ModuleKind::Esm {
        return Err(deno_error::JsErrorBox::generic(format!(
            "require() cannot load ES module {}",
            resolved.url
        )));
    }

    let is_napi = match &resolved.locator {
        ModuleLocator::LocalFile(path) => {
            path.extension().and_then(|ext| ext.to_str()) == Some("node")
        }
        ModuleLocator::Cached { member, .. } => {
            Path::new(member).extension().and_then(|ext| ext.to_str()) == Some("node")
        }
        ModuleLocator::Native { .. } => false,
    };
    let filename = match &resolved.locator {
        ModuleLocator::Native { .. } => cjs_filename_for(&resolved.url, &resolved.url),
        _ => state
            .resolver
            .projected_path_for(&resolved.locator)
            .map(Ok)
            .unwrap_or_else(|| state.resolver.runtime_path_for(&resolved.locator))
            .map_err(|err| deno_error::JsErrorBox::generic(format!("meow: {err}")))?
            .to_string_lossy()
            .into_owned(),
    };
    let dirname = PathBuf::from(&filename)
        .parent()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(CjsLoadedModule {
        url: resolved.url.to_string(),
        filename,
        dirname,
        source: if is_napi {
            String::new()
        } else {
            resolved.source.as_ref().to_owned()
        },
        kind: if is_napi {
            "napi"
        } else if resolved.kind == ModuleKind::Json {
            "json"
        } else {
            "cjs"
        },
    })
}

extension!(meow_cjs_resolver, ops = [op_cjs_resolve_and_load]);

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

        // CommonJS: hand execution to Deno's upstream `node:module` /
        // `NodeRequireLoader` via a `createRequire` ESM facade. Named exports are
        // discovered from the Oxc AST (analyze_cjs), never regex-scraped.
        if resolved.kind == ModuleKind::Cjs {
            let named_exports = self.cjs_named_exports(module_specifier, &resolved)?;
            let require_path = self.cjs_require_path(&resolved)?;
            return Ok(cjs_facade_source(
                module_specifier,
                &require_path,
                &named_exports,
            ));
        }

        let graph_path = graph_path_for(&resolved.url, module_specifier);
        let (code, analysis) = {
            let mut db = self.graph.borrow_mut();
            let fid = db.set_file(graph_path, resolved.source.clone());
            let code = runtime_ir_code(&db, fid, module_specifier)?;
            let cst = db.cst(fid).ok_or_else(|| {
                internal_loader_error(format!(
                    "module {module_specifier} was not interned in the graph"
                ))
            })?;
            (code, meow_graph::analyze_cjs(cst))
        };
        // A `.js`/`.mjs` module whose syntax is CommonJS (e.g. a `type: module`
        // package shipping a `.js` CJS file) also routes through the facade.
        if analysis.has_commonjs_syntax && !analysis.has_esm_syntax {
            let require_path = self.cjs_require_path(&resolved)?;
            return Ok(cjs_facade_source(
                module_specifier,
                &require_path,
                &analysis.named_exports,
            ));
        }
        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            module_specifier,
            None,
        ))
    }

    /// AST-derived CommonJS named exports for the ESM facade (replaces the old
    /// regex export-scrape). Interns the source in the graph and runs
    /// [`meow_graph::analyze_cjs`].
    fn cjs_named_exports(
        &self,
        module_specifier: &ModuleSpecifier,
        resolved: &ResolvedModule,
    ) -> Result<Vec<String>, ModuleLoaderError> {
        let graph_path = graph_path_for(&resolved.url, module_specifier);
        let mut db = self.graph.borrow_mut();
        let fid = db.set_file(graph_path, resolved.source.clone());
        let cst = db.cst(fid).ok_or_else(|| {
            internal_loader_error(format!(
                "module {module_specifier} was not interned in the graph"
            ))
        })?;
        Ok(meow_graph::analyze_cjs(cst).named_exports)
    }

    /// Absolute filesystem path Deno's `createRequire`/`require` loads the module
    /// from. Prefers the projected `node_modules` view, falling back to the
    /// content-addressed runtime path (unpacked store / local file).
    fn cjs_require_path(&self, resolved: &ResolvedModule) -> Result<String, ModuleLoaderError> {
        let path = self
            .resolver
            .projected_path_for(&resolved.locator)
            .map(Ok)
            .unwrap_or_else(|| self.resolver.runtime_path_for(&resolved.locator))
            .map_err(resolve_error)?;
        Ok(path.to_string_lossy().into_owned())
    }
}

/// Build the ESM facade that delegates CommonJS execution to Deno's upstream
/// `node:module`. `createRequire(path)(path)` loads and runs the module through
/// `NodeRequireLoader`; Deno reads and compiles the file (not meow), so
/// `require()`, `node:*` builtins and dynamic `import()` all use upstream
/// semantics. Named exports are re-published so `import { x } from "./m.cjs"`
/// keeps resolving.
fn cjs_facade_source(
    module_specifier: &ModuleSpecifier,
    require_path: &str,
    named_exports: &[String],
) -> ModuleSource {
    let path_js =
        serde_json::to_string(require_path).expect("serializing require path as JS string");
    let mut code =
        String::from("import { createRequire as __meowCreateRequire } from \"node:module\";\n");
    code.push_str("const __meowRequire = __meowCreateRequire(");
    code.push_str(&path_js);
    code.push_str(");\nconst __meowCjsExports = __meowRequire(");
    code.push_str(&path_js);
    code.push_str(");\nexport default __meowCjsExports;\n");
    for name in named_exports {
        let key = serde_json::to_string(name).expect("serializing export name as JS string");
        code.push_str("export const ");
        code.push_str(name);
        code.push_str(" = __meowCjsExports[");
        code.push_str(&key);
        code.push_str("];\n");
    }
    javascript_module(module_specifier, code)
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

fn cjs_filename_for(url: &Url, module_specifier: &ModuleSpecifier) -> String {
    if let Ok(path) = url.to_file_path() {
        return path.to_string_lossy().into_owned();
    }
    graph_path_for(url, module_specifier)
        .to_string_lossy()
        .into_owned()
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
