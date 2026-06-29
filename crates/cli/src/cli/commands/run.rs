//! `meow run` / `dev` / `task` / `node`-shim — drive one ESM module to completion
//! through V8. Hosts the shared [`RuntimeNodeBridge`], [`build_runtime_context`],
//! [`run_native_request`] and [`RunFlagView`] used by `test` and `x` as well.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use meow_runtime::node::{
    FastString, InNpmPackageChecker, JsErrorBox, NodeRequireLoader, NpmPackageFolderResolver,
    PackageFolderResolveError, PackageJsonLoadError, PermissionsContainer, Url, UrlOrPathRef,
    Version as DenoVersion,
};
use node_resolver::errors::{PackageFolderResolveErrorKind, PackageNotFoundError};

use crate::cli::{
    cold_start, find_project_root, hiss, load_lockfile, meow_version, mode_label, ui,
    NodeEvalArgs, ResidualLazySources, RunArgs, RunScriptArgs, TaskArgs,
};
use crate::host;

// === RT-006 ===
#[derive(Debug, Clone, Copy)]
pub(super) struct RunFlagView<'a> {
    pub argv: &'a [String],
    pub allow_clock: bool,
    pub allow_random: bool,
    pub allow_env: &'a Option<String>,
    pub trust: bool,
    pub max_old_space_size: Option<usize>,
    pub no_snapshot: bool,
    pub v8_flags: Option<&'a str>,
}
fn run_flags(args: &RunArgs) -> RunFlagView<'_> {
    RunFlagView {
        argv: &args.argv,
        allow_clock: args.allow_clock || args.trust,
        allow_random: args.allow_random || args.trust,
        allow_env: &args.allow_env,
        trust: args.trust,
        max_old_space_size: args.max_old_space_size.or_else(env_max_old_space_size),
        no_snapshot: args.no_snapshot,
        v8_flags: args.v8_flags.as_deref(),
    }
}
// === RUN-001 ===
fn run_script_flags(args: &RunScriptArgs) -> RunFlagView<'_> {
    RunFlagView {
        argv: &args.argv,
        allow_clock: args.allow_clock || args.trust,
        allow_random: args.allow_random || args.trust,
        allow_env: &args.allow_env,
        trust: args.trust,
        max_old_space_size: args.max_old_space_size.or_else(env_max_old_space_size),
        no_snapshot: args.no_snapshot,
        v8_flags: args.v8_flags.as_deref(),
    }
}
// === /RUN-001 ===

/// Build the raw hermetic (clock/rng/env) config from the `meow run` grant flags.
/// Mode-specific CLI defaults are applied by `run_hermetic_config`.
fn hermetic_config(args: &RunFlagView<'_>) -> meow_runtime::hermetic::HermeticConfig {
    let mut cfg = meow_runtime::hermetic::HermeticConfig::default();
    if args.trust || args.allow_clock {
        cfg = cfg.with_real_clock();
    }
    if args.trust || args.allow_random {
        cfg = cfg.with_os_rng();
    }
    if args.trust {
        cfg = cfg.with_env_all();
    } else if let Some(names) = args.allow_env {
        cfg = if names.is_empty() {
            cfg.with_env_all()
        } else {
            cfg.with_env_allow(
                names
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            )
        };
    }
    cfg
}
// === /RT-006 ===

// === RT-007 ===
fn runtime_mode(project_dir: &Path) -> Result<meow_runtime::node::NodeMode, String> {
    match meow_config::MeowConfig::load(project_dir) {
        Ok(cfg) => Ok(match cfg.mode {
            meow_config::Mode::StrictWeb => meow_runtime::node::NodeMode::StrictWeb,
            meow_config::Mode::NodeCompat | meow_config::Mode::Legacy => {
                meow_runtime::node::NodeMode::Enabled
            }
        }),
        Err(meow_config::ConfigError::NotFound(_)) => Ok(meow_runtime::node::NodeMode::Enabled),
        Err(err) => Err(err.to_string()),
    }
}

fn run_hermetic_config(
    args: &RunFlagView<'_>,
    mode: meow_runtime::node::NodeMode,
) -> meow_runtime::hermetic::HermeticConfig {
    let mut cfg = hermetic_config(args);
    if matches!(mode, meow_runtime::node::NodeMode::Enabled) {
        cfg = cfg.with_real_clock().with_os_rng();
        if args.allow_env.is_none() {
            cfg = cfg.with_env_all();
        }
    }
    if matches!(mode, meow_runtime::node::NodeMode::StrictWeb) {
        cfg.env = meow_runtime::hermetic::EnvPolicy::Deny;
    }
    cfg
}
// === /RT-007 ===

