use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use meow_graph::{GraphDb, SourceType};
use meow_loader::{ResolvedModule, Resolver};
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType as OxcSourceType;
use oxc_transformer::{TransformOptions, Transformer};

#[derive(Debug)]
pub struct ToolDiagnostic {
    pub path: PathBuf,
    pub source: Arc<str>,
    pub span: (usize, usize),
    pub message: String,
    pub label: Option<String>,
}

pub struct LintReport {
    pub checked: usize,
    pub diagnostics: Vec<ToolDiagnostic>,
}

pub fn lint_paths(root: &Path, paths: &[PathBuf]) -> Result<LintReport, ToolError> {
    let files = collect_targets(root, paths)?;
    let mut db = GraphDb::new();
    let mut report = LintReport {
        checked: files.len(),
        diagnostics: Vec::new(),
    };

    for path in files {
        let source = read_source(&path)?;
        let source_type = permissive_source_type(&path);
        let id = db.set_file_with_source_type(&path, Arc::clone(&source), source_type);
        let cst = db.cst(id).ok_or_else(|| ToolError::InconsistentGraph {
            path: path.clone(),
            reason: "missing CST after insertion",
        })?;
        let semantic = db
            .semantic(id)
            .ok_or_else(|| ToolError::InconsistentGraph {
                path: path.clone(),
                reason: "missing semantic stage after CST insertion",
            })?;

        append_diagnostics(&path, &source, cst.errors(), &mut report.diagnostics);
        append_diagnostics(&path, &source, semantic.errors(), &mut report.diagnostics);
        append_starter_lints(&path, &source, &mut report.diagnostics);

        if cst.panicked() {
            report.diagnostics.push(ToolDiagnostic {
                path: path.clone(),
                source: Arc::clone(&source),
                span: (0, 0),
                message: "parser panic while parsing file".to_string(),
                label: None,
            });
        }
    }

    Ok(report)
}

#[derive(Clone, Copy)]
pub struct FormatOptions {
    pub check: bool,
}

pub struct FormatReport {
    pub checked: usize,
    pub changed: Vec<PathBuf>,
    pub diagnostics: Vec<ToolDiagnostic>,
}

pub fn format_paths(
    root: &Path,
    paths: &[PathBuf],
    options: FormatOptions,
) -> Result<FormatReport, ToolError> {
    let files = collect_targets(root, paths)?;
    let mut db = GraphDb::new();
    let mut report = FormatReport {
        checked: files.len(),
        changed: Vec::new(),
        diagnostics: Vec::new(),
    };

    for path in files {
        let source = read_source(&path)?;
        let source_type = permissive_source_type(&path);
        let id = db.set_file_with_source_type(&path, Arc::clone(&source), source_type);
        let cst = db.cst(id).ok_or_else(|| ToolError::InconsistentGraph {
            path: path.clone(),
            reason: "missing CST after insertion",
        })?;

        append_diagnostics(&path, &source, cst.errors(), &mut report.diagnostics);
        if cst.panicked() {
            report.diagnostics.push(ToolDiagnostic {
                path: path.clone(),
                source: Arc::clone(&source),
                span: (0, 0),
                message: "parser panicked while parsing file".to_string(),
                label: None,
            });
            continue;
        }

        if !cst.errors().is_empty() {
            continue;
        }

        let formatted = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Codegen::new().build(cst.program()).code
        })) {
            Ok(formatted) => formatted,
            Err(err) => {
                let message = formatter_panic_message(err);
                report.diagnostics.push(ToolDiagnostic {
                    path: path.clone(),
                    source: Arc::clone(&source),
                    span: (0, 0),
                    message: format!("formatter panicked while formatting file: {message}"),
                    label: None,
                });
                continue;
            }
        };

        if formatted != source.as_ref() {
            report.changed.push(path.clone());
            if !options.check {
                write_source(&path, &formatted)?;
            }
        }
    }

    Ok(report)
}

pub struct BundlePlan {
    pub entries: Vec<PathBuf>,
    pub out: Option<PathBuf>,
}

