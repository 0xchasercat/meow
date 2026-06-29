use std::{
    collections::{HashSet, VecDeque},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use meow_graph::{GraphDb, SourceType};
use meow_loader::Resolver;
use oxc_codegen::Codegen;
use oxc_diagnostics::OxcDiagnostic;

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

#[derive(Debug, Clone)]
pub struct BundleReport {
    pub out_dir: PathBuf,
    pub emitted: Vec<PathBuf>,
}

struct MeowResolverPlugin {
    resolver: Resolver,
}

impl std::fmt::Debug for MeowResolverPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeowResolverPlugin").finish_non_exhaustive()
    }
}

impl MeowResolverPlugin {
    fn new(resolver: Resolver) -> Self {
        Self { resolver }
    }

    fn referrer_url(&self, importer: Option<&str>) -> Result<deno_core::url::Url, ToolError> {
        let Some(importer) = importer else {
            return Ok(self.resolver.project_root().clone());
        };

        if let Ok(url) = deno_core::url::Url::parse(importer) {
            return Ok(url);
        }

        let path = Path::new(importer);
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.resolver
                .project_root()
                .to_file_path()
                .unwrap_or_default()
                .join(path)
        };
        deno_core::url::Url::from_file_path(&abs).map_err(|()| {
            ToolError::Message(format!(
                "rolldown importer is not a valid path or URL: {importer}"
            ))
        })
    }

    fn module_type(path: &Path) -> rolldown_common::ModuleType {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("jsx") => rolldown_common::ModuleType::Jsx,
            Some("ts") | Some("mts") | Some("cts") => rolldown_common::ModuleType::Ts,
            Some("tsx") => rolldown_common::ModuleType::Tsx,
            Some("json") => rolldown_common::ModuleType::Json,
            _ => rolldown_common::ModuleType::Js,
        }
    }
}

impl rolldown_plugin::Plugin for MeowResolverPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("rolldown-plugin-meow-resolver")
    }

    fn resolve_id(
        &self,
        _ctx: &rolldown_plugin::PluginContext,
        args: &rolldown_plugin::HookResolveIdArgs<'_>,
    ) -> impl std::future::Future<Output = rolldown_plugin::HookResolveIdReturn> + Send {
        let result = (|| {
            if args.specifier.starts_with("\0") {
                return Ok(None);
            }
            if args.specifier.starts_with("node:")
                || args.specifier.starts_with("meow:")
                || args.specifier.starts_with("ext:")
            {
                return Ok(None);
            }

            let referrer = self.referrer_url(args.importer)?;
            let resolved = self
                .resolver
                .resolve(args.specifier, &referrer)
                .map_err(|err| {
                    ToolError::Message(format!(
                        "cannot resolve {} from {}: {err}",
                        args.specifier, referrer
                    ))
                })?;
            let path = resolved.url.to_file_path().map_err(|()| {
                ToolError::Message(format!(
                    "rolldown resolved {} to unsupported URL {}",
                    args.specifier, resolved.url
                ))
            })?;

            Ok(Some(rolldown_plugin::HookResolveIdOutput::from_id(
                path.to_string_lossy().into_owned(),
            )))
        })()
        .map_err(|err: ToolError| anyhow::anyhow!(err.to_string()));

        async move { result }
    }

    fn load(
        &self,
        _ctx: rolldown_plugin::SharedLoadPluginContext,
        args: &rolldown_plugin::HookLoadArgs<'_>,
    ) -> impl std::future::Future<Output = rolldown_plugin::HookLoadReturn> + Send {
        let result = (|| {
            if args.id.starts_with("\0") {
                return Ok(None);
            }

            let path = PathBuf::from(args.id);
            if !path.is_absolute() || !path.is_file() || !is_target_file(&path) {
                return Ok(None);
            }

            let source = fs::read_to_string(&path).map_err(|source| ToolError::Read {
                path: path.clone(),
                source,
            })?;

            Ok(Some(rolldown_plugin::HookLoadOutput {
                code: source.into(),
                map: None,
                side_effects: None,
                module_type: Some(Self::module_type(&path)),
            }))
        })()
        .map_err(|err: ToolError| anyhow::anyhow!(err.to_string()));

        async move { result }
    }

    fn register_hook_usage(&self) -> rolldown_plugin::HookUsage {
        rolldown_plugin::HookUsage::ResolveId | rolldown_plugin::HookUsage::Load
    }
}