// === RUN-001 ===
#[derive(Debug, thiserror::Error)]
pub(super) enum RunCommandError {
    #[error("cannot resolve the current directory: {0}")]
    CurrentDir(#[source] std::io::Error),
    #[error("cannot start the async runtime: {0}")]
    AsyncRuntime(#[source] std::io::Error),
    #[error("cannot find {target}: {source}")]
    MissingEntry {
        target: String,
        #[source]
        source: std::io::Error,
    },
    #[error("entry is not a file: {0}")]
    EntryNotFile(String),
    #[error("invalid entry path {0}")]
    InvalidEntryPath(String),
    #[error("invalid project directory {0}")]
    InvalidProjectDirectory(String),
    #[error(transparent)]
    PackageJson(#[from] meow_config::ConfigError),
    #[error(transparent)]
    Lockfile(#[from] meow_pkg::LockError),
    #[error("cannot read cached manifest {path}: {source}")]
    CachedManifestRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cached manifest {path} is invalid: {source}")]
    CachedManifestParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("package `{package}` declares an invalid bin target `{target}`")]
    InvalidBinTarget { package: String, target: String },
    #[error("package `{package}` bin `{command}` points at missing file `{target}`")]
    MissingBinFile {
        package: String,
        command: String,
        target: String,
    },
    #[error("bin `{command}` is ambiguous across direct dependencies: {packages}")]
    AmbiguousBin { command: String, packages: String },
    #[error("cannot unpack cached package `{package}`: {reason}")]
    UnpackBin { package: String, reason: String },
    #[error("cannot spawn {shell}: {source}")]
    ShellSpawn {
        shell: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("the shell terminated without an exit code")]
    ShellTerminated,
    #[error("{0}")]
    Message(String),
}

impl From<String> for RunCommandError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

#[derive(Clone)]
pub(super) struct RuntimeContext {
    pub project_dir: PathBuf,
    pub project_root: meow_runtime::ModuleSpecifier,
    pub node_mode: meow_runtime::node::NodeMode,
    pub graph: std::sync::Arc<meow_pkg::ResolutionGraph>,
    pub cache: std::sync::Arc<meow_pkg::Cache>,
}

#[derive(Debug, Clone)]
pub(super) struct NativeRunRequest {
    pub project_dir: PathBuf,
    pub process_cwd: PathBuf,
    pub spec: meow_runtime::ModuleSpecifier,
    pub main_module: Option<String>,
    pub argv1: Option<String>,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone)]
enum PlannedScript {
    Native(NativeRunRequest),
    Shell { script: String, argv: Vec<String> },
}

// === /RUN-001 ===

// === RT-001 ===
/// `meow run <name-or-file>`: prefer a package.json script when present; otherwise
/// canonicalize the named entry -> `file:` URL -> drive one module to completion
/// through V8. The binary edge owns host access (cwd/package root lookup) and error
/// rendering; `meow-runtime` stays free of ambient reads (I-6).
pub fn cmd_node_eval(args: NodeEvalArgs) -> ExitCode {
    if args.mode == "--interactive" {
        hiss("node: interactive REPL mode is not supported by the meow node shim");
        return ExitCode::FAILURE;
    }
    let Some(code) = args.code else {
        hiss("node: eval requires source code");
        return ExitCode::FAILURE;
    };
    let print = args.mode == "-p" || args.mode == "--print";
    let source = if print {
        format!("console.log({code});")
    } else {
        code
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            hiss(&format!("node: cannot resolve current directory: {err}"));
            return ExitCode::FAILURE;
        }
    };
    let project_dir = find_project_root(&cwd);
    static EVAL_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let eval_path = std::env::temp_dir().join(format!(
        "meow-node-eval-{}-{}.mjs",
        std::process::id(),
        EVAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    if let Err(err) = std::fs::write(&eval_path, source) {
        hiss(&format!(
            "node: cannot write eval module {}: {err}",
            eval_path.display()
        ));
        return ExitCode::FAILURE;
    }
    let spec = match meow_runtime::ModuleSpecifier::from_file_path(&eval_path) {
        Ok(spec) => spec,
        Err(()) => {
            std::fs::remove_file(&eval_path).ok();
            hiss(&format!(
                "node: invalid eval module path {}",
                eval_path.display()
            ));
            return ExitCode::FAILURE;
        }
    };
    let request = NativeRunRequest {
        project_dir,
        process_cwd: cwd,
        spec,
        main_module: None,
        argv1: None,
        argv: args.argv,
    };
    let flags = RunFlagView {
        argv: &[],
        allow_clock: false,
        allow_random: false,
        allow_env: &None,
        trust: false,
        max_old_space_size: env_max_old_space_size(),
        no_snapshot: false,
        v8_flags: None,
    };
    let code = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => match rt.block_on(run_native_request(
            &request,
            flags,
            host_env_map(flags.allow_env, meow_runtime::node::NodeMode::Enabled),
        )) {
            Ok(code) => code,
            Err(err) => {
                hiss(&format!("node: {err}"));
                ExitCode::FAILURE
            }
        },
        Err(err) => {
            hiss(&format!("node: cannot start async runtime: {err}"));
            ExitCode::FAILURE
        }
    };
    std::fs::remove_file(&eval_path).ok();
    code
}

pub fn cmd_run(args: &RunArgs) -> ExitCode {
    cmd_run_inner("run", &args.target, run_flags(args))
}

// === RUN-001 ===
pub fn cmd_dev(args: &RunScriptArgs) -> ExitCode {
    let mode = std::env::current_dir()
        .ok()
        .and_then(|cwd| runtime_mode(&cwd).ok())
        .map(mode_label)
        .unwrap_or("node-compat");
    ui().dev_banner(meow_version(), mode, "dev", cold_start());
    cmd_run_inner("dev", "dev", run_script_flags(args))
}

fn cmd_run_inner(verb: &'static str, target: &str, flags: RunFlagView<'_>) -> ExitCode {
    match cmd_run_result(target, flags) {
        Ok(code) => code,
        Err(err) => {
            hiss(&format!("meow {verb}: {err}"));
            ExitCode::FAILURE
        }
    }
}

fn cmd_run_result(target: &str, flags: RunFlagView<'_>) -> Result<ExitCode, RunCommandError> {
    let cwd = std::env::current_dir().map_err(RunCommandError::CurrentDir)?;
    let async_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(RunCommandError::AsyncRuntime)?;
    async_rt.block_on(async move {
        if let Some(project_dir) = find_package_root(&cwd) {
            let package_json = meow_config::PackageJson::read(&project_dir)?;
            if package_json.scripts.contains_key(target) {
                return execute_package_script(&package_json, &project_dir, &cwd, target, flags)
                    .await;
            }
        }

        let request = prepare_direct_file_run(&cwd, target, flags.argv)?;
        run_native_request(
            &request,
            flags,
            host_env_map(flags.allow_env, meow_runtime::node::NodeMode::Enabled),
        )
        .await
    })
}

async fn execute_package_script(
    package_json: &meow_config::PackageJson,
    project_dir: &Path,
    init_cwd: &Path,
    script_name: &str,
    flags: RunFlagView<'_>,
) -> Result<ExitCode, RunCommandError> {
    let ctx = build_runtime_context(project_dir, false)?;
    let pre_name = format!("pre{script_name}");
    if let Some(pre_script) = package_json.scripts.get(&pre_name) {
        let status = execute_script_body(&ctx, init_cwd, &pre_name, pre_script, &[], flags).await?;
        if status != ExitCode::SUCCESS {
            return Ok(status);
        }
    }

    let main_script = package_json
        .scripts
        .get(script_name)
        .ok_or_else(|| RunCommandError::Message(format!("missing script `{script_name}`")))?;
    let status =
        execute_script_body(&ctx, init_cwd, script_name, main_script, flags.argv, flags).await?;
    if status != ExitCode::SUCCESS {
        return Ok(status);
    }

    let post_name = format!("post{script_name}");
    if let Some(post_script) = package_json.scripts.get(&post_name) {
        return execute_script_body(&ctx, init_cwd, &post_name, post_script, &[], flags).await;
    }

    Ok(ExitCode::SUCCESS)
}

async fn execute_script_body(
    ctx: &RuntimeContext,
    init_cwd: &Path,
    event: &str,
    script: &str,
    cli_argv: &[String],
    flags: RunFlagView<'_>,
) -> Result<ExitCode, RunCommandError> {
    let env = lifecycle_env(event, script, &ctx.project_dir, init_cwd, flags.allow_env)?;
    match plan_script(ctx, script, cli_argv)? {
        PlannedScript::Native(request) => run_native_request(&request, flags, env).await,
        PlannedScript::Shell { script, argv } => {
            run_shell_command(&ctx.project_dir, &script, &argv, env)
        }
    }
}

pub(super) struct RuntimeNodeBridge {
    resolver: meow_loader::Resolver,
    store: meow_pkg::UnpackedStore,
}

impl RuntimeNodeBridge {
    pub(super) fn new(
        resolver: meow_loader::Resolver,
        cache: std::sync::Arc<meow_pkg::Cache>,
    ) -> RuntimeNodeBridge {
        RuntimeNodeBridge {
            resolver,
            store: meow_pkg::UnpackedStore::new(cache.root().join("unpacked"), cache),
        }
    }

    fn package_folder_error(kind: PackageFolderResolveErrorKind) -> PackageFolderResolveError {
        PackageFolderResolveError(Box::new(kind))
    }

    fn missing_package(
        &self,
        package_name: &str,
        referrer: &UrlOrPathRef,
    ) -> PackageFolderResolveError {
        self.missing_package_with_extra(package_name, referrer, None)
    }

    fn missing_package_with_extra(
        &self,
        package_name: &str,
        referrer: &UrlOrPathRef,
        referrer_extra: Option<String>,
    ) -> PackageFolderResolveError {
        Self::package_folder_error(PackageFolderResolveErrorKind::PackageNotFound(
            PackageNotFoundError {
                package_name: package_name.to_owned(),
                referrer: referrer.display(),
                referrer_extra,
            },
        ))
    }

    fn referrer_url(&self, referrer: &UrlOrPathRef) -> Result<Url, PackageFolderResolveError> {
        referrer.url().cloned().map_err(|err| {
            Self::package_folder_error(PackageFolderResolveErrorKind::PathToUrl(err))
        })
    }

    fn module_kind(&self, specifier: &Url) -> Option<meow_loader::ModuleKind> {
        let resolved = self
            .resolver
            .resolve_require(specifier.as_str(), specifier)
            .ok()?;
        Some(resolved.kind)
    }

    fn projected_referrer_path(&self, path: &Path) -> Option<PathBuf> {
        let referrer = Url::from_file_path(path).ok()?;
        let resolved = self.resolver.resolve(referrer.as_str(), &referrer).ok()?;
        self.resolver.projected_path_for(&resolved.locator)
    }
}

fn nearest_package_root(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    while let Some(dir) = current {
        if dir.join("package.json").is_file() {
            return Some(dir.to_path_buf());
        }
        if dir.ends_with("node_modules") {
            return None;
        }
        current = dir.parent();
    }
    None
}

fn push_node_module_paths(paths: &mut Vec<String>, from: &Path) {
    let mut current_path = from;
    let mut maybe_parent = Some(current_path);
    while let Some(parent) = maybe_parent {
        if !parent.ends_with("node_modules") {
            let candidate = parent.join("node_modules").to_string_lossy().into_owned();
            if !paths.contains(&candidate) {
                paths.push(candidate);
            }
        }
        current_path = parent;
        maybe_parent = current_path.parent();
    }
}

impl NpmPackageFolderResolver for RuntimeNodeBridge {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        referrer: &UrlOrPathRef,
    ) -> Result<PathBuf, PackageFolderResolveError> {
        let referrer_url = self.referrer_url(referrer)?;
        if let Ok(package_root) = self
            .resolver
            .package_root_for_require(specifier, &referrer_url)
        {
            return Ok(package_root);
        }
        let resolved = self
            .resolver
            .resolve_require(specifier, &referrer_url)
            .map_err(|err| {
                self.missing_package_with_extra(specifier, referrer, Some(err.to_string()))
            })?;

        let package_root = match resolved.locator {
            meow_loader::ModuleLocator::Cached { ref package, .. } => {
                if let Some(projected) = self.resolver.projected_path_for(&resolved.locator) {
                    nearest_package_root(&projected)
                        .or_else(|| projected.parent().map(Path::to_path_buf))
                        .unwrap_or(projected)
                } else {
                    self.store.ensure(package).map_err(|err| {
                        self.missing_package_with_extra(specifier, referrer, Some(err.to_string()))
                    })?
                }
            }
            meow_loader::ModuleLocator::LocalFile(ref path) => {
                path.parent().unwrap_or(path.as_path()).to_path_buf()
            }
            meow_loader::ModuleLocator::Native { .. } => {
                return Err(self.missing_package(specifier, referrer))
            }
        };
        Ok(package_root)
    }

    fn resolve_types_package_folder(
        &self,
        types_package_name: &str,
        _maybe_package_version: Option<&DenoVersion>,
        maybe_referrer: Option<&UrlOrPathRef>,
    ) -> Option<PathBuf> {
        let types_package_name = if types_package_name.starts_with("@types/") {
            types_package_name.to_owned()
        } else {
            format!("@types/{types_package_name}")
        };
        let referrer = maybe_referrer
            .and_then(|referrer| referrer.url().ok())
            .unwrap_or_else(|| self.resolver.project_root());
        let referrer = UrlOrPathRef::from_url(referrer);
        self.resolve_package_folder_from_package(&types_package_name, &referrer)
            .ok()
    }
}

impl InNpmPackageChecker for RuntimeNodeBridge {
    fn in_npm_package(&self, specifier: &Url) -> bool {
        let Ok(path) = specifier.to_file_path() else {
            return false;
        };
        path.starts_with(self.store.root())
            || self
                .resolver
                .project_root()
                .to_file_path()
                .ok()
                .is_some_and(|root| path.starts_with(root.join("node_modules")))
    }
}

impl NodeRequireLoader for RuntimeNodeBridge {
    fn ensure_read_permission<'a>(
        &self,
        _permissions: &mut PermissionsContainer,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, JsErrorBox> {
        Ok(path)
    }

