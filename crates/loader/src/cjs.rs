use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use deno_core::error::ModuleLoaderError;
use deno_core::url::Url;
use deno_core::{ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType};
use meow_graph::CjsAnalysis;

use crate::{ModuleKind, ModuleLocator, ResolvedModule, Resolver};

const HELPER_SPECIFIER: &str = "meow:internal/cjs";
const REGISTRY_FRAGMENT: &str = "meow-cjs-registry";
const EXECUTOR_FRAGMENT: &str = "meow-cjs-executor";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompanionKind {
    Registry,
    Executor,
}

#[derive(Debug, Clone)]
pub(crate) struct CjsPlan {
    pub original_url: Url,
    pub registry_url: Url,
    pub executor_url: Url,
    pub filename: String,
    pub dirname: String,
    pub lowered_source: String,
    pub named_exports: Vec<String>,
    pub requires: Vec<CjsRequirePlan>,
}

#[derive(Debug, Clone)]
pub(crate) struct CjsRequirePlan {
    pub requested: String,
    pub target: CjsRequireTarget,
}

#[derive(Debug, Clone)]
pub(crate) enum CjsRequireTarget {
    SelfModule,
    JSImport {
        url: Url,
    },
    CJSImport {
        registry_url: Url,
        executor_url: Url,
    },
    Json {
        cache_key: String,
        source: Arc<str>,
    },
}

pub(crate) fn parse_companion_url(url: &Url) -> Option<(Url, CompanionKind)> {
    let kind = match url.fragment()? {
        REGISTRY_FRAGMENT => CompanionKind::Registry,
        EXECUTOR_FRAGMENT => CompanionKind::Executor,
        _ => return None,
    };
    let mut original = url.clone();
    original.set_fragment(None);
    Some((original, kind))
}

pub(crate) fn should_wrap_cjs(
    resolved: &ResolvedModule,
    graph_path: &Path,
    analysis: &CjsAnalysis,
) -> bool {
    if resolved.kind == ModuleKind::Cjs {
        return true;
    }
    matches!(resolved.locator, ModuleLocator::LocalFile(_))
        && matches!(
            graph_path.extension().and_then(|ext| ext.to_str()),
            Some("ts") | Some("cts")
        )
        && analysis.has_commonjs_syntax
        && !analysis.has_esm_syntax
}

pub(crate) fn build_plan(
    resolver: &Resolver,
    resolved: &ResolvedModule,
    analysis: &CjsAnalysis,
    lowered_source: String,
) -> Result<CjsPlan, ModuleLoaderError> {
    let runtime_path = resolver
        .runtime_path_for(&resolved.locator)
        .map_err(super::resolve_error)?;
    let filename = runtime_path.to_string_lossy().into_owned();
    let dirname = runtime_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .into_owned();
    let original_url = resolved.url.clone();
    let registry_url = companion_url(&original_url, CompanionKind::Registry);
    let executor_url = companion_url(&original_url, CompanionKind::Executor);

    let mut requires = Vec::with_capacity(analysis.static_requires.len());
    for requested in &analysis.static_requires {
        let dep = resolver
            .resolve_require(requested, &original_url)
            .map_err(super::resolve_error)?;
        let target = if dep.url == original_url {
            CjsRequireTarget::SelfModule
        } else {
            match dep.kind {
                ModuleKind::Json => CjsRequireTarget::Json {
                    cache_key: dep.url.to_string(),
                    source: dep.source.clone(),
                },
                ModuleKind::Esm => CjsRequireTarget::JSImport {
                    url: dep.url.clone(),
                },
                ModuleKind::Cjs => CjsRequireTarget::CJSImport {
                    registry_url: companion_url(&dep.url, CompanionKind::Registry),
                    executor_url: companion_url(&dep.url, CompanionKind::Executor),
                },
            }
        };
        requires.push(CjsRequirePlan {
            requested: requested.clone(),
            target,
        });
    }

    Ok(CjsPlan {
        original_url,
        registry_url,
        executor_url,
        filename,
        dirname,
        lowered_source: normalize_cjs_body(lowered_source),
        named_exports: analysis.named_exports.clone(),
        requires,
    })
}