pub async fn execute_bundle(
    resolver: Resolver,
    root: &Path,
    entries: &[PathBuf],
    out_dir: &Path,
) -> Result<BundleReport, ToolError> {
    if entries.is_empty() {
        return Err(ToolError::NoBundleEntries);
    }

    fs::create_dir_all(out_dir).map_err(|source| ToolError::Write {
        path: out_dir.to_path_buf(),
        source,
    })?;

    let inputs = entries
        .iter()
        .map(|entry| {
            let entry_abs = if entry.is_absolute() {
                entry.clone()
            } else {
                root.join(entry)
            };
            let entry_abs = normalize_path(&entry_abs);
            rolldown::InputItem {
                name: entry_abs
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(ToOwned::to_owned),
                import: entry_abs.to_string_lossy().into_owned(),
            }
        })
        .collect::<Vec<_>>();

    let options = rolldown::BundlerOptions {
        input: Some(inputs),
        cwd: Some(root.to_path_buf()),
        dir: Some(out_dir.to_string_lossy().into_owned()),
        platform: Some(rolldown::Platform::Node),
        format: Some(rolldown::OutputFormat::Esm),
        entry_filenames: Some(rolldown::ChunkFilenamesOutputOption::String(
            "[name].js".into(),
        )),
        chunk_filenames: Some(rolldown::ChunkFilenamesOutputOption::String(
            "chunks/[name]-[hash].js".into(),
        )),
        clean_dir: Some(true),
        ..rolldown::BundlerOptions::default()
    };

    let plugin: std::sync::Arc<dyn rolldown_plugin::Pluginable> =
        std::sync::Arc::new(MeowResolverPlugin::new(resolver));
    let mut bundler = rolldown::Bundler::with_plugins(options, vec![plugin])
        .map_err(|err| ToolError::Message(format!("rolldown initialization failed: {err}")))?;
    let output = bundler
        .write()
        .await
        .map_err(|err| ToolError::Message(format!("rolldown write failed: {err}")))?;
    bundler
        .close()
        .await
        .map_err(|err| ToolError::Message(format!("rolldown close failed: {err}")))?;

    let emitted = output
        .assets
        .iter()
        .map(|asset| out_dir.join(asset.filename()))
        .collect();

    Ok(BundleReport {
        out_dir: out_dir.to_path_buf(),
        emitted,
    })
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

        let labels = diag.labels.as_ref();
        if labels.is_empty() {
            out.push(ToolDiagnostic {
                path: path.to_path_buf(),
                source: Arc::clone(source),
                span: (0, 0),
                message,
                label: None,
            });
            continue;
        }

        for label in labels {
            let start: usize = label.offset().try_into().unwrap();
            let len: usize = label.len().try_into().unwrap();
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

    fn jsx_js_file(tmp: &Path) -> PathBuf {
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
        let report = format_paths(
            &tmp,
            std::slice::from_ref(&file),
            FormatOptions { check: true },
        )
        .expect("format_paths should parse jsx js fixture");

        assert_no_parser_blocking_diagnostics(&report.diagnostics);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn lint_paths_supports_js_with_directive_and_jsx() {
        let tmp = tmp_dir("lint-jsx");
        let file = jsx_js_file(&tmp);
        let report = lint_paths(&tmp, std::slice::from_ref(&file))
            .expect("lint_paths should parse jsx js fixture");

        assert_no_parser_blocking_diagnostics(&report.diagnostics);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