    fn load_text_file_lossy(&self, path: &Path) -> Result<FastString, JsErrorBox> {
        let source = std::fs::read(path).map_err(|err| {
            JsErrorBox::generic(format!("failed reading {}: {err}", path.display()))
        })?;
        Ok(std::string::String::from_utf8_lossy(&source)
            .into_owned()
            .into())
    }

    fn is_maybe_cjs(&self, specifier: &Url) -> Result<bool, PackageJsonLoadError> {
        if let Ok(path) = specifier.to_file_path() {
            if path.starts_with(self.store.root()) {
                return Ok(matches!(
                    self.module_kind(specifier),
                    Some(meow_loader::ModuleKind::Cjs)
                ));
            }
            match path.extension().and_then(|ext| ext.to_str()) {
                None | Some("cjs") | Some("cts") => return Ok(true),
                Some("json") | Some("mjs") | Some("mts") => return Ok(false),
                _ => {}
            }
        }
        Ok(matches!(
            self.module_kind(specifier),
            Some(meow_loader::ModuleKind::Cjs)
        ))
    }

    fn is_maybe_cjs_from_require(&self, specifier: &Url) -> Result<bool, PackageJsonLoadError> {
        self.is_maybe_cjs(specifier)
    }

    fn resolve_require_node_module_paths(&self, from: &Path) -> Vec<String> {
        let mut paths = Vec::with_capacity(from.components().count() + 4);
        if let Some(projected_from) = self.projected_referrer_path(from) {
            push_node_module_paths(&mut paths, &projected_from);
        }
        push_node_module_paths(&mut paths, from);
        paths
    }