pub fn plan_bundle(
    root: &Path,
    entries: &[PathBuf],
    out: Option<PathBuf>,
) -> Result<BundlePlan, ToolError> {
    if entries.is_empty() {
        return Err(ToolError::NoBundleEntries);
    }

    let mut normalized_entries = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if entry.as_os_str().is_empty() {
            return Err(ToolError::EmptyBundleEntry { index });
        }
        normalized_entries.push(normalize_path(&resolve_path(root, entry)));
    }

    let out = match out {
        Some(out) => {
            if out.as_os_str().is_empty() {
                return Err(ToolError::EmptyBundleOutput);
            }
            Some(normalize_path(&resolve_path(root, &out)))
        }
        None => None,
    };

    Ok(BundlePlan {
        entries: normalized_entries,
        out,
    })
}

/// A bundled module with its transpiled source and metadata.
#[derive(Clone, Debug)]
struct BundleModule {
    /// Relative module path used as the require key
    id: String,
    /// Transpiled JavaScript source
    source: String,
}

/// Bundle one or more entry modules through the meow resolver. Walks the
/// dependency graph, transpiles TS/JSX, and emits concatenated output files.
///
/// Each entry produces one output file at `out_dir/<entry_stem>.js`. The bundle
/// wraps every module in an IIFE registered against a shared module registry,
/// then evaluates the entry module last.
pub fn bundle_entries(
    resolver: &Resolver,
    root: &Path,
    entries: &[PathBuf],
    out_dir: &Path,
) -> Result<Vec<PathBuf>, ToolError> {
    if entries.is_empty() {
        return Err(ToolError::NoBundleEntries);
    }

    fs::create_dir_all(out_dir).map_err(|source| ToolError::Write {
        path: out_dir.to_path_buf(),
        source,
    })?;

    let mut output_files = Vec::with_capacity(entries.len());
    for entry in entries {
        let entry_abs = if entry.is_absolute() {
            entry.clone()
        } else {
            root.join(entry)
        };
        let entry_url = deno_core::url::Url::from_file_path(&entry_abs)
            .map_err(|_| ToolError::Message(format!("invalid entry path: {}", entry_abs.display())))?;

        // Resolve the entry module
        let resolved = resolver
            .resolve(entry_url.as_str(), &entry_url)
            .map_err(|e| ToolError::Message(format!("cannot resolve entry {}: {e}", entry_abs.display())))?;

        // BFS walk to collect all dependencies
        let mut modules: BTreeMap<String, BundleModule> = BTreeMap::new();
        let mut queue: VecDeque<ResolvedModule> = VecDeque::new();
        queue.push_back(resolved);

        while let Some(modl) = queue.pop_front() {
            let module_id = module_identifier(&modl);
            if modules.contains_key(&module_id) {
                continue;
            }

            // Transpile if needed
            let source = transpile_for_bundle(&modl)?;

            let bundle_mod = BundleModule {
                id: module_id.clone(),
                source,
            };

            // Parse the original source (before transpilation) to discover imports
            let imports = discover_imports(&modl);
            modules.insert(module_id, bundle_mod);

            for import_spec in &imports {
                match resolver.resolve(import_spec, &modl.url) {
                    Ok(dep) => queue.push_back(dep),
                    Err(_) => {
                        // Skip unresolvable imports (external packages, node builtins)
                        continue;
                    }
                }
            }
        }

        // Determine output file name
        let stem = entry_abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("bundle");
        let out_path = out_dir.join(format!("{stem}.js"));

        // Emit the bundle
        let bundle_source = emit_bundle(&modules, &module_identifier_from_url(&entry_url));
        fs::write(&out_path, &bundle_source).map_err(|source| ToolError::Write {
            path: out_path.clone(),
            source,
        })?;

        output_files.push(out_path);
    }

    Ok(output_files)
}

/// Produce a unique module identifier string from a resolved module.
fn module_identifier(modl: &ResolvedModule) -> String {
    module_identifier_from_url(&modl.url)
}

fn module_identifier_from_url(url: &deno_core::url::Url) -> String {
    if url.scheme() == "file" {
        url.path().to_string()
    } else {
        url.as_str().to_string()
    }
}