pub(crate) fn companion_module_source(
    specifier: &ModuleSpecifier,
    kind: CompanionKind,
    plan: &CjsPlan,
) -> ModuleSource {
    let code = match kind {
        CompanionKind::Registry => registry_source(plan),
        CompanionKind::Executor => executor_source(plan),
    };
    javascript_module(specifier, code)
}

pub(crate) fn wrapper_module_source(specifier: &ModuleSpecifier, plan: &CjsPlan) -> ModuleSource {
    javascript_module(specifier, wrapper_source(plan))
}

fn companion_url(original: &Url, kind: CompanionKind) -> Url {
    let mut url = original.clone();
    url.set_fragment(Some(match kind {
        CompanionKind::Registry => REGISTRY_FRAGMENT,
        CompanionKind::Executor => EXECUTOR_FRAGMENT,
    }));
    url
}

fn wrapper_source(plan: &CjsPlan) -> String {
    let mut exports = vec![
        "__meowCjsExports as default".to_owned(),
        "__meowCjsExports as __meow_cjs_exports__".to_owned(),
    ];
    exports.extend(plan.named_exports.iter().cloned());
    format!(
        "import {{ __meowExecute }} from {};\n__meowExecute();\nexport {{ {} }} from {};\n",
        js_string(plan.executor_url.as_str()),
        exports.join(", "),
        js_string(plan.registry_url.as_str()),
    )
}

fn registry_source(plan: &CjsPlan) -> String {
    let mut out = String::new();
    let names_array = string_array(&plan.named_exports);
    let _ = writeln!(
        out,
        "import {{ createNamedExportsProxy, syncNamedExports }} from {};",
        js_string(HELPER_SPECIFIER)
    );
    let _ = writeln!(out, "const __meowNames = {};", names_array);
    for name in &plan.named_exports {
        let _ = writeln!(out, "export var {} = undefined;", name);
    }
    out.push_str("function __meowSetNamed(name, value) {\n  switch (name) {\n");
    for name in &plan.named_exports {
        let _ = writeln!(
            out,
            "    case {}: {} = value; return;",
            js_string(name),
            name
        );
    }
    out.push_str("    default: return;\n  }\n}\n");
    out.push_str("export function __meowSyncNamedFrom(value) {\n  syncNamedExports(value, __meowNames, __meowSetNamed);\n}\n");
    out.push_str(
        "export var __meowCjsExports = createNamedExportsProxy(__meowNames, __meowSetNamed);\n",
    );
    out.push_str(
        "export { __meowCjsExports as default, __meowCjsExports as __meow_cjs_exports__ };\n",
    );
    out.push_str("export function __meowSetDefault(value) {\n");
    out.push_str(
        "  __meowCjsExports = createNamedExportsProxy(__meowNames, __meowSetNamed, value);\n",
    );
    out.push_str("  __meowSyncNamedFrom(__meowCjsExports);\n");
    out.push_str("  return __meowCjsExports;\n}\n");
    out
}

