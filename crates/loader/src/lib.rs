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
pub use crate::url::{
    decode as decode_cache_url, encode as encode_cache_url, SCHEME as CACHE_SCHEME,
};

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
        println!(
            "MEOW RESOLVE: specifier={}, referrer={}",
            specifier, referrer
        );
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
        println!("MEOW LOAD: specifier={}", module_specifier);
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
            return Ok(cjs_module_source(
                module_specifier,
                resolved.source.as_ref(),
            ));
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
            let filename = cjs_filename_for(&resolved.url, module_specifier);
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

fn cjs_module_source(module_specifier: &ModuleSpecifier, source: &str) -> ModuleSource {
    let specifier = serde_json::to_string(module_specifier.as_str())
        .expect("serializing module specifier as JS string");
    let mut code = String::from(
        "import { runCjsModule } from \"meow:internal/cjs\";\nconst __meowCjsExports = runCjsModule(",
    );
    code.push_str(&specifier);
    code.push_str(");\nexport default __meowCjsExports;\nexport { __meowCjsExports as __meow_cjs_exports__ };\n");
    for name in cjs_named_exports(source) {
        code.push_str("export const ");
        code.push_str(&name);
        code.push_str(" = __meowCjsExports.");
        code.push_str(&name);
        code.push_str(";\n");
    }
    javascript_module(module_specifier, code)
}

fn cjs_named_exports(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed
            .strip_prefix("exports.")
            .or_else(|| trimmed.strip_prefix("module.exports."))
        {
            let mut end = 0usize;
            for ch in rest.chars() {
                if ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
                    end += ch.len_utf8();
                } else {
                    break;
                }
            }
            if end != 0 {
                let name = &rest[..end];
                if rest[end..].trim_start().starts_with('=')
                    && !names.iter().any(|existing| existing == name)
                {
                    names.push(name.to_owned());
                }
            }
        }

        if line.contains("=>") {
            let mut search = line;
            let mut base = 0usize;
            while let Some(open_rel) = search.find('{') {
                let open = base + open_rel;
                let after_open = open + 1;
                if let Some(close_rel) = line[after_open..].find('}') {
                    let close = after_open + close_rel;
                    let segment = &line[after_open..close];
                    for field in segment.split(',') {
                        let Some((candidate, value)) = field.split_once(':') else {
                            continue;
                        };
                        let candidate = candidate.trim();
                        if !value.contains("=>") || !is_js_identifier(candidate) {
                            continue;
                        }
                        if names.iter().any(|existing| existing == candidate) {
                            continue;
                        }
                        names.push(candidate.to_owned());
                    }
                    if close + 1 >= line.len() {
                        break;
                    }
                    base = close + 1;
                    search = &line[base..];
                } else {
                    break;
                }
            }
        }
    }
    names
}

fn is_js_identifier(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
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

fn cjs_filename_for(url: &Url, module_specifier: &ModuleSpecifier) -> String {
    if let Ok(path) = url.to_file_path() {
        return path.to_string_lossy().into_owned();
    }
    if let Ok((package, member)) = decode_cache_url(url) {
        return PathBuf::from("/meow-cache")
            .join(package.to_url_host())
            .join(member)
            .to_string_lossy()
            .into_owned();
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