/// Discover import specifiers from the module's source using a lightweight parse.
/// Returns a list of resolved import/require targets.
fn discover_imports(modl: &ResolvedModule) -> Vec<String> {
    let mut imports = Vec::new();
    // Look for static import and export-from statements
    let source = &modl.source;
    for line in source.lines() {
        let trimmed = line.trim();
        // static import: `import ... from "..."` or `import("...")`
        if let Some(from_pos) = trimmed.find(" from ") {
            if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
                let after_from = &trimmed[from_pos + 6..];
                let spec = extract_string_literal(after_from);
                if let Some(spec) = spec {
                    if !spec.starts_with("node:") && !spec.starts_with("meow:") && !spec.starts_with("ext:") {
                        imports.push(spec);
                    }
                }
            }
        }
        // dynamic import
        if trimmed.starts_with("import(") {
            let after_paren = &trimmed[7..];
            if let Some(end) = after_paren.find(')') {
                let spec = &after_paren[..end].trim().trim_matches('"').trim_matches('\'');
                if !spec.is_empty() && !spec.starts_with("node:") && !spec.starts_with("meow:") {
                    imports.push(spec.to_string());
                }
            }
        }
        // re-export: `export ... from "..."`
        // covered by the ` from ` check above
    }
    imports
}

/// Extract a string literal from after `from ` keyword.
fn extract_string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') {
        s[1..].find('"').map(|end| s[1..=end].trim_end_matches('"').to_string())
    } else if s.starts_with('\'') {
        s[1..].find('\'').map(|end| s[1..=end].trim_end_matches('\'').to_string())
    } else {
        None
    }
}

/// Transpile a module's source to plain JS: strip types via Oxc, then convert
/// ESM import/export syntax to CommonJS equivalents so the bundle works
/// inside `new Function()` (CommonJS context).
fn transpile_for_bundle(modl: &ResolvedModule) -> Result<String, ToolError> {
    let source = modl.source.as_ref();
    let url_str = modl.url.as_str();

    let should_transpile = url_str.ends_with(".ts")
        || url_str.ends_with(".mts")
        || url_str.ends_with(".cts")
        || url_str.ends_with(".tsx")
        || url_str.ends_with(".jsx");

    if !should_transpile {
        // For CJS, wrap with module.exports preservation; for ESM, keep as-is
        return Ok(match modl.kind {
            meow_loader::ModuleKind::Cjs => {
                // Keep the CJS source; the module registry handles it
                source.to_string()
            }
            _ => source.to_string(),
        });
    }

    // Transpile via Oxc
    // Convert URL to file path for Oxc
    let file_path = if url_str.starts_with("file://") {
        match modl.url.to_file_path() {
            Ok(p) => p,
            Err(_) => std::path::PathBuf::from(url_str.trim_start_matches("file://")),
        }
    } else {
        std::path::PathBuf::from(url_str)
    };
    let source_type = OxcSourceType::from_path(&file_path)
        .unwrap_or_else(|_| {
            if url_str.ends_with(".tsx") {
                OxcSourceType::tsx()
            } else if url_str.ends_with(".ts") {
                OxcSourceType::ts()
            } else if url_str.ends_with(".jsx") {
                OxcSourceType::jsx()
            } else {
                OxcSourceType::mjs()
            }
        })
        .with_typescript(true)
        .with_jsx(true);

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type)
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();

    if parsed.panicked || !parsed.errors.is_empty() {
        let msg = parsed
            .errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ToolError::Message(format!(
            "parse error in {url_str}: {msg}"
        )));
    }

    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);

    if !semantic.errors.is_empty() {
        let msg = semantic
            .errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ToolError::Message(format!(
            "semantic error in {url_str}: {msg}"
        )));
    }

    let transform = TransformOptions {
        typescript: oxc_transformer::TypeScriptOptions {
            only_remove_type_imports: true,
            ..oxc_transformer::TypeScriptOptions::default()
        },
        ..TransformOptions::default()
    };
    let transformed =
        Transformer::new(&allocator, &file_path, &transform).build_with_scoping(
            semantic.semantic.into_scoping(),
            &mut program,
        );

    if !transformed.errors.is_empty() {
        let msg = transformed
            .errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(ToolError::Message(format!(
            "transform error in {url_str}: {msg}"
        )));
    }

    let code = Codegen::new().build(&program).code;
    // After stripping types, convert ESM import/export to CommonJS for the
    // module registry runtime (new Function evaluates in CJS context).
    // Resolve relative imports against the module's URL so require() calls
    // use the same absolute paths as the module registry keys.
    let module_path = module_identifier(modl);
    let code = esm_to_cjs(&code, |spec| {
        if spec.starts_with('.') {
            if let Some(parent) = std::path::Path::new(&module_path).parent() {
                let resolved = parent.join(spec);
                // Normalize the path without resolving symlinks (canonicalize
                // would change /tmp → /private/tmp on macOS).
                let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
                for component in resolved.components() {
                    match component {
                        std::path::Component::CurDir => {},
                        std::path::Component::ParentDir => { parts.pop(); },
                        other => { parts.push(other.as_os_str()); },
                    }
                }
                let normalized: std::path::PathBuf = parts.iter().collect();
                normalized.to_string_lossy().into_owned()
            } else {
                spec.to_string()
            }
        } else {
            spec.to_string()
        }
    });
    Ok(code)
}