    fn resolve_package_folder_from_name(&self, package_name: &str) -> Option<PathBuf> {
        let referrer = UrlOrPathRef::from_url(self.resolver.project_root());
        self.resolve_package_folder_from_package(package_name, &referrer)
            .ok()
    }
}

pub(super) fn host_env_map(
    allow_env: &Option<String>,
    node_mode: meow_runtime::node::NodeMode,
) -> BTreeMap<String, String> {
    let all_vars: BTreeMap<String, String> = std::env::vars().collect();
    match allow_env {
        None if matches!(node_mode, meow_runtime::node::NodeMode::Enabled) => all_vars,
        None => {
            let mut env = BTreeMap::new();
            for key in &[
                "HOME",
                "PATH",
                "MEOW_NODE_PLATFORM",
                "MEOW_NODE_ARCH",
                "MEOW_EXEC_PATH",
                "USER",
                "LOGNAME",
                "SHELL",
                "TMPDIR",
                "TEMP",
                "TMP",
            ] {
                if let Some(val) = all_vars.get(*key) {
                    env.insert((*key).to_string(), val.clone());
                }
            }
            env
        }
        Some(names) if names.is_empty() => all_vars,
        Some(names) => {
            let mut env = BTreeMap::new();
            let allowed_keys: std::collections::HashSet<&str> = names
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            for key in allowed_keys {
                if let Some(val) = all_vars.get(key) {
                    env.insert(key.to_string(), val.clone());
                }
            }
            env
        }
    }
}

pub(super) async fn run_native_request(
    request: &NativeRunRequest,
    flags: RunFlagView<'_>,
    env: BTreeMap<String, String>,
) -> Result<ExitCode, RunCommandError> {
    let mut env = env;
    let host_home = host::host_home();
    env.entry("HOME".to_owned())
        .or_insert_with(|| host_home.to_string_lossy().into_owned());
    env.entry("MEOW_HOME".to_owned())
        .or_insert_with(|| host_home.to_string_lossy().into_owned());
    env.entry("MEOW_NODE_PLATFORM".to_owned())
        .or_insert_with(|| match std::env::consts::OS {
            "macos" => "darwin".to_owned(),
            "windows" => "win32".to_owned(),
            other => other.to_owned(),
        });
    env.entry("MEOW_NODE_ARCH".to_owned())
        .or_insert_with(|| match std::env::consts::ARCH {
            "aarch64" => "arm64".to_owned(),
            "x86_64" => "x64".to_owned(),
            other => other.to_owned(),
        });
    install_node_shim_env(&mut env)?;
    env.insert("MEOW_NODE_SHIM".to_owned(), "1".to_owned());
    if let Ok(exe) = std::env::current_exe() {
        env.insert(
            "MEOW_EXEC_PATH".to_owned(),
            exe.to_string_lossy().into_owned(),
        );
    }
    let ctx = build_runtime_context(&request.project_dir, true)?;
    let resolver = meow_loader::Resolver::from_resolution(
        &ctx.graph,
        ctx.cache.clone(),
        ctx.project_root.clone(),
        // === RT-005 ===
        meow_runtime::native::native_module_registry(),
        // === /RT-005 ===
    );
    let loader: std::rc::Rc<dyn meow_runtime::deno_core::ModuleLoader> =
        std::rc::Rc::new(meow_loader::MeowModuleLoader::new(
            resolver.clone(),
            std::rc::Rc::new(std::cell::RefCell::new(meow_graph::GraphDb::new())),
        ));
    let deno_node_bridge: std::rc::Rc<dyn meow_runtime::node::DenoNodeBridge> =
        std::rc::Rc::new(RuntimeNodeBridge::new(resolver.clone(), ctx.cache.clone()));
    let deno_node_services =
        meow_runtime::node::DenoNodeServicesBuilder::new(deno_node_bridge).build();

    // === RT-004 ===
    let caps: meow_runtime::web::NetCaps = std::sync::Arc::new(meow_runtime::AllowAll);
    let mut extensions = Vec::new();
    // === RT-005 ===
    extensions.push(meow_runtime::http_extension());
    // === UI-001 ===
    extensions.push(meow_runtime::ui_extension());
    extensions.push(meow_loader::cjs_resolve_extension(resolver.clone()));
    // === /UI-001 ===
    // === /RT-005 ===
    // === /RT-004 ===

    // === RT-006 ===
    let hermetic = run_hermetic_config(&flags, ctx.node_mode);
    meow_runtime::hermetic::pin_deterministic_intl(&hermetic);
    extensions.extend(meow_runtime::hermetic::extensions(hermetic));
    // === RT-007 ===
    let mut node_argv =
        Vec::with_capacity(request.argv.len() + 1 + usize::from(request.argv1.is_some()));
    node_argv.push("meow".to_owned());
    if let Some(argv1) = &request.argv1 {
        node_argv.push(argv1.clone());
    }
    node_argv.extend(request.argv.iter().cloned());
    extensions.extend(meow_runtime::node::extensions(
        meow_runtime::node::NodeOptions {
            mode: ctx.node_mode,
            argv: node_argv,
            main_module: request.main_module.clone(),
            cwd: request.process_cwd.clone(),
            env: env.clone(),
            deno_node_services: Some(deno_node_services),
            caps: Some(caps),
            user_agent: Some(format!("meow/{}", env!("CARGO_PKG_VERSION"))),
        },
    ));
    // === /RT-007 ===
    // === /RT-006 ===
    let max_heap_size = flags.max_old_space_size.map(|mib| mib * 1024 * 1024);
    let startup_snapshot = if flags.no_snapshot {
        None
    } else {
        Some(crate::SNAPSHOT_BLOB)
    };
    // Residual lazy sources re-feed deno_core the lazy_loaded_esm/js modules a
    // snapshot does not bake into the V8 heap. They apply ONLY when loading from
    // the snapshot; in eager mode the extensions register their own lazy sources,
    // so feeding residuals too would double-insert.
    let (residual_lazy_js, residual_lazy_esm): ResidualLazySources = if startup_snapshot.is_some() {
        (crate::RESIDUAL_LAZY_JS, crate::RESIDUAL_LAZY_ESM)
    } else {
        (&[], &[])
    };
    let mut runtime = meow_runtime::Runtime::new(meow_runtime::RuntimeOptions {
        module_loader: loader,
        extensions,
        max_heap_size,
        startup_snapshot,
        residual_lazy_js_sources: residual_lazy_js,
        residual_lazy_esm_sources: residual_lazy_esm,
        v8_flags: flags.v8_flags.map(|s| s.to_string()),
    })
    .map_err(|err| RunCommandError::Message(err.to_string()))?;
    // Apply hermetic shadows (Date / Math.random / performance / crypto) if the
    // active config requires them. Deferred to runtime init so snapshots keep
    // V8's native intrinsics; under --trust the conditionals short-circuit and
    // no FFI tax is paid in hot loops.
    runtime
        .apply_hermetic_shadows()
        .map_err(|err| RunCommandError::Message(err.to_string()))?;
    // Refresh Node bootstrap state if we loaded from a snapshot.
    // The snapshot bakes in placeholder argv/cwd/env from snapshot-creation time.
    if startup_snapshot.is_some() {
        let mut node_argv =
            Vec::with_capacity(request.argv.len() + 1 + usize::from(request.argv1.is_some()));
        node_argv.push("meow".to_string());
        if let Some(argv1) = &request.argv1 {
            node_argv.push(argv1.clone());
        }
        node_argv.extend(request.argv.iter().cloned());
        runtime
            .refresh_node_bootstrap(
                node_argv,
                request.main_module.clone(),
                request.process_cwd.clone(),
                env.clone(),
            )
            .map_err(|err| RunCommandError::Message(err.to_string()))?;
    }
    match runtime.run_main_module(&request.spec).await {
        Ok(()) => Ok(match runtime.take_process_exit_code() {
            Some(code) => ExitCode::from(code.rem_euclid(256) as u8),
            None => ExitCode::SUCCESS,
        }),
        Err(err) => match runtime.take_process_exit_code() {
            Some(code) => Ok(ExitCode::from(code.rem_euclid(256) as u8)),
            None => Err(RunCommandError::Message(err.to_string())),
        },
    }
}

fn prepare_direct_file_run(
    cwd: &Path,
    target: &str,
    argv: &[String],
) -> Result<NativeRunRequest, RunCommandError> {
    let abs = resolve_local_entry(cwd, target, true)?
        .ok_or_else(|| RunCommandError::Message(format!("cannot find {target}")))?;
    let entry_root = abs
        .parent()
        .map(find_project_root)
        .unwrap_or_else(|| find_project_root(cwd));
    native_file_request(entry_root, cwd.to_path_buf(), abs, argv)
}

fn native_file_request(
    project_dir: PathBuf,
    process_cwd: PathBuf,
    abs: PathBuf,
    argv: &[String],
) -> Result<NativeRunRequest, RunCommandError> {
    let spec = meow_runtime::ModuleSpecifier::from_file_path(&abs)
        .map_err(|()| RunCommandError::InvalidEntryPath(abs.display().to_string()))?;
    let main_module = Some(spec.to_string());
    Ok(NativeRunRequest {
        project_dir,
        process_cwd,
        spec,
        main_module,
        argv1: Some(abs.to_string_lossy().into_owned()),
        argv: argv.to_vec(),
    })
}

pub(super) fn build_runtime_context(
    project_dir: &Path,
    verify_cache: bool,
) -> Result<RuntimeContext, RunCommandError> {
    let project_root =
        meow_runtime::ModuleSpecifier::from_directory_path(project_dir).map_err(|()| {
            RunCommandError::InvalidProjectDirectory(project_dir.display().to_string())
        })?;
    let node_mode = runtime_mode(project_dir).map_err(RunCommandError::Message)?;
    let lockfile = load_lockfile(project_dir)?;
    let root_deps =
        load_declared_root_deps(project_dir, &lockfile).map_err(RunCommandError::Message)?;
    let cache = std::sync::Arc::new(meow_pkg::Cache::in_home(host::host_home()));
    let graph = meow_pkg::ResolutionGraph::assemble(std::sync::Arc::new(lockfile), root_deps)
        .map_err(|err| RunCommandError::Message(err.to_string()))?;
    let graph = std::sync::Arc::new(graph);
    if verify_cache {
        graph
            .verify_cached(&cache)
            .map_err(|err| RunCommandError::Message(err.to_string()))?;
    }
    Ok(RuntimeContext {
        project_dir: project_dir.to_path_buf(),
        project_root,
        node_mode,
        graph,
        cache,
    })
}

fn plan_script(
    ctx: &RuntimeContext,
    script: &str,
    cli_argv: &[String],
) -> Result<PlannedScript, RunCommandError> {
    let Some(tokens) = tokenize_simple_script(script) else {
        return Ok(PlannedScript::Shell {
            script: script.to_owned(),
            argv: cli_argv.to_vec(),
        });
    };
    if tokens.is_empty() {
        return Ok(PlannedScript::Shell {
            script: script.to_owned(),
            argv: cli_argv.to_vec(),
        });
    }

    let first = &tokens[0];
    if matches!(first.as_str(), "node" | "meow") {
        if tokens.len() < 2 || tokens[1].starts_with('-') {
            return Ok(PlannedScript::Shell {
                script: script.to_owned(),
                argv: cli_argv.to_vec(),
            });
        }
        if let Some(abs) =
            resolve_local_entry(&ctx.project_dir, &tokens[1], looks_like_path(&tokens[1]))?
        {
            let mut argv = tokens[2..].to_vec();
            argv.extend(cli_argv.iter().cloned());
            return Ok(PlannedScript::Native(native_file_request(
                ctx.project_dir.clone(),
                ctx.project_dir.clone(),
                abs,
                &argv,
            )?));
        }
        return Ok(PlannedScript::Shell {
            script: script.to_owned(),
            argv: cli_argv.to_vec(),
        });
    }

    if let Some(abs) = resolve_local_entry(&ctx.project_dir, first, looks_like_path(first))? {
        let mut argv = tokens[1..].to_vec();
        argv.extend(cli_argv.iter().cloned());
        return Ok(PlannedScript::Native(native_file_request(
            ctx.project_dir.clone(),
            ctx.project_dir.clone(),
            abs,
            &argv,
        )?));
    }

    let mut argv = tokens[1..].to_vec();
    argv.extend(cli_argv.iter().cloned());
    if let Some(bin) = resolve_package_bin(ctx, first, &argv)? {
        return Ok(PlannedScript::Native(bin));
    }

    Ok(PlannedScript::Shell {
        script: script.to_owned(),
        argv: cli_argv.to_vec(),
    })
}

fn resolve_package_bin(
    ctx: &RuntimeContext,
    command: &str,
    argv: &[String],
) -> Result<Option<NativeRunRequest>, RunCommandError> {
    let store = meow_pkg::UnpackedStore::new(ctx.cache.root().join("unpacked"), ctx.cache.clone());
    let mut matches = Vec::new();
    for (name, version) in ctx.graph.root_deps() {
        let Some(entry) = ctx.graph.lockfile().get(name, version) else {
            continue;
        };
        let root = store
            .ensure(&entry.integrity)
            .map_err(|err| RunCommandError::UnpackBin {
                package: name.to_string(),
                reason: err.to_string(),
            })?;
        let manifest_path = root.join("package.json");
        let bytes = std::fs::read(&manifest_path).map_err(|source| {
            RunCommandError::CachedManifestRead {
                path: manifest_path.clone(),
                source,
            }
        })?;
        let manifest: meow_loader::package::PackageJson =
            serde_json::from_slice(&bytes).map_err(|source| {
                RunCommandError::CachedManifestParse {
                    path: manifest_path,
                    source,
                }
            })?;
        let Some(raw_member) = manifest.bin_entry(command) else {
            continue;
        };
        let member = normalize_cached_member(name.as_str(), raw_member)?;
        let runtime_bin_path = root.join(&member);
        if !runtime_bin_path.is_file() {
            return Err(RunCommandError::MissingBinFile {
                package: name.to_string(),
                command: command.to_owned(),
                target: member,
            });
        }
        let key = format!("{}@{}", name.as_str().replace('/', "+"), version);
        let projected_bin_path = ctx
            .project_dir
            .join("node_modules")
            .join(".meow")
            .join(key)
            .join("node_modules")
            .join(name.as_str())
            .join(&member);
        let bin_path = if projected_bin_path.is_file() {
            projected_bin_path
        } else {
            runtime_bin_path
        };
        let spec = Url::from_file_path(&bin_path)
            .map_err(|()| RunCommandError::InvalidEntryPath(bin_path.display().to_string()))?;
        matches.push((name.to_string(), spec, bin_path));
    }

    match matches.len() {
        0 => Ok(None),
        1 => {
            let (_package, spec, bin_path) = matches.pop().expect("one match");
            Ok(Some(NativeRunRequest {
                project_dir: ctx.project_dir.clone(),
                process_cwd: ctx.project_dir.clone(),
                main_module: Some(spec.to_string()),
                spec,
                argv1: Some(bin_path.to_string_lossy().into_owned()),
                argv: argv.to_vec(),
            }))
        }
        _ => Err(RunCommandError::AmbiguousBin {
            command: command.to_owned(),
            packages: matches
                .iter()
                .map(|(package, _, _)| package.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

fn normalize_cached_member(package: &str, raw: &str) -> Result<String, RunCommandError> {
    let path = Path::new(raw);
    let mut member = String::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(segment) => {
                let segment =
                    segment
                        .to_str()
                        .ok_or_else(|| RunCommandError::InvalidBinTarget {
                            package: package.to_owned(),
                            target: raw.to_owned(),
                        })?;
                if !member.is_empty() {
                    member.push('/');
                }
                member.push_str(segment);
            }
            _ => {
                return Err(RunCommandError::InvalidBinTarget {
                    package: package.to_owned(),
                    target: raw.to_owned(),
                });
            }
        }
    }
    if member.is_empty() {
        return Err(RunCommandError::InvalidBinTarget {
            package: package.to_owned(),
            target: raw.to_owned(),
        });
    }
    Ok(member)
}

fn resolve_local_entry(
    base: &Path,
    raw: &str,
    required: bool,
) -> Result<Option<PathBuf>, RunCommandError> {
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        base.join(raw)
    };
    match std::fs::canonicalize(&candidate) {
        Ok(path) => {
            if path.is_file() {
                Ok(Some(path))
            } else {
                Err(RunCommandError::EntryNotFile(path.display().to_string()))
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && !required => Ok(None),
        Err(source) => Err(RunCommandError::MissingEntry {
            target: raw.to_owned(),
            source,
        }),
    }
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with('.')
        || token.starts_with('/')
        || token.contains('/')
        || token.contains('\\')
        || matches!(
            Path::new(token).extension().and_then(|ext| ext.to_str()),
            Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "jsx" | "tsx")
        )
}

fn tokenize_simple_script(script: &str) -> Option<Vec<String>> {
    #[derive(Clone, Copy)]
    enum Quote {
        Single,
        Double,
    }

    let mut quote = None;
    let mut chars = script.chars();
    let mut current = String::new();
    let mut tokens = Vec::new();
    while let Some(ch) = chars.next() {
        match quote {
            None => match ch {
                ' ' | '\t' | '\r' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                '\'' => quote = Some(Quote::Single),
                '"' => quote = Some(Quote::Double),
                '\\' => current.push(chars.next()?),
                '&' | '|' | '>' | '<' | ';' | '`' | '(' | ')' | '\n' | '*' | '?' | '[' | ']'
                | '{' | '}' | '$' => return None,
                _ => current.push(ch),
            },
            Some(Quote::Single) => match ch {
                '\'' => quote = None,
                _ => current.push(ch),
            },
            Some(Quote::Double) => match ch {
                '"' => quote = None,
                '\\' => current.push(chars.next()?),
                '$' | '`' => return None,
                _ => current.push(ch),
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    (!tokens.is_empty()).then_some(tokens)
}

fn lifecycle_env(
    event: &str,
    script: &str,
    project_dir: &Path,
    init_cwd: &Path,
    allow_env: &Option<String>,
) -> Result<BTreeMap<String, String>, RunCommandError> {
    let mut env = host_env_map(allow_env, meow_runtime::node::NodeMode::Enabled);
    env.insert(
        "INIT_CWD".to_owned(),
        init_cwd.to_string_lossy().into_owned(),
    );
    env.insert("npm_lifecycle_event".to_owned(), event.to_owned());
    env.insert("npm_lifecycle_script".to_owned(), script.to_owned());
    env.insert(
        "npm_package_json".to_owned(),
        project_dir
            .join("package.json")
            .to_string_lossy()
            .into_owned(),
    );
    install_node_shim_env(&mut env)?;
    Ok(env)
}

fn install_node_shim_env(env: &mut BTreeMap<String, String>) -> Result<(), RunCommandError> {
    let exe = std::env::current_exe().map_err(|source| {
        RunCommandError::Message(format!("cannot resolve current executable: {source}"))
    })?;
    let home = host::host_home();
    let shim_dir = home.join(".meow").join("bin");
    std::fs::create_dir_all(&shim_dir).map_err(|source| {
        RunCommandError::Message(format!(
            "cannot create node shim directory {}: {source}",
            shim_dir.display()
        ))
    })?;
    let shim_path = shim_dir.join(if cfg!(windows) { "node.exe" } else { "node" });
    install_node_shim(&exe, &shim_path)?;

    env.insert(
        "npm_node_execpath".to_owned(),
        shim_path.to_string_lossy().into_owned(),
    );
    env.insert("NODE".to_owned(), shim_path.to_string_lossy().into_owned());

    let current_path = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let mut paths = std::env::split_paths(&current_path).collect::<Vec<_>>();
    if paths.first() != Some(&shim_dir) {
        paths.retain(|path| path != &shim_dir);
        paths.insert(0, shim_dir);
    }
    let joined = std::env::join_paths(paths).map_err(|source| {
        RunCommandError::Message(format!("cannot construct PATH for node shim: {source}"))
    })?;
    env.insert("PATH".to_owned(), joined.to_string_lossy().into_owned());
    Ok(())
}

#[cfg(unix)]
fn install_node_shim(exe: &Path, shim_path: &Path) -> Result<(), RunCommandError> {
    use std::os::unix::fs::PermissionsExt;

    let quoted_exe = exe.to_string_lossy().replace('\'', "'\\''");
    let script = format!("#!/bin/sh\nMEOW_NODE_SHIM=1 exec '{quoted_exe}' \"$@\"\n");
    if std::fs::read_to_string(shim_path).is_ok_and(|existing| existing == script) {
        return Ok(());
    }
    match std::fs::remove_file(shim_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(RunCommandError::Message(format!(
                "cannot replace node shim {}: {source}",
                shim_path.display()
            )));
        }
    }
    std::fs::write(shim_path, script).map_err(|source| {
        RunCommandError::Message(format!(
            "cannot write node shim {}: {source}",
            shim_path.display()
        ))
    })?;
    let mut perms = std::fs::metadata(shim_path)
        .map_err(|source| {
            RunCommandError::Message(format!(
                "cannot stat node shim {}: {source}",
                shim_path.display()
            ))
        })?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(shim_path, perms).map_err(|source| {
        RunCommandError::Message(format!(
            "cannot mark node shim executable {}: {source}",
            shim_path.display()
        ))
    })
}

#[cfg(windows)]
fn install_node_shim(exe: &Path, shim_path: &Path) -> Result<(), RunCommandError> {
    // On Windows, copying the running executable fails with "file in use"
    // (os error 32) when meow is the active process. Use a hard link first
    // (no file lock, instant), falling back to a batch shim if linking fails
    // (e.g. cross-volume). The batch shim delegates to the real exe at runtime.
    let quoted_exe = exe.to_string_lossy().replace('\'', "'\\''");
    let script = format!("@echo off\r\nset MEOW_NODE_SHIM=1\r\n\"{quoted_exe}\" %*\r\n");

    // Check if the shim is already correct (idempotent — avoid touching a
    // locked file when no change is needed).
    if std::fs::read_to_string(shim_path).is_ok_and(|existing| existing == script) {
        return Ok(());
    }

    // Remove any existing shim first.
    match std::fs::remove_file(shim_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(RunCommandError::Message(format!(
                "cannot replace node shim {}: {source}",
                shim_path.display()
            )));
        }
    }

    // Write the batch shim. This avoids the file-lock problem of copying the
    // running executable, and works across volumes.
    std::fs::write(shim_path, script).map_err(|source| {
        RunCommandError::Message(format!(
            "cannot write node shim {}: {source}",
            shim_path.display()
        ))
    })
}

fn run_shell_command(
    project_dir: &Path,
    script: &str,
    argv: &[String],
    env: BTreeMap<String, String>,
) -> Result<ExitCode, RunCommandError> {
    let mut command_text = script.to_owned();
    if !argv.is_empty() {
        command_text.push(' ');
        command_text.push_str(
            &argv
                .iter()
                .map(|arg| quote_shell_arg(arg))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    let mut command = shell_command(&command_text);
    command
        .current_dir(project_dir)
        .envs(env)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let status = command
        .status()
        .map_err(|source| RunCommandError::ShellSpawn {
            shell: shell_name(),
            source,
        })?;
    match status.code() {
        Some(code) => Ok(ExitCode::from(code.rem_euclid(256) as u8)),
        None => Err(RunCommandError::ShellTerminated),
    }
}

#[cfg(unix)]
fn shell_name() -> &'static str {
    "sh"
}

#[cfg(windows)]
fn shell_name() -> &'static str {
    "cmd"
}

#[cfg(unix)]
fn shell_command(script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(script);
    command
}

#[cfg(windows)]
fn shell_command(script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command.arg("/C").arg(script);
    command
}

#[cfg(unix)]
fn quote_shell_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_owned();
    }
    if arg
        .bytes()
        .all(|byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'-' | b':' | b'@' | b'=' | b'+'))
    {
        return arg.to_owned();
    }
    format!("'{}'", arg.replace('\'', "'\"'\"'"))
}

#[cfg(windows)]
fn quote_shell_arg(arg: &str) -> String {
    let escaped = arg.replace('^', "^^").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn find_package_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        if current.join("package.json").is_file() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}
// === /RUN-001 ===
// === /RT-001 ===

// === CFG-003 ===
/// Derive runtime roots from package.json when present. A missing package.json
/// keeps the lockfile-only fallback so local-only runs still work.
fn load_declared_root_deps(
    project_root: &Path,
    lockfile: &meow_pkg::Lockfile,
) -> Result<BTreeMap<meow_pkg::PackageName, meow_pkg::Version>, String> {
    match meow_config::PackageJson::read(project_root) {
        Ok(package_json) => {
            let direct = package_json
                .direct_dependencies()
                .map_err(|err| err.to_string())?;
            meow_pkg::resolve_roots(&direct, lockfile).map_err(|err| err.to_string())
        }
        Err(meow_config::ConfigError::PackageJsonNotFound(_)) => {
            fallback_root_deps_from_lockfile(lockfile)
        }
        Err(err) => Err(err.to_string()),
    }
}

fn fallback_root_deps_from_lockfile(
    lockfile: &meow_pkg::Lockfile,
) -> Result<BTreeMap<meow_pkg::PackageName, meow_pkg::Version>, String> {
    let mut deps = BTreeMap::new();
    for entry in lockfile.iter() {
        if let Some(prev) = deps.insert(entry.name.clone(), entry.version.clone()) {
            return Err(format!(
                "lockfile pins multiple versions of `{}` ({prev}, {}) — root resolution is ambiguous without package.json; run `meow install` or add package.json",
                entry.name, entry.version
            ));
        }
    }
    Ok(deps)
}
// === /CFG-003 ===

/// Parse `--max-old-space-size=<N>` from `NODE_OPTIONS` (value in MiB).
/// Returns `None` if the env var is unset or the flag is absent/malformed.
fn env_max_old_space_size() -> Option<usize> {
    let node_options = std::env::var("NODE_OPTIONS").ok()?;
    for token in node_options.split_whitespace() {
        if let Some(rest) = token.strip_prefix("--max-old-space-size=") {
            if let Ok(mib) = rest.parse::<usize>() {
                return Some(mib);
            }
        }
    }
    None
}

// === TEST-001 ===
// === TASK-001 ===
/// `meow task <name>` — run a package.json script by name. Delegates to the same
/// script resolution as `meow run`.
pub fn cmd_task(args: &TaskArgs) -> ExitCode {
    let flags = RunFlagView {
        argv: &args.argv,
        allow_clock: false,
        allow_random: false,
        allow_env: &None,
        trust: false,
        max_old_space_size: None,
        no_snapshot: false,
        v8_flags: None,
    };
    cmd_run_inner("task", &args.name, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_script_rejects_shell_operators() {
        assert_eq!(
            tokenize_simple_script("node ./dev.cjs --watch").expect("simple script"),
            vec!["node", "./dev.cjs", "--watch"]
        );
        assert!(tokenize_simple_script("echo hi && echo ok").is_none());
    }
}