fn executor_source(plan: &CjsPlan) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "import {{ __meowCjsExports, __meowSetDefault, __meowSyncNamedFrom }} from {};",
        js_string(plan.registry_url.as_str())
    );
    let _ = writeln!(
        out,
        "import {{ createCjsModule, dynamicRequire, requireFromNamespace, requireJson }} from {};",
        js_string(HELPER_SPECIFIER)
    );

    let mut imports = Vec::<(String, String)>::new();
    let mut binding_by_url = HashMap::<String, String>::new();
    let mut exec_imports = Vec::<(String, String)>::new();
    let mut exec_binding_by_url = HashMap::<String, String>::new();
    for req in &plan.requires {
        match &req.target {
            CjsRequireTarget::JSImport { url } => {
                let key = url.as_str().to_owned();
                binding_by_url.entry(key.clone()).or_insert_with(|| {
                    let binding = format!("__meowDep{}", imports.len());
                    imports.push((binding.clone(), key));
                    binding
                });
            }
            CjsRequireTarget::CJSImport {
                registry_url,
                executor_url,
            } => {
                let registry_key = registry_url.as_str().to_owned();
                binding_by_url
                    .entry(registry_key.clone())
                    .or_insert_with(|| {
                        let binding = format!("__meowDep{}", imports.len());
                        imports.push((binding.clone(), registry_key));
                        binding
                    });
                let exec_key = executor_url.as_str().to_owned();
                exec_binding_by_url
                    .entry(exec_key.clone())
                    .or_insert_with(|| {
                        let binding = format!("__meowExec{}", exec_imports.len());
                        exec_imports.push((binding.clone(), exec_key));
                        binding
                    });
            }
            CjsRequireTarget::SelfModule | CjsRequireTarget::Json { .. } => {}
        }
    }
    for (binding, url) in &imports {
        let _ = writeln!(out, "import * as {} from {};", binding, js_string(url));
    }
    for (binding, url) in &exec_imports {
        let _ = writeln!(
            out,
            "import {{ __meowExecute as {} }} from {};",
            binding,
            js_string(url),
        );
    }

    out.push_str("const __meowRequire = (specifier) => {\n");
    out.push_str(
        "  const key = typeof specifier === \"string\" ? specifier : String(specifier);\n",
    );
    out.push_str("  switch (key) {\n");
    for req in &plan.requires {
        let _ = write!(out, "    case {}: return ", js_string(&req.requested));
        match &req.target {
            CjsRequireTarget::SelfModule => {
                out.push_str("__meowCjsExports;\n");
            }
            CjsRequireTarget::JSImport { url } => {
                let binding = binding_by_url
                    .get(url.as_str())
                    .expect("import binding present for JSImport");
                let _ = writeln!(out, "requireFromNamespace({});", binding);
            }
            CjsRequireTarget::CJSImport {
                registry_url,
                executor_url,
            } => {
                let binding = binding_by_url
                    .get(registry_url.as_str())
                    .expect("import binding present for CJSImport");
                let exec = exec_binding_by_url
                    .get(executor_url.as_str())
                    .expect("executor binding present for CJSImport");
                let _ = writeln!(out, "({}(), requireFromNamespace({}));", exec, binding);
            }
            CjsRequireTarget::Json { cache_key, source } => {
                let _ = writeln!(
                    out,
                    "requireJson({}, {});",
                    js_string(cache_key),
                    js_string(source),
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "    default: return dynamicRequire(key, {});",
        js_string(&plan.filename)
    );
    out.push_str("  }\n};\n");
    let _ = writeln!(
        out,
        "const {{ module, exports, start, finish, abort, current }} = createCjsModule(__meowCjsExports, __meowRequire, {}, {}, __meowSetDefault, __meowSyncNamedFrom);",
        js_string(&plan.filename),
        js_string(&plan.dirname),
    );
    let _ = writeln!(
        out,
        "const __meowFactory = Function(\"exports\", \"require\", \"module\", \"__filename\", \"__dirname\", {});",
        js_string(&plan.lowered_source),
    );
    out.push_str("export function __meowExecute() {\n");
    out.push_str("  if (!start()) {\n");
    out.push_str("    return current();\n");
    out.push_str("  }\n");
    out.push_str("  try {\n");
    let _ = writeln!(
        out,
        "    __meowFactory.call(exports, exports, __meowRequire, module, {}, {});",
        js_string(&plan.filename),
        js_string(&plan.dirname),
    );
    out.push_str("    finish();\n");
    out.push_str("  } catch (error) {\n");
    out.push_str("    abort();\n");
    out.push_str("    throw error;\n");
    out.push_str("  }\n");
    out.push_str("  return current();\n");
    out.push_str("}\n");
    out
}

fn normalize_cjs_body(mut body: String) -> String {
    let offset = if body.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    if body[offset..].starts_with("#!") {
        body.replace_range(offset..offset + 2, "//");
    }
    body
}

fn javascript_module(specifier: &ModuleSpecifier, code: String) -> ModuleSource {
    ModuleSource::new(
        ModuleType::JavaScript,
        ModuleSourceCode::String(code.into()),
        specifier,
        None,
    )
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing JS string literal")
}

fn string_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(&js_string(value));
    }
    out.push(']');
    out
}