/// Extract a string literal from a token: removes outer quotes and semicolons.
/// Extract leading whitespace from a line (for indentation preservation).
fn leading_ws(line: &str) -> &str {
    let ws_end = line.find(|c: char| c != ' ' && c != '\t').unwrap_or(line.len());
    &line[..ws_end]
}

fn extract_quoted_string(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') || s.starts_with('\'') {
        let quote = s.chars().next().unwrap();
        // Find matching closing quote
        if let Some(end) = s[1..].find(quote) {
            s[1..=end].trim_end_matches(quote).to_string()
        } else {
            s.trim_matches(quote).to_string()
        }
    } else {
        s.to_string()
    }
}

/// Convert ESM import/export syntax to CommonJS for the bundle runtime.
/// Operates on the full source, tracking brace depth for multi-line function/class
/// declarations so the exports assignment lands after the closing brace.
/// `resolve` maps import specifiers to absolute module IDs for the registry.
fn esm_to_cjs(source: &str, resolve: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(source.len() + 256);
    let chars: Vec<char> = source.chars().collect();
    let len = chars.len();
    let mut pos = 0usize;

    // Helper: read chars until newline, return the line and advance pos.
    let read_line = |p: &mut usize| -> String {
        let start = *p;
        while *p < len && chars[*p] != '\n' && chars[*p] != '\r' {
            *p += 1;
        }
        let line: String = chars[start..*p].iter().collect();
        while *p < len && (chars[*p] == '\n' || chars[*p] == '\r') {
            *p += 1;
        }
        line
    };

    while pos < len {
        let line = read_line(&mut pos);
        let trimmed = line.trim();

        // === import statements ===
        if let Some(rest) = trimmed.strip_prefix("import ") {
            if rest.contains(" from ") {
                let mut parts = rest.splitn(2, " from ");
                let imports = parts.next().unwrap_or("").trim();
                let raw_src = parts.next().unwrap_or("").trim().trim_end_matches(';');
                let src = extract_quoted_string(raw_src);
                if imports.starts_with('{') {
                    let names = imports
                        .trim_start_matches('{')
                        .trim_end_matches('}')
                        .split(',')
                        .map(|s| {
                            let s = s.trim();
                            if let Some(alias) = s.splitn(2, " as ").nth(1) {
                                alias.trim().to_string()
                            } else {
                                s.to_string()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let resolved = resolve(&src);
                    let ws = leading_ws(&line);
                    out.push_str(&format!("{ws}const {{ {names} }} = require({resolved:?});\n"));
                } else if imports.starts_with("* as ") {
                    let ns = imports.trim_start_matches("* as ").trim();
                    let ws = leading_ws(&line);
                    let resolved = resolve(&src);
                    out.push_str(&format!("{ws}const {ns} = require({resolved:?});\n"));
                } else {
                    let name = imports.trim();
                    let ws = leading_ws(&line);
                    if !name.is_empty() && !name.contains('{') && !name.contains('*') {
                        let resolved = resolve(&src);
                        out.push_str(&format!("{ws}const {name} = require({resolved:?}).default || require({resolved:?});\n"));
                    } else {
                        out.push_str(&line);
                        out.push('\n');
                    }
                }
            } else {
                // side-effect import
                let spec = rest.trim().trim_end_matches(';').trim();
                let src = extract_quoted_string(spec);
                let resolved = resolve(&src);
                let ws = leading_ws(&line);
                out.push_str(&format!("{ws}require({resolved:?});\n"));
            }
            continue;
        }

        // === export function / export async function ===
        if trimmed.starts_with("export function ") || trimmed.starts_with("export async function ") {
            let has_async = trimmed.starts_with("export async function ");
            let body = if has_async {
                trimmed.strip_prefix("export async function ").unwrap_or("")
            } else {
                trimmed.strip_prefix("export function ").unwrap_or("")
            };
            let fn_name = body.split(|c| c == '(' || c == ' ' || c == '<').next().unwrap_or("").trim();
            let fn_decl = if has_async { "async function " } else { "function " };
            let ws = leading_ws(&line);

            // Emit the declaration line with "export " stripped
            out.push_str(&format!("{ws}{fn_decl}{}", body));

            // If the declaration ends with "}" (single-line), add exports right away
            if body.trim_end().ends_with('}') {
                out.push('\n');
                out.push_str(&format!("{ws}exports.{fn_name} = {fn_name};\n"));
            } else {
                // Multi-line: read and emit lines until brace depth returns to 0
                out.push('\n');
                let mut brace_depth: i32 = 0;
                // Count braces already on this line (in the body)
                for c in body.chars() {
                    if c == '{' { brace_depth += 1; }
                    else if c == '}' { brace_depth -= 1; }
                }
                while pos < len && brace_depth > 0 {
                    let next_line = read_line(&mut pos);
                    let trimmed_next = next_line.trim();
                    // Check if this line contains an "export " keyword (nested) — skip
                    // Count braces on this line
                    for c in trimmed_next.chars() {
                        if c == '{' { brace_depth += 1; }
                        else if c == '}' { brace_depth -= 1; }
                    }
                    out.push_str(&next_line);
                    out.push('\n');
                }
                out.push_str(&format!("{ws}exports.{fn_name} = {fn_name};\n"));
            }
            continue;
        }

        // === export class ===
        if trimmed.starts_with("export class ") {
            let body = trimmed.strip_prefix("export class ").unwrap_or("");
            let class_name = body.split(|c| c == ' ' || c == '{' || c == '<').next().unwrap_or("").trim();
            let ws = leading_ws(&line);
            out.push_str(&format!("{ws}class {body}"));

            if body.trim_end().ends_with('}') {
                out.push('\n');
                out.push_str(&format!("{ws}exports.{class_name} = {class_name};\n"));
            } else {
                out.push('\n');
                let mut brace_depth: i32 = 0;
                for c in body.chars() {
                    if c == '{' { brace_depth += 1; }
                    else if c == '}' { brace_depth -= 1; }
                }
                while pos < len && brace_depth > 0 {
                    let next_line = read_line(&mut pos);
                    let trimmed_next = next_line.trim();
                    for c in trimmed_next.chars() {
                        if c == '{' { brace_depth += 1; }
                        else if c == '}' { brace_depth -= 1; }
                    }
                    out.push_str(&next_line);
                    out.push('\n');
                }
                out.push_str(&format!("{ws}exports.{class_name} = {class_name};\n"));
            }
            continue;
        }

        // === export default ===
        if let Some(val) = trimmed.strip_prefix("export default ") {
            let val = val.trim_end_matches(';');
            let ws = leading_ws(&line);
            out.push_str(&format!("{ws}module.exports = {val};\n"));
            continue;
        }

        // === export const/let/var ===
        if let Some(rest) = trimmed.strip_prefix("export ") {
            if rest.starts_with("const ") || rest.starts_with("let ") || rest.starts_with("var ") {
                let ws = leading_ws(&line);
                let var_kw = if rest.starts_with("const ") { "const " } else if rest.starts_with("let ") { "let " } else { "var " };
                let after_kw = rest.strip_prefix(var_kw).unwrap_or("");
                let names = after_kw.split('=').next().unwrap_or("").trim();
                out.push_str(&format!("{ws}{rest}\n"));
                for name in names.split(',').map(|s| s.trim()) {
                    if !name.is_empty() {
                        out.push_str(&format!("{ws}exports.{name} = {name};\n"));
                    }
                }
                continue;
            }
            // export { a, b, c }
            if trimmed.starts_with("export {") {
                let ws = leading_ws(&line);
                let body = trimmed.strip_prefix("export ").unwrap_or("");
                out.push_str(&format!("{ws}{body}\n"));
                continue;
            }
            // export * from "x"
            if let Some(rest) = trimmed.strip_prefix("export * from ") {
                let raw = rest.trim().trim_end_matches(';');
                let src = extract_quoted_string(raw);
                let resolved = resolve(&src);
                let ws = leading_ws(&line);
                out.push_str(&format!("{ws}Object.assign(exports, require({resolved:?}));\n"));
                continue;
            }
            // export { a, b } from "x"
            if trimmed.starts_with("export {") && trimmed.contains(" from ") {
                let after_brace = trimmed.trim_start_matches("export {");
                if let Some(end_brace) = after_brace.find('}') {
                    let names = &after_brace[..end_brace];
                    let after = &after_brace[end_brace + 1..];
                    if let Some(from_pos) = after.find(" from ") {
                        let raw = after[from_pos + 6..].trim().trim_end_matches(';');
                        let src = extract_quoted_string(raw);
                        let resolved = resolve(&src);
                        let name_list: String = names.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(", ");
                        let ws = leading_ws(&line);
                        out.push_str(&format!("{ws}const {{ {name_list} }} = require({resolved:?});\n"));
                        continue;
                    }
                }
            }
        }

        // === fallback: pass through unchanged ===
        out.push_str(&line);
        out.push('\n');
    }

    out
}

/// Emit a bundled JS file. Uses JSON.stringify for safe source embedding,
/// then wraps each module in a lightweight CommonJS-like registry.
fn emit_bundle(modules: &BTreeMap<String, BundleModule>, entry_id: &str) -> String {
    let mut output = String::new();
    output.push_str("// meow bundle\n(function(){\n");

    // Build a JSON-serializable map of module IDs to source code
    output.push_str("var __map = ");
    let mut map = serde_json::Map::new();
    for (id, modl) in modules {
        map.insert(
            id.clone(),
            serde_json::Value::String(modl.source.clone()),
        );
    }
    output.push_str(&serde_json::to_string(&map).unwrap_or_default());
    output.push_str(";\n");

    // Require function + module registry
    output.push_str(
        "var __cache = {};\n\
         function __require(id) {\n\
         if (__cache[id]) return __cache[id].exports;\n\
         var mod = { exports: {} };\n\
         __cache[id] = mod;\n\
         var fn = new Function('require','module','exports',__map[id]);\n\
         fn(__require, mod, mod.exports);\n\
         return mod.exports;\n\
         }\n",
    );

    // Require the entry point
    let safe_entry = serde_json::Value::String(entry_id.to_string());
    output.push_str(&format!(
        "var __exports = __require({});\n",
        serde_json::to_string(&safe_entry).unwrap_or_default()
    ));
    output.push_str("})();\n");

    output
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("failed to read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to write {}: {source}", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("bundle requires at least one entry")]
    NoBundleEntries,

    #[error("failed to walk {}: {source}", .path.display())]
    Walk {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("empty input path at index {index}")]
    EmptyPath { index: usize },

    #[error("bundle entry at index {index} is empty")]
    EmptyBundleEntry { index: usize },

    #[error("bundle output path is empty")]
    EmptyBundleOutput,

    #[error("inconsistent graph state for {}: {reason}", .path.display())]
    InconsistentGraph { path: PathBuf, reason: &'static str },

    #[error("{0}")]
    Message(String),
}

fn read_source(path: &Path) -> Result<Arc<str>, ToolError> {
    let source = fs::read_to_string(path).map_err(|source| ToolError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Arc::from(source))
}

fn write_source(path: &Path, source: &str) -> Result<(), ToolError> {
    fs::write(path, source).map_err(|source| ToolError::Write {
        path: path.to_path_buf(),
        source,
    })
}
fn permissive_source_type(path: &Path) -> SourceType {
    SourceType::from_path(path)
        .unwrap_or_default()
        .with_typescript(true)
        .with_jsx(true)
}

fn formatter_panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast::<String>()
        .map(|message| *message)
        .or_else(|payload| {
            payload
                .downcast::<&str>()
                .map(|message| (*message).to_string())
        })
        .unwrap_or_else(|_| "unknown panic".to_string())
}

fn append_diagnostics(
    path: &Path,
    source: &Arc<str>,
    diagnostics: &[OxcDiagnostic],
    out: &mut Vec<ToolDiagnostic>,
) {
    for diag in diagnostics {
        let message = diag.message.to_string();

        if let Some(labels) = diag.labels.as_ref() {
            if labels.is_empty() {
                out.push(ToolDiagnostic {
                    path: path.to_path_buf(),
                    source: Arc::clone(source),
                    span: (0, 0),
                    message: message.clone(),
                    label: None,
                });
            } else {
                for label in labels {
                    let start = label.offset();
                    let len = label.len();
                    let end = start.saturating_add(len);
                    out.push(ToolDiagnostic {
                        path: path.to_path_buf(),
                        source: Arc::clone(source),
                        span: (start, end),
                        message: message.clone(),
                        label: label.label().map(ToString::to_string),
                    });
                }
            }
        } else {
            out.push(ToolDiagnostic {
                path: path.to_path_buf(),
                source: Arc::clone(source),
                span: (0, 0),
                message,
                label: None,
            });
        }
    }
}

// Starter bridge until oxc_linter is available in this pinned Oxc generation.
// This intentionally only scans source bytes for a small recommended-set subset.
fn append_starter_lints(path: &Path, source: &Arc<str>, out: &mut Vec<ToolDiagnostic>) {
    for (start, end) in scan_pattern(source.as_bytes(), b"debugger") {
        out.push(ToolDiagnostic {
            path: path.to_path_buf(),
            source: Arc::clone(source),
            span: (start, end),
            message: "`debugger` statements are not allowed".to_string(),
            label: Some("avoid debugger statements".to_string()),
        });
    }

    let console_len = b"console.log".len();
    for (start, end) in scan_pattern(source.as_bytes(), b"console.log") {
        if start > 0 {
            let prev = source.as_bytes()[start - 1];
            if prev == b'.' {
                continue;
            }
        }

        match source.as_bytes().get(start + console_len).copied() {
            Some(b'(' | b' ' | b'\t' | b'\n' | b'\r' | b';') => {}
            None => continue,
            _ => continue,
        }

        out.push(ToolDiagnostic {
            path: path.to_path_buf(),
            source: Arc::clone(source),
            span: (start, end),
            message: "`console.log` calls are not recommended".to_string(),
            label: Some("avoid console logging".to_string()),
        });
    }
}

fn scan_pattern(source: &[u8], pattern: &[u8]) -> impl Iterator<Item = (usize, usize)> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while let Some(found) = find_from(source, pattern, index) {
        if is_isolated_word(source, found, found + pattern.len()) {
            out.push((found, found + pattern.len()));
        }
        index = found + 1;
    }

    out.into_iter()
}

fn find_from(source: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= source.len() {
        return None;
    }

    source
        .windows(needle.len())
        .enumerate()
        .skip(start)
        .find_map(|(index, window)| if window == needle { Some(index) } else { None })
}

fn is_isolated_word(source: &[u8], start: usize, end: usize) -> bool {
    let before = start == 0 || !is_ident_byte(source[start - 1]);
    let after = if end >= source.len() {
        true
    } else {
        !is_ident_byte(source[end])
    };
    before && after
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

const TOOL_EXTENSIONS: [&str; 8] = ["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];

fn collect_targets(root: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>, ToolError> {
    let mut queue: VecDeque<PathBuf> = if paths.is_empty() {
        vec![normalize_path(root)].into_iter().collect()
    } else {
        let mut list = Vec::with_capacity(paths.len());
        for (index, path) in paths.iter().enumerate() {
            if path.as_os_str().is_empty() {
                return Err(ToolError::EmptyPath { index });
            }
            list.push(normalize_path(&resolve_path(root, path)));
        }
        list.into_iter().collect()
    };

    let mut seen = HashSet::new();
    let mut files = Vec::new();

    while let Some(current) = queue.pop_front() {
        let metadata = fs::metadata(&current).map_err(|source| ToolError::Walk {
            path: current.clone(),
            source,
        })?;

        if has_ignored_dir_component(&current, metadata.is_dir()) {
            continue;
        }

        if metadata.is_dir() {
            let mut entries = fs::read_dir(&current).map_err(|source| ToolError::Walk {
                path: current.clone(),
                source,
            })?;
            let mut raw_entries: Vec<_> = Vec::new();
            while let Some(entry) =
                entries
                    .next()
                    .transpose()
                    .map_err(|source| ToolError::Walk {
                        path: current.clone(),
                        source,
                    })?
            {
                raw_entries.push(entry);
            }

            raw_entries.sort_unstable_by(|a, b| {
                let a = a.file_name();
                let b = b.file_name();
                a.cmp(&b)
            });

            for entry in raw_entries {
                let entry_path = entry.path();
                let meta = fs::metadata(&entry_path).map_err(|source| ToolError::Walk {
                    path: entry_path.clone(),
                    source,
                })?;

                if meta.is_dir() {
                    if let Some(name) = entry_path.file_name() {
                        if is_ignored_dir_name(name) {
                            continue;
                        }
                    }
                    queue.push_back(normalize_path(&entry_path));
                    continue;
                }

                if meta.is_file() && is_target_file(&entry_path) {
                    let normalized = normalize_path(&entry_path);
                    if seen.insert(normalized.clone()) {
                        files.push(normalized);
                    }
                }
            }
            continue;
        }

        if metadata.is_file() && is_target_file(&current) && seen.insert(current.clone()) {
            files.push(current);
        }
    }

    files.sort_unstable();
    files.dedup();
    Ok(files)
}

fn is_target_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let ext = ext.to_ascii_lowercase();
            TOOL_EXTENSIONS.contains(&ext.as_str())
        })
}

fn resolve_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn has_ignored_dir_component(path: &Path, current_is_dir: bool) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            continue;
        };

        let is_current = components.peek().is_none();
        if (current_is_dir || !is_current) && is_ignored_dir_name(name) {
            return true;
        }
    }
    false
}

fn is_ignored_dir_name(name: &std::ffi::OsStr) -> bool {
    let hidden = name.to_string_lossy();
    if hidden.starts_with('.') {
        return true;
    }
    matches!(hidden.as_ref(), "target" | "node_modules" | "vendor")
}

enum ParentAction {
    Pop,
    Keep,
    Ignore,
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let action = match normalized.components().next_back() {
                    Some(Component::Normal(_)) => ParentAction::Pop,
                    Some(Component::ParentDir) | None => ParentAction::Keep,
                    Some(Component::Prefix(_))
                    | Some(Component::RootDir)
                    | Some(Component::CurDir) => ParentAction::Ignore,
                };
                match action {
                    ParentAction::Pop => {
                        normalized.pop();
                    }
                    ParentAction::Keep => normalized.push(".."),
                    ParentAction::Ignore => {}
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TOOL_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    const JSX_JS_SOURCE: &str = r#"'use client';

function Widget() {
  return <Provider />;
}

Widget();
"#;

    fn tmp_dir(tag: &str) -> PathBuf {
        let id = TOOL_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("meow-tool-{tag}-{id}-{}", process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn jsx_js_file(tmp: &PathBuf) -> PathBuf {
        let file = tmp.join("entry.js");
        std::fs::write(&file, JSX_JS_SOURCE).expect("write js fixture");
        file
    }

    #[test]
    fn formatter_panic_message_handles_non_string_payload() {
        assert_eq!(
            formatter_panic_message(Box::new("boom")),
            "boom".to_string()
        );
        assert_eq!(
            formatter_panic_message(Box::new(String::from("kapow"))),
            "kapow".to_string()
        );
        assert_eq!(
            formatter_panic_message(Box::new(std::fmt::Error)),
            "unknown panic".to_string()
        );
    }
    fn assert_no_parser_blocking_diagnostics(diagnostics: &[ToolDiagnostic]) {
        assert!(
            diagnostics.iter().all(|diag| {
                !diag.message.contains("parser panicked while parsing file")
                    && !diag.message.contains("parser panic while parsing file")
                    && !diag.message.contains("Unexpected JSX expression")
            }),
            "unexpected parser diagnostics from js with JSX: {diagnostics:?}"
        );
    }

    #[test]
    fn format_paths_supports_js_with_directive_and_jsx() {
        let tmp = tmp_dir("fmt-jsx");
        let file = jsx_js_file(&tmp);
        let report = format_paths(&tmp, &vec![file.clone()], FormatOptions { check: true })
            .expect("format_paths should parse jsx js fixture");

        assert_no_parser_blocking_diagnostics(&report.diagnostics);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn lint_paths_supports_js_with_directive_and_jsx() {
        let tmp = tmp_dir("lint-jsx");
        let file = jsx_js_file(&tmp);
        let report =
            lint_paths(&tmp, &vec![file.clone()]).expect("lint_paths should parse jsx js fixture");

        assert_no_parser_blocking_diagnostics(&report.diagnostics);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
