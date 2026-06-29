//! The `meow` command tree (DIST-001). Every parsed verb dispatches to a concrete
//! implementation — no stubs remain.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::OnceLock;

use clap::{Args, Parser, Subcommand, ValueEnum};
use meow_ui::Ui;

use meow_runtime::node::{
    FastString, InNpmPackageChecker, JsErrorBox, NodeRequireLoader, NpmPackageFolderResolver,
    PackageFolderResolveError, PackageJsonLoadError, PermissionsContainer, Url, UrlOrPathRef,
    Version as DenoVersion,
};
use node_resolver::errors::{PackageFolderResolveErrorKind, PackageNotFoundError};

/// (lazy JS, lazy ESM) residual sources re-fed to deno_core under a snapshot.
type ResidualLazySources = (
    &'static [(&'static str, &'static str)],
    &'static [(&'static str, &'static str)],
);

fn ui() -> Ui {
    Ui::from_env(&crate::host::term_env())
}

fn purr(body: &str) {
    ui().success(body);
}

fn hiss(body: &str) {
    ui().error(body);
}

/// meow — a standards-first JavaScript/TypeScript runtime + unified toolchain.
#[derive(Debug, Parser)]
#[command(
    name = "meow",
    version,
    about = meow_ui::banner::TAGLINE,
    long_about = None,
    propagate_version = true,
    styles = meow_help_styles()
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

pub fn normalize_argv(mut argv: Vec<OsString>) -> Vec<OsString> {
    if invoked_as_node(argv.first()) {
        return normalize_node_argv(argv);
    }
    // If called as `meowx` or `mwx`, auto-inject the `x` subcommand.
    if invoked_as_meowx(argv.first()) {
        let mut out = Vec::with_capacity(argv.len() + 1);
        out.push(argv[0].clone());
        out.push(OsString::from("x"));
        out.extend(argv.into_iter().skip(1));
        return out;
    }
    if argv.len() <= 1 {
        return argv;
    }
    let first = argv[1].to_string_lossy();
    if first == "-e" || first == "--eval" || first == "-p" || first == "--print" {
        let mut out = Vec::with_capacity(argv.len() + 1);
        out.push(argv[0].clone());
        out.push(OsString::from("node-eval"));
        out.extend(argv.into_iter().skip(1));
        return out;
    }
    if first == "run" && argv.len() > 2 {
        let second = argv[2].to_string_lossy();
        if second == "-e" || second == "--eval" || second == "-p" || second == "--print" {
            let mut out = Vec::with_capacity(argv.len());
            out.push(argv[0].clone());
            out.push(OsString::from("node-eval"));
            out.extend(argv.into_iter().skip(2));
            return out;
        }
    }
    // Omni-router: if argv[1] is not a known command, figure out intent.
    if !is_known_command(&first) && !first.starts_with('-') {
        if should_inject_run(&first) {
            // File path or standard script → `meow run <target>`
            argv.insert(1, OsString::from("run"));
            return argv;
        }
        // Otherwise assume ephemeral package → `meow x <pkg>`
        let mut out = Vec::with_capacity(argv.len() + 1);
        out.push(argv[0].clone());
        out.push(OsString::from("x"));
        out.extend(argv.into_iter().skip(1));
        return out;
    }
    if first != "run" {
        return argv;
    }
    let mut out = Vec::with_capacity(argv.len());
    out.push(argv[0].clone());
    out.push(argv[1].clone());
    let mut index = 2;
    while index < argv.len() {
        let arg = argv[index].to_string_lossy();
        if !is_deno_run_compat_flag(&arg) {
            break;
        }
        index += 1;
    }
    out.extend(argv.into_iter().skip(index));
    out
}

fn invoked_as_node(argv0: Option<&OsString>) -> bool {
    let Some(argv0) = argv0 else {
        return false;
    };
    if Path::new(argv0)
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name == "node" || name == "node.exe")
    {
        return true;
    }
    std::env::var_os("MEOW_NODE_SHIM").is_some_and(|value| value == "1")
}

fn normalize_node_argv(argv: Vec<OsString>) -> Vec<OsString> {
    let mut out = Vec::with_capacity(argv.len() + 2);
    let mut iter = argv.into_iter();
    out.push(iter.next().unwrap_or_else(|| OsString::from("node")));
    let mut args: Vec<OsString> = iter.collect();
    if args.first().and_then(|arg| arg.to_str()) == Some("run") {
        args.remove(0);
    }

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].clone();
        let raw = arg.to_string_lossy();
        if raw == "run" && index + 1 < args.len() {
            let next = args[index + 1].to_string_lossy();
            if next == "-e" || next == "--eval" || next == "-p" || next == "--print" {
                out.push(OsString::from("node-eval"));
                out.extend(args[index + 1..].iter().cloned());
                return out;
            }
        }
        if raw == "-e" || raw == "--eval" || raw == "-p" || raw == "--print" {
            out.push(OsString::from("node-eval"));
            out.extend(args[index..].iter().cloned());
            return out;
        }
        if raw == "-r" || raw == "--require" || raw == "--import" || raw == "--loader" {
            index += 2;
            continue;
        }
        if raw.starts_with("--require=")
            || raw.starts_with("--import=")
            || raw.starts_with("--loader=")
        {
            index += 1;
            continue;
        }
        if raw.starts_with('-') {
            index += 1;
            continue;
        }
        out.push(OsString::from("run"));
        out.push(arg);
        out.push(OsString::from("--"));
        out.extend(args[index + 1..].iter().cloned());
        return out;
    }

    out.push(OsString::from("node-eval"));
    out.push(OsString::from("--interactive"));
    out
}

/// Standard npm/package.json script names — the omni-router maps bare script
/// requests to `meow run <script>` without requiring explicit `run`.
const STANDARD_SCRIPTS: &[&str] = &[
    "build",
    "start",
    "dev",
    "lint",
    "fmt",
    "test",
    "preview",
    "serve",
    "deploy",
    "release",
    "clean",
    "compile",
    "watch",
    "storybook",
];

fn is_standard_script(arg: &str) -> bool {
    STANDARD_SCRIPTS.contains(&arg)
}

/// Detect if the binary was invoked via a symlink/alias like `meowx` or `mwx`.
/// When true, the argv normaliser automatically injects `x` as the subcommand,
/// so `meowx create-vite my-app` works like `meow x create-vite my-app`.
fn invoked_as_meowx(argv0: Option<&OsString>) -> bool {
    let Some(argv0) = argv0 else {
        return false;
    };
    Path::new(argv0)
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name == "meowx" || name == "meowx.exe" || name == "mwx" || name == "mwx.exe"
        })
}

fn should_inject_run(arg: &str) -> bool {
    if arg.is_empty() || arg.starts_with('-') || is_known_command(arg) {
        return false;
    }
    // File paths (local or absolute)
    arg.starts_with("file://")
        || arg.starts_with("./")
        || arg.starts_with("../")
        || arg.starts_with('/')
        || arg.contains('/')
        || arg.contains('\\')
        // Known file extensions
        || Path::new(arg)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext, "js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "json"))
        // Standard package.json scripts
        || is_standard_script(arg)
}

fn is_known_command(arg: &str) -> bool {
    matches!(
        arg,
        "run"
            | "x"
            | "execute"
            | "dev"
            | "install"
            | "i"
            | "add"
            | "remove"
            | "rm"
            | "del"
            | "delete"
            | "uninstall"
            | "task"
            | "test"
            | "check"
            | "lint"
            | "fmt"
            | "bundle"
            | "ls"
            | "why-slow"
            | "why-large"
            | "why-dep"
            | "doctor"
            | "sync"
            | "types"
    )
}

fn is_deno_run_compat_flag(arg: &str) -> bool {
    matches!(arg, "-A" | "--allow-all" | "--unstable") || arg.starts_with("--unstable-")
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute a file or a package.json script (default mode = node-compat; `strict-web` is opt-in).
    Run(RunArgs),
    /// Ephemeral package execution (npx/bunx equivalent): installs the named package
    /// into a transient workspace, runs its binary, and discards the workspace.
    #[command(alias = "execute")]
    X(XArgs),
    /// Internal node-shim eval/print mode.
    #[command(name = "node-eval", hide = true)]
    NodeEval(NodeEvalArgs),
    /// Shorthand for `meow run dev`.
    Dev(RunScriptArgs),
    /// Install dependencies from the lockfile.
    #[command(alias = "i")]
    Install(InstallArgs),
    /// Add a dependency + update the lockfile.
    Add(PkgArgs),
    /// Remove a dependency + update the lockfile.
    #[command(alias = "rm", alias = "del", alias = "delete", alias = "uninstall")]
    Remove(PkgArgs),
    /// Run a typed task from meow.tasks.ts.
    Task(TaskArgs),
    /// Isolate-backed test runner.
    Test(TestArgs),
    /// Typecheck via TypeScript.
    Check(PathArgs),
    /// Check for lint errors.
    Lint(PathArgs),
    /// Format source files.
    Fmt(FmtArgs),
    /// Bundle via Rolldown over the module graph.
    Bundle(BundleArgs),
    /// Observability: slowest imports / init / cold-start (the Module Load timeline).
    #[command(name = "why-slow")]
    WhySlow(PathArgs),
    /// Observability: largest modules / duplicate packages.
    #[command(name = "why-large")]
    WhyLarge(PathArgs),
    /// Dependency provenance / paths.
    #[command(name = "why-dep")]
    WhyDep(WhyDepArgs),
    /// Environment / config / lockfile health.
    Doctor,
    /// Regenerate TypeScript configuration and type declarations.
    Sync,
    /// Regenerate bundled type declarations.
    Types(TypesArgs),
    /// List active dev servers and processes.
    Ls,
}

#[derive(Debug, Args)]
pub struct NodeEvalArgs {
    /// Node eval flag used by the shim.
    #[arg(allow_hyphen_values = true, value_parser = ["-e", "--eval", "-p", "--print", "--interactive"])]
    pub mode: String,
    /// Source code to evaluate.
    pub code: Option<String>,
    /// Arguments after eval source.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Script name or entry module to execute.
    pub target: String,
    /// Arguments after `--`, to forward to the program.
    #[arg(last = true)]
    pub argv: Vec<String>,
    // === RT-006 ===
    /// Expose the real system clock + monotonic time (the run is no longer reproducible).
    #[arg(long)]
    pub allow_clock: bool,
    /// Use OS entropy for `Math.random` + `crypto.getRandomValues` (the run is no longer reproducible).
    #[arg(long)]
    pub allow_random: bool,
    /// Expose host env vars: bare `--allow-env` grants ALL (widest), `--allow-env=HOME,PATH` scopes
    /// to the named vars; ungranted vars stay invisible. Absent = no host env (deterministic).
    #[arg(long, value_name = "NAMES", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    pub allow_env: Option<String>,
    // === /RT-006 ===
    /// Explicitly grant full host access (clock, entropy, environment).
    #[arg(long)]
    pub trust: bool,
    /// Set the V8 heap limit in MiB (overrides the adaptive default).
    /// Equivalent to Node's `--max-old-space-size`.
    #[arg(long, value_name = "MiB")]
    pub max_old_space_size: Option<usize>,
    /// Disable the V8 startup snapshot (slower init, useful for debugging).
    #[arg(long, hide = true)]
    pub no_snapshot: bool,
    /// Pass arbitrary flags directly to the V8 engine
    /// (e.g. `--v8-flags=--allow-natives-syntax,--trace-opt`).
    #[arg(long, value_name = "FLAGS")]
    pub v8_flags: Option<String>,
}

// === RUN-001 ===
#[derive(Debug, Args)]
pub struct RunScriptArgs {
    /// Arguments after `--`, to forward to the script.
    #[arg(last = true)]
    pub argv: Vec<String>,
    // === RT-006 ===
    #[arg(long)]
    pub allow_clock: bool,
    #[arg(long)]
    pub allow_random: bool,
    #[arg(long, value_name = "NAMES", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    pub allow_env: Option<String>,
    // === /RT-006 ===
    /// Explicitly grant full host access (clock, entropy, environment).
    #[arg(long)]
    pub trust: bool,
    /// Set the V8 heap limit in MiB (overrides the adaptive default).
    /// Equivalent to Node's `--max-old-space-size`.
    #[arg(long, value_name = "MiB")]
    pub max_old_space_size: Option<usize>,
    /// Disable the V8 startup snapshot (slower init, useful for debugging).
    #[arg(long, hide = true)]
    pub no_snapshot: bool,
    /// Pass arbitrary flags directly to the V8 engine
    /// (e.g. `--v8-flags=--allow-natives-syntax,--trace-opt`).
    #[arg(long, value_name = "FLAGS")]
    pub v8_flags: Option<String>,
}
// === /RUN-001 ===

// === LS-001 ===
/// `meow ls` — list active dev servers and processes discovered via `lsof`.
/// Renders a clean table with PID, COMMAND, and PORT columns.
fn cmd_ls() -> ExitCode {
    let u = ui();
    let output = match std::process::Command::new("lsof")
        .args(["-i", "-P", "-n"])
        .output()
    {
        Ok(out) if out.status.success() => out.stdout,
        Ok(_) => {
            u.note("meow ls: lsof returned no data (no active servers?)");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            hiss(&format!("meow ls: cannot run lsof: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let text = String::from_utf8_lossy(&output);
    let mut rows: Vec<(u32, String, u16)> = Vec::new();

    // PORT → known-dev-server label map
    let known_ports: std::collections::HashMap<u16, &str> = {
        let mut m = std::collections::HashMap::new();
        m.insert(3000, "Next.js / React");
        m.insert(4321, "Astro");
        m.insert(5173, "Vite");
        m.insert(4173, "Vite Preview");
        m.insert(8000, "Python / Caddy");
        m.insert(8080, "HTTP alt");
        m.insert(1420, "Tauri");
        m.insert(8787, "Wrangler");
        m
    };

    for line in text.lines().skip(1) {
        // lsof -i -P -n output: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let command = parts[0];
        let pid: u32 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let name = parts[8];

        // Extract port from "[::1]:5173" or "127.0.0.1:5173" or "*:3000" etc
        if let Some(colon) = name.rfind(':') {
            let port_str = &name[colon + 1..];
            if let Ok(port) = port_str.parse::<u16>() {
                if known_ports.contains_key(&port) {
                    let label = known_ports.get(&port).copied().unwrap_or(command);
                    rows.push((pid, label.to_string(), port));
                }
            }
        }
    }

    if rows.is_empty() {
        u.note("meow ls: no known dev servers detected.");
        return ExitCode::SUCCESS;
    }

    // Deduplicate by (pid, port)
    rows.sort();
    rows.dedup();

    let headers = &["PID", "SERVICE", "PORT"];
    let data: Vec<Vec<String>> = rows
        .iter()
        .map(|(pid, cmd, port)| vec![pid.to_string(), cmd.clone(), port.to_string()])
        .collect();
    let aligns = &[
        meow_ui::table::Align::Right,
        meow_ui::table::Align::Left,
        meow_ui::table::Align::Right,
    ];
    u.table(headers, &data, aligns);
    ExitCode::SUCCESS
}

// === EPHEMERAL-X ===
/// Arguments for `meow x <package> [-- <args>]`.
/// Arguments for `meow x [flags] <package> [args...]`.
/// Flags come BEFORE the package name; everything after the package is treated
/// as trailing arguments (no `--` separator required).
#[derive(Debug, Args)]
pub struct XArgs {
    // === RT-006 ===
    /// Expose the real system clock + monotonic time.
    #[arg(long)]
    pub allow_clock: bool,
    /// Use OS entropy for `Math.random` + `crypto.getRandomValues`.
    #[arg(long)]
    pub allow_random: bool,
    /// Expose host env vars.
    #[arg(long, value_name = "NAMES", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    pub allow_env: Option<String>,
    // === /RT-006 ===
    /// Explicitly grant full host access (clock, entropy, environment).
    #[arg(long)]
    pub trust: bool,
    /// Set the V8 heap limit in MiB.
    #[arg(long, value_name = "MiB")]
    pub max_old_space_size: Option<usize>,
    /// Disable the V8 startup snapshot.
    #[arg(long, hide = true)]
    pub no_snapshot: bool,
    /// Pass arbitrary flags directly to the V8 engine
    /// (e.g. `--v8-flags=--allow-natives-syntax,--trace-opt`).
    #[arg(long, value_name = "FLAGS")]
    pub v8_flags: Option<String>,
    /// Package to download + execute ephemerally (e.g. `create-vite@latest`).
    pub package: String,
    /// Arguments forwarded to the package's binary (everything after the package name).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}
// === /EPHEMERAL-X ===

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// How to lay out installed packages on disk.
    #[arg(long, value_enum, default_value_t = InstallMode::Materialize)]
    pub mode: InstallMode,
    // === PKG-004 ===
    /// Install into node_modules/ (default).
    #[arg(long, conflicts_with_all = ["mode", "vendor"])]
    pub materialize: bool,
    /// Copy packages into a vendor/ directory instead of node_modules/.
    #[arg(long, conflicts_with_all = ["mode", "materialize"])]
    pub vendor: bool,
    /// Target directory for --vendor (default: vendor).
    #[arg(long, default_value = "vendor")]
    pub vendor_dir: PathBuf,
    /// Remove existing node_modules/ or vendor/ before writing.
    #[arg(long)]
    pub clean: bool,
    // === /PKG-004 ===
    // === PKG-002 ===
    /// Optional package specifier(s) to add before installing, e.g. `lodash` or `p-limit@^5`.
    #[arg(value_name = "PKG")]
    pub packages: Vec<String>,
    // === /PKG-002 ===
}

#[derive(Debug, Args)]
pub struct TypesArgs {
    /// Regenerate bundled type declarations.
    #[arg(long, conflicts_with = "check")]
    pub emit: bool,
    /// Verify the committed declarations are fresh (default).
    #[arg(long, conflicts_with = "emit")]
    pub check: bool,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum InstallMode {
    /// Install into node_modules/ with hardlinks (default).
    Materialize,
    /// Copy all packages into a vendor/ directory.
    Vendor,
}

#[derive(Debug, Args)]
pub struct PkgArgs {
    /// Package specifier(s), e.g. `lodash@^4`.
    #[arg(required = true)]
    pub packages: Vec<String>,
    /// Install or remove globally (writes/removes shim in ~/.meow/bin).
    #[arg(short = 'g', long)]
    pub global: bool,
}

#[derive(Debug, Args)]
pub struct TaskArgs {
    /// Task name from meow.tasks.ts.
    pub name: String,
    /// Arguments forwarded to the task.
    #[arg(last = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct TestArgs {
    /// Optional test name/path filter.
    pub filter: Option<String>,
}

#[derive(Debug, Args)]
pub struct PathArgs {
    /// Target paths (default: workspace root).
    pub paths: Vec<PathBuf>,
}
#[derive(Debug, Args)]
pub struct FmtArgs {
    /// Target paths (default: workspace root).
    pub paths: Vec<PathBuf>,
    /// Report changed files only and fail in check mode.
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Args)]
pub struct BundleArgs {
    /// Entry module(s) to bundle.
    #[arg(required = true)]
    pub entries: Vec<PathBuf>,
    /// Output directory.
    #[arg(long, short)]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct WhyDepArgs {
    /// Package to explain.
    pub pkg: String,
    // === OBS-001 ===
    /// Show one shortest chain per version instead of every chain.
    #[arg(long)]
    pub shortest: bool,
    /// Emit machine-readable JSON (the serialized report) instead of prose.
    #[arg(long)]
    pub json: bool,
    /// Max chains to enumerate per version before reporting truncation.
    #[arg(long, default_value_t = meow_obs::DEFAULT_PATH_LIMIT)]
    pub limit: usize,
    // === /OBS-001 ===
}

impl Cli {
    pub fn run(self) -> ExitCode {
        let Some(command) = self.command else {
            return cmd_landing();
        };
        match command {
            // === CFG-001 ===
            Command::Sync => cmd_sync(),
            // === /CFG-001 ===
            // === PKG-002 ===
            Command::Install(args) => cmd_install(&args),
            Command::Add(args) => cmd_add(&args),
            Command::Remove(args) => cmd_remove(&args),
            // === /PKG-002 ===
            // === RT-005 ===
            Command::Types(args) => cmd_types(&args),
            // === /RT-005 ===
            // === ADR-5 ===
            Command::Check(args) => cmd_check(&args),
            // === /ADR-5 ===
            // === TEST-001 ===
            Command::Test(args) => cmd_test(&args),
            // === /TEST-001 ===
            // === TASK-001 ===
            Command::Task(args) => cmd_task(&args),
            // === /TASK-001 ===
            // === RUN-001 ===
            Command::Dev(args) => cmd_dev(&args),
            // === /RUN-001 ===
            // === RT-001 ===
            // === TOOL-003 ===
            Command::Bundle(args) => cmd_bundle(&args),
            Command::Fmt(args) => cmd_fmt(&args),
            Command::Lint(args) => cmd_lint(&args),
            // === /TOOL-003 ===
            // === RT-001 ===
            Command::Run(args) => cmd_run(&args),
            Command::NodeEval(args) => cmd_node_eval(args),
            // === OBS-001 ===
            Command::WhyDep(args) => cmd_why_dep(&args),
            // === /OBS-001 ===
            // === WHY-LARGE ===
            Command::WhyLarge(args) => cmd_why_large(&args),
            // === /WHY-LARGE ===
            // === WHY-SLOW ===
            Command::WhySlow(args) => cmd_why_slow(&args),
            // === /WHY-SLOW ===
            // === EPHEMERAL-X ===
            Command::X(args) => cmd_x(&args),
            // === /EPHEMERAL-X ===
            Command::Doctor => cmd_doctor(),
            Command::Ls => cmd_ls(),
        }
    }
}

// === RT-005 ===
fn shadow_type_files() -> Vec<(String, &'static str)> {
    let mut files = Vec::with_capacity(1 + meow_runtime::native::NATIVE_MODULES.len());
    files.push((
        meow_config::STRICT_WEB_DTS_FILE.to_owned(),
        meow_runtime::web::STRICT_WEB_DTS,
    ));
    for name in meow_runtime::native::NATIVE_MODULES {
        if let Some(decl) = meow_runtime::native::native_module_declaration(name) {
            files.push((format!("types/meow/{name}.d.ts"), decl));
        }
    }
    files
}

fn find_runtime_workspace_root(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        if current.join("Cargo.toml").is_file()
            && current.join("crates/runtime/Cargo.toml").is_file()
        {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

fn cmd_types(args: &TypesArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow types: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let workspace_root = match find_runtime_workspace_root(&cwd) {
        Some(root) => root,
        None => {
            hiss(&format!(
                "meow types: cannot find the meow workspace root from {}; run this inside the repo checkout",
                cwd.display()
            ));
            return ExitCode::FAILURE;
        }
    };
    let runtime_root = workspace_root.join("crates/runtime");
    let source_dir = runtime_root.join("src/js/meow");
    let committed_types_dir = runtime_root.join("types/meow");
    let home_dir = crate::host::host_home();
    let meow_tsc = crate::host::host_meow_tsc();
    let env = meow_runtime::typegen::TypegenEnv {
        project_root: &workspace_root,
        home_dir: &home_dir,
        meow_tsc: meow_tsc.as_deref(),
    };
    let layout = meow_runtime::typegen::TypegenLayout {
        source_dir: &source_dir,
        committed_types_dir: &committed_types_dir,
    };

    let result = if args.emit {
        meow_runtime::typegen::emit_to_dir(&env, &layout, &committed_types_dir).map(|_| ())
    } else {
        meow_runtime::typegen::check_against_dir(&env, &layout)
    };

    match result {
        Ok(()) => {
            if args.emit {
                purr("meow types: regenerated crates/runtime/types/meow/*.d.ts");
            } else {
                purr("meow types: declarations are fresh");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            hiss(&format!("meow types: {err}"));
            ExitCode::FAILURE
        }
    }
}
// === TOOL-001 ===
fn cmd_lint(args: &PathArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow lint: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let report = match meow_tool::lint_paths(&root, &args.paths) {
        Ok(report) => report,
        Err(err) => {
            hiss(&format!("meow lint: {err}"));
            return ExitCode::FAILURE;
        }
    };

    for diag in &report.diagnostics {
        ui().diagnostic(&meow_ui::SourceDiagnostic {
            path: diag.path.to_str().unwrap_or("<invalid path>"),
            source: diag.source.as_ref(),
            span: diag.span,
            message: diag.message.as_str(),
            label: diag.label.as_deref(),
            help: None,
            note: None,
        });
    }

    if report.diagnostics.is_empty() {
        purr("meow lint: no diagnostics");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn cmd_fmt(args: &FmtArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow fmt: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let report = match meow_tool::format_paths(
        &root,
        &args.paths,
        meow_tool::FormatOptions { check: args.check },
    ) {
        Ok(report) => report,
        Err(err) => {
            hiss(&format!("meow fmt: {err}"));
            return ExitCode::FAILURE;
        }
    };

    for diag in &report.diagnostics {
        ui().diagnostic(&meow_ui::SourceDiagnostic {
            path: diag.path.to_str().unwrap_or("<invalid path>"),
            source: diag.source.as_ref(),
            span: diag.span,
            message: diag.message.as_str(),
            label: diag.label.as_deref(),
            help: None,
            note: None,
        });
    }

    if !report.diagnostics.is_empty() || (args.check && !report.changed.is_empty()) {
        return ExitCode::FAILURE;
    }

    if report.changed.is_empty() {
        if args.check {
            purr("meow fmt: check passed");
        } else {
            purr("meow fmt: no files changed");
        }
    } else {
        purr("meow fmt: formatted files");
    }

    ExitCode::SUCCESS
}

fn cmd_bundle(args: &BundleArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow bundle: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    // Validate entries + determine output directory
    let plan = match meow_tool::plan_bundle(&root, &args.entries, args.out.clone()) {
        Ok(plan) => plan,
        Err(err) => {
            hiss(&format!("meow bundle: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let out_dir = plan.out.clone().unwrap_or_else(|| root.join("dist"));

    // Build a resolver from the project context
    let async_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            hiss(&format!("meow bundle: cannot start async runtime: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let result: Result<Vec<PathBuf>, String> = async_rt.block_on(async {
        let ctx = build_runtime_context(&root, false).map_err(|e| e.to_string())?;
        let resolver = meow_loader::Resolver::from_resolution(
            &ctx.graph,
            ctx.cache.clone(),
            ctx.project_root.clone(),
            meow_runtime::native::native_module_registry(),
        );
        meow_tool::bundle_entries(&resolver, &root, &plan.entries, &out_dir)
            .map_err(|e| e.to_string())
    });

    match result {
        Ok(files) => {
            let file_list: Vec<String> = files
                .iter()
                .map(|f| f.to_string_lossy().into_owned())
                .collect();
            purr(&format!(
                "meow bundle: wrote {} file{} — {}",
                files.len(),
                if files.len() == 1 { "" } else { "s" },
                file_list.join(", "),
            ));
            ExitCode::SUCCESS
        }
        Err(err) => {
            hiss(&format!("meow bundle: {err}"));
            ExitCode::FAILURE
        }
    }
}
// === /TOOL-001 ===

// === /RT-005 ===

// === CFG-001 ===
/// `meow sync` — regenerate the shadow configs from `meow.config.json` (ADR-8).
/// The binary edge owns host access (cwd) and error rendering; the library
/// (`meow-config`) stays free of ambient reads (I-6).
fn cmd_sync() -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow sync: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let cfg = match meow_config::MeowConfig::load(&root) {
        Ok(cfg) => cfg,
        Err(err) => {
            hiss(&format!("meow sync: {err}"));
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = meow_config::generate_shadow_tsconfig(&cfg, &root) {
        hiss(&format!("meow sync: {err}"));
        return ExitCode::FAILURE;
    }
    if let Err(err) = meow_config::write_root_tsconfig_shim(&root) {
        hiss(&format!("meow sync: {err}"));
        return ExitCode::FAILURE;
    }
    // === RT-004 ===
    // Drop the curated strict-web ambient decl into `.meow/` so editors + `meow check`
    // resolve the §8.1 globals (fetch/URL/crypto.subtle/…) with nothing installed. The
    // runtime owns the content (I-9, curated-from-upstream); config owns the shadow dir.
    // === RT-005 ===
    // `meow sync` also refreshes the shipped `meow:*` declarations into `.meow/types/`
    // so editors resolve modules like `meow:http` and `meow:ui` without any install step.
    let shadow_types = shadow_type_files();
    let shadow_refs = shadow_types
        .iter()
        .map(|(path, content)| (path.as_str(), *content))
        .collect::<Vec<_>>();
    if let Err(err) = meow_config::write_shadow_types(&root, &shadow_refs) {
        hiss(&format!("meow sync: {err}"));
        return ExitCode::FAILURE;
    }
    // === /RT-005 ===
    // === /RT-004 ===
    // === CFG-003 ===
    // `package.json` is user-owned after CANON Amendment 001; sync refreshes only
    // the tsconfig/type shadows and never rewrites package.json.
    // === /CFG-003 ===
    purr(
        "meow sync: regenerated .meow/tsconfig.json + .meow/strict-web.d.ts + .meow/types/meow/*.d.ts + tsconfig.json shim",
    );
    ExitCode::SUCCESS
}
// === /CFG-001 ===

// === OBS-001 ===
/// `meow why-dep <name>` — trace the dependency path(s) from the project's direct
/// deps to <name>, read from meow.lock.jsonl (PKG-001). The binary edge owns the
/// one ambient read (cwd); meow-obs stays host-pure + does no resolution (I-6, I-1).
fn cmd_why_dep(args: &WhyDepArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => find_project_root(&dir),
        Err(err) => {
            hiss(&format!(
                "meow why-dep: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let lockfile = match load_lockfile(&root) {
        Ok(lf) => lf,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    // === CFG-003 ===
    let package_json = match meow_config::PackageJson::read(&root) {
        Ok(package_json) => package_json,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    let direct = match package_json.direct_dependencies() {
        Ok(direct) => direct,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    let roots = match meow_pkg::resolve_roots(&direct, &lockfile) {
        Ok(roots) => roots,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    // === /CFG-003 ===
    let mode = if args.shortest {
        meow_obs::PathMode::Shortest
    } else {
        meow_obs::PathMode::All
    };
    let target = meow_pkg::PackageName::new(args.pkg.clone());
    let report = meow_obs::why_dep(&lockfile, &roots, &target, mode, args.limit);

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                hiss(&format!("meow why-dep: {err}"));
                return ExitCode::FAILURE;
            }
        }
        return if report.found {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    if !report.found {
        hiss(&format!(
            "meow: `{}` is not in the dependency tree (no path from any direct dependency in package.json)",
            args.pkg
        ));
        return ExitCode::FAILURE;
    }
    render_why_dep(&report);
    ExitCode::SUCCESS
}

/// Render a found `why-dep` report as prose chains (stdout).
fn render_why_dep(report: &meow_obs::WhyDep) {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} is in the dependency tree — {} version(s).",
        report.target,
        report.versions.len()
    ));
    lines.push("(each chain starts at a project direct dependency)".to_owned());
    for (idx, tv) in report.versions.iter().enumerate() {
        if idx > 0 {
            lines.push(String::new());
        }
        let integrity = match &tv.integrity {
            Some(hash) => hash.to_sri(),
            None => "none — referenced but not in lockfile".to_string(),
        };
        let direct = if tv.direct {
            "  (direct dependency)"
        } else {
            ""
        };
        lines.push(format!(
            "{}@{}  integrity {integrity}{direct}",
            tv.node.name, tv.node.version
        ));
        for path in &tv.paths {
            let chain = path
                .nodes
                .iter()
                .map(|n| format!("{}@{}", n.name, n.version))
                .collect::<Vec<_>>()
                .join(" → ");
            lines.push(format!("  {chain}"));
        }
        if tv.truncated {
            lines.push(format!(
                "  … showing first {} of more chains (raise with --limit)",
                tv.paths.len()
            ));
        }
    }
    let title = format!("why-dep {}", report.target);
    ui().panel(&title, &lines);
}
// === /OBS-001 ===

// === PKG-002 ===
const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org";
const NPM_INSTALL_METADATA_ACCEPT: &str = "application/vnd.npm.install-v1+json";
const NPM_METADATA_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const NPM_TARBALL_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
const NPM_FETCH_RETRIES: usize = 5;
const NPM_HTTP_CONCURRENCY: usize = 40;
const NPM_FETCH_INITIAL_RETRY_DELAY_MS: u64 = 100;

fn transient_registry_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

async fn sleep_before_retry(attempt: usize) {
    let factor = 1_u64 << attempt.min(6);
    tokio::time::sleep(std::time::Duration::from_millis(
        NPM_FETCH_INITIAL_RETRY_DELAY_MS * factor,
    ))
    .await;
}

/// Production npm registry client. Lives at the CLI edge so `meow-pkg` stays
/// network-free; all registry I/O is explicit here.
///
/// The reqwest client is built lazily on first use — a fully cache-hit install
/// (warm/hot) never pays the rustls/aws-lc init cost (~10-15ms).
#[derive(Clone)]
struct NpmRegistry {
    base: String,
    client: Arc<OnceLock<reqwest::Client>>,
    /// Tarball download concurrency limit (bandwidth-sensitive).
    limiter: Arc<tokio::sync::Semaphore>,
    /// Metadata fetch concurrency limit (JSON docs — small and fast, so much higher).
    metadata_limiter: Arc<tokio::sync::Semaphore>,
    /// On-disk metadata cache root: `~/.meow/cache/metadata/`.
    /// Stores raw npm registry JSON responses keyed by package name.
    /// This is a performance hint — tarball SHA-512 is always verified,
    /// so stale/forged metadata cannot compromise security (worst case:
    /// a stale version list causes a tarball cache miss → network fetch).
    metadata_cache_dir: PathBuf,
}

impl NpmRegistry {
    /// Construct without initializing the HTTP client — the reqwest/rustls
    /// stack is deferred to the first actual network call.
    fn lazy() -> Result<NpmRegistry, String> {
        let home = crate::host::host_home();
        let metadata_cache_dir = home.join(".meow").join("cache").join("metadata");
        Ok(NpmRegistry {
            base: NPM_REGISTRY_URL.to_owned(),
            client: Arc::new(OnceLock::new()),
            limiter: Arc::new(tokio::sync::Semaphore::new(NPM_HTTP_CONCURRENCY)),
            metadata_limiter: Arc::new(tokio::sync::Semaphore::new(256)),
            metadata_cache_dir,
        })
    }

    /// Eagerly construct with the HTTP client built now (used by `meow add`
    /// which always needs the registry).
    fn npm() -> Result<NpmRegistry, String> {
        let reg = Self::lazy()?;
        let _ = reg.client();
        Ok(reg)
    }

    fn metadata_cache_path(&self, name: &meow_pkg::PackageName) -> PathBuf {
        // Escape `/` in scoped package names for flat-file storage.
        let escaped = name.as_str().replace('/', "+");
        self.metadata_cache_dir.join(format!("{escaped}.json"))
    }

    /// Compact metadata cache: stores only the fields the resolver needs,
    /// not the full npm document. A package like zod has ~1MB of raw JSON
    /// but only ~50KB of useful resolution data (versions + deps + tarball URLs).
    fn metadata_cache_compact_path(&self, name: &meow_pkg::PackageName) -> PathBuf {
        let escaped = name.as_str().replace('/', "+");
        self.metadata_cache_dir
            .join(format!("{escaped}.compact.json"))
    }

    fn client(&self) -> Result<&reqwest::Client, meow_pkg::RegistryError> {
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let client = reqwest::Client::builder()
            .user_agent(concat!("meow/", env!("CARGO_PKG_VERSION")))
            .http2_adaptive_window(true)
            .pool_max_idle_per_host(NPM_HTTP_CONCURRENCY)
            .build()
            .map_err(|err| meow_pkg::RegistryError::Fetch {
                target: "http client init".to_owned(),
                reason: format!("cannot initialize npm HTTP client: {err}"),
            })?;
        let _ = self.client.set(client);
        Ok(self.client.get().expect("client was just initialized"))
    }

    fn base_url(&self) -> &str {
        &self.base
    }

    fn metadata_url(&self, name: &meow_pkg::PackageName) -> String {
        format!("{}/{}", self.base, name.as_str())
    }

    async fn acquire_http_permit(
        &self,
        target: &str,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, meow_pkg::RegistryError> {
        self.limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|err| meow_pkg::RegistryError::Fetch {
                target: target.to_owned(),
                reason: err.to_string(),
            })
    }

    async fn fetch_metadata_async(
        &self,
        name: &meow_pkg::PackageName,
    ) -> Result<meow_pkg::PackageMetadata, meow_pkg::RegistryError> {
        let trace = std::env::var_os("MEOW_INSTALL_TRACE").is_some();
        let t0 = std::time::Instant::now();

        // Compact metadata cache: synchronous read + parse for small files.
        // The compact cache is 10-100x smaller than the raw npm document,
        // so reading + parsing on the async thread is faster than the
        // spawn_blocking thread pool hop overhead (~1ms per hop).
        let compact_path = self.metadata_cache_compact_path(name);
        if let Ok(bytes) = std::fs::read(&compact_path) {
            if let Ok(meta) = serde_json::from_slice::<meow_pkg::PackageMetadata>(&bytes) {
                if trace {
                    eprintln!(
                        "[trace] metadata compact HIT {name}: {}μs",
                        t0.elapsed().as_micros()
                    );
                }
                return Ok(meta);
            }
        }

        // Fall back to raw JSON cache (larger, needs spawn_blocking for parse).
        let raw_path = self.metadata_cache_path(name);
        if let Ok(bytes) = std::fs::read(&raw_path) {
            let bytes_for_parse = bytes.clone();
            let parse_result = tokio::task::spawn_blocking(move || {
                serde_json::from_slice::<meow_pkg::PackageMetadata>(&bytes_for_parse)
            })
            .await
            .map_err(|err| meow_pkg::RegistryError::Fetch {
                target: "metadata cache parse".to_owned(),
                reason: err.to_string(),
            })?;
            if let Ok(meta) = parse_result {
                // Write compact cache from the parsed metadata.
                let compact_path = compact_path.clone();
                let meta_clone = meta.clone();
                tokio::task::spawn_blocking(move || {
                    let _ = std::fs::create_dir_all(compact_path.parent().unwrap_or(&compact_path));
                    if let Ok(compact) = serde_json::to_vec(&meta_clone) {
                        let tmp = compact_path.with_extension("compact.json.tmp");
                        let _ = std::fs::write(&tmp, &compact);
                        let _ = std::fs::rename(&tmp, &compact_path);
                    }
                })
                .await
                .ok();
                if trace {
                    eprintln!(
                        "[trace] metadata raw HIT {name}: {}μs",
                        t0.elapsed().as_micros()
                    );
                }
                return Ok(meta);
            }
        }

        if trace {
            eprintln!("[trace] metadata cache MISS {name}, fetching from network");
        }

        let url = self.metadata_url(name);
        let client = self.client()?;
        for attempt in 0..NPM_FETCH_RETRIES {
            let _permit = self
                .metadata_limiter
                .clone()
                .acquire_owned()
                .await
                .map_err(|err| meow_pkg::RegistryError::Fetch {
                    target: url.clone(),
                    reason: err.to_string(),
                })?;
            let response = match client
                .get(&url)
                .header("Accept", NPM_INSTALL_METADATA_ACCEPT)
                .send()
                .await
            {
                Ok(response) => response,
                Err(err) if attempt + 1 < NPM_FETCH_RETRIES => {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    drop(err);
                    continue;
                }
                Err(err) => {
                    return Err(meow_pkg::RegistryError::Fetch {
                        target: url,
                        reason: err.to_string(),
                    });
                }
            };

            let status = response.status();
            if !status.is_success() {
                if attempt + 1 < NPM_FETCH_RETRIES && transient_registry_status(status.as_u16()) {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    continue;
                }
                return Err(meow_pkg::RegistryError::Status {
                    url,
                    status: status.as_u16(),
                });
            }

            let bytes = match read_limited_response(response, NPM_METADATA_LIMIT_BYTES).await {
                Ok(bytes) => bytes,
                Err(err) if attempt + 1 < NPM_FETCH_RETRIES => {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    drop(err);
                    continue;
                }
                Err(err) => {
                    return Err(meow_pkg::RegistryError::Metadata {
                        name: name.to_string(),
                        reason: err,
                    });
                }
            };

            match serde_json::from_slice::<meow_pkg::PackageMetadata>(&bytes) {
                Ok(meta) => {
                    // Write both raw + compact caches (best-effort, non-blocking).
                    let raw_path = self.metadata_cache_path(name);
                    let compact_path = self.metadata_cache_compact_path(name);
                    let bytes_to_write = bytes.clone();
                    let meta_to_write = meta.clone();
                    tokio::task::spawn_blocking(move || {
                        let dir = raw_path.parent().unwrap_or(&raw_path);
                        let _ = std::fs::create_dir_all(dir);
                        // Raw cache (for debugging/fallback)
                        let tmp = raw_path.with_extension("json.tmp");
                        let _ = std::fs::write(&tmp, &bytes_to_write);
                        let _ = std::fs::rename(&tmp, &raw_path);
                        // Compact cache (what we actually read on warm)
                        if let Ok(compact) = serde_json::to_vec(&meta_to_write) {
                            let tmp = compact_path.with_extension("compact.json.tmp");
                            let _ = std::fs::write(&tmp, &compact);
                            let _ = std::fs::rename(&tmp, &compact_path);
                        }
                    })
                    .await
                    .ok();
                    return Ok(meta);
                }
                Err(err) if attempt + 1 < NPM_FETCH_RETRIES => {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    drop(err);
                }
                Err(err) => {
                    return Err(meow_pkg::RegistryError::Metadata {
                        name: name.to_string(),
                        reason: err.to_string(),
                    });
                }
            }
        }

        Err(meow_pkg::RegistryError::Fetch {
            target: url,
            reason: "metadata retries exhausted".to_owned(),
        })
    }

    async fn fetch_tarball_async(&self, url: &str) -> Result<Vec<u8>, meow_pkg::RegistryError> {
        let client = self.client()?;
        for attempt in 0..NPM_FETCH_RETRIES {
            let _permit = self.acquire_http_permit(url).await?;
            let response = match client.get(url).send().await {
                Ok(response) => response,
                Err(err) if attempt + 1 < NPM_FETCH_RETRIES => {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    drop(err);
                    continue;
                }
                Err(err) => {
                    return Err(meow_pkg::RegistryError::Fetch {
                        target: url.to_owned(),
                        reason: err.to_string(),
                    });
                }
            };

            let status = response.status();
            if !status.is_success() {
                if attempt + 1 < NPM_FETCH_RETRIES && transient_registry_status(status.as_u16()) {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    continue;
                }
                return Err(meow_pkg::RegistryError::Status {
                    url: url.to_owned(),
                    status: status.as_u16(),
                });
            }

            match read_limited_response(response, NPM_TARBALL_LIMIT_BYTES).await {
                Ok(bytes) => return Ok(bytes),
                Err(err) if attempt + 1 < NPM_FETCH_RETRIES => {
                    drop(_permit);
                    sleep_before_retry(attempt).await;
                    drop(err);
                }
                Err(err) => {
                    return Err(meow_pkg::RegistryError::Fetch {
                        target: url.to_owned(),
                        reason: err,
                    });
                }
            }
        }

        Err(meow_pkg::RegistryError::Fetch {
            target: url.to_owned(),
            reason: "tarball retries exhausted".to_owned(),
        })
    }
}

impl meow_pkg::RegistrySource for NpmRegistry {
    fn fetch_metadata<'a>(
        &'a self,
        name: &'a meow_pkg::PackageName,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<meow_pkg::PackageMetadata, meow_pkg::RegistryError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { self.fetch_metadata_async(name).await })
    }

    fn fetch_tarball<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, meow_pkg::RegistryError>> + Send + 'a>> {
        Box::pin(async move { self.fetch_tarball_async(url).await })
    }

    fn has_metadata_cache(&self, name: &meow_pkg::PackageName) -> bool {
        self.metadata_cache_compact_path(name).exists()
    }
}

async fn read_limited_response(response: reqwest::Response, limit: u64) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|len| len > limit) {
        return Err(format!(
            "the response body is larger than request limit: {limit}"
        ));
    }
    let bytes = response.bytes().await.map_err(|err| err.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "the response body is larger than request limit: {limit}"
        ));
    }
    Ok(bytes.to_vec())
}

enum InstallSuccess {
    Materialized {
        installed: usize,
        lock_path: PathBuf,
        report: meow_pkg::MaterializeReport,
    },
}

fn default_install_args() -> InstallArgs {
    InstallArgs {
        mode: InstallMode::Materialize,
        materialize: false,
        vendor: false,
        vendor_dir: PathBuf::from("vendor"),
        clean: false,
        packages: Vec::new(),
    }
}

fn cmd_add(args: &PkgArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow add: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            hiss(&format!("meow add: cannot start async resolver: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let outcome: Result<Vec<(meow_pkg::PackageName, meow_pkg::VersionReq)>, String> = runtime
        .block_on(async {
            let registry = NpmRegistry::npm()?;
            let mut resolved = Vec::with_capacity(args.packages.len());
            for package in &args.packages {
                let (name, req) = requested_dependency(&registry, package)
                    .await
                    .map_err(|err| err.to_string())?;
                meow_config::add_dependency(&root, name.clone(), req.clone())
                    .map_err(|err| err.to_string())?;
                resolved.push((name, req));
            }
            Ok(resolved)
        });

    let resolved = match outcome {
        Ok(resolved) => resolved,
        Err(err) => {
            hiss(&format!("meow add: {err}"));
            return ExitCode::FAILURE;
        }
    };

    // === PKG-003 (global install) ===
    if args.global {
        return cmd_add_global(resolved);
    }
    // === /PKG-003 ===

    for (name, req) in resolved {
        purr(&format!("added {name}@{}", req.as_str()));
    }
    cmd_install(&default_install_args())
}

/// Install packages globally: resolve, install into a dedicated global workspace,
/// write shell shims to `~/.meow/bin/`.
fn cmd_add_global(resolved: Vec<(meow_pkg::PackageName, meow_pkg::VersionReq)>) -> ExitCode {
    let meow_home = crate::host::host_home().join(".meow");
    let global_root = meow_home.join("global");
    let bin_dir = meow_home.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap_or(());
    std::fs::create_dir_all(global_root.join("node_modules")).unwrap_or(());

    let u = ui();
    let mut succeeded = 0;

    for (name, _req) in &resolved {
        // Write a shim script
        let bin_name = name.as_str().rsplit('/').next().unwrap_or(name.as_str());
        let shim_path = bin_dir.join(bin_name);
        let shim_content = format!(
            r#"#!/bin/sh
exec meow x "{}" "$@"
"#,
            name.as_str(),
        );
        match std::fs::write(&shim_path, shim_content.as_bytes()) {
            Ok(_) => {
                // Make executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &shim_path,
                        std::fs::Permissions::from_mode(0o755),
                    );
                }
                u.purr(&format!("Globally installed {bin_name}"));
                succeeded += 1;
            }
            Err(err) => {
                hiss(&format!(
                    "meow add -g: cannot write shim for {bin_name}: {err}"
                ));
            }
        }
    }

    if succeeded > 0 {
        u.info(&format!(
            "Bin directory: {} (add to $PATH if not already)",
            bin_dir.display(),
        ));
    }

    if succeeded == resolved.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn cmd_remove(args: &PkgArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow remove: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    for package in &args.packages {
        let (name, req) = match split_package_arg(package) {
            Ok(parsed) => parsed,
            Err(err) => {
                hiss(&format!("meow remove: {err}"));
                return ExitCode::FAILURE;
            }
        };
        if req.is_some() {
            hiss(&format!(
                "meow remove: package specifier {package:?} includes a version; remove by package name"
            ));
            return ExitCode::FAILURE;
        }
        if let Err(err) = meow_config::remove_dependency(&root, &name) {
            hiss(&format!("meow remove: {err}"));
            return ExitCode::FAILURE;
        }
        purr(&format!("removed {name}"));

        // === PKG-003 (global remove) ===
        if args.global {
            let meow_home = crate::host::host_home().join(".meow");
            let bin_dir = meow_home.join("bin");
            let bin_name = name.as_str().rsplit('/').next().unwrap_or(name.as_str());
            let shim_path = bin_dir.join(bin_name);
            if shim_path.exists() {
                if let Err(err) = std::fs::remove_file(&shim_path) {
                    hiss(&format!(
                        "meow remove -g: cannot remove shim {}: {err}",
                        shim_path.display(),
                    ));
                    return ExitCode::FAILURE;
                }
                purr(&format!("removed global shim {bin_name}"));
            }
        }
        // === /PKG-003 ===
    }

    cmd_install(&default_install_args())
}

/// `meow install`: resolve declared deps, populate the cache, and write the lockfile.
fn cmd_install(args: &InstallArgs) -> ExitCode {
    let started = std::time::Instant::now();
    let projection = match install_projection(args) {
        Ok(projection) => projection,
        Err(err) => {
            hiss(&format!("meow install: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow install: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let mut bar = ui().progress(0, "resolving dependencies");
    let bar_animate = bar.animate();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            bar.clear();
            hiss(&format!(
                "meow install: cannot start async installer: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let outcome: Result<InstallSuccess, String> = runtime.block_on(async {
        let trace = std::env::var_os("MEOW_INSTALL_TRACE").is_some();
        let t0 = std::time::Instant::now();
        let registry = NpmRegistry::lazy()?;
        // === CFG-003 ===
        for package in &args.packages {
            let (name, req) = requested_dependency(&registry, package)
                .await
                .map_err(|err| err.to_string())?;
            meow_config::add_dependency(&root, name, req).map_err(|err| err.to_string())?;
        }
        let package_json = load_install_package_json(&root)?;
        let direct_deps = package_json
            .direct_dependencies()
            .map_err(|err| err.to_string())?;
        let overrides = package_json
            .package_overrides()
            .map_err(|err| err.to_string())?;
        // === /CFG-003 ===

        let cache = meow_pkg::Cache::in_home(crate::host::host_home());
        let meow_req = runtime_meow_requirement().map_err(|err| err.to_string())?;
        let lock_path = root.join("meow.lock.jsonl");
        let t_lock = std::time::Instant::now();
        let prior_lockfile = if lock_path.exists() {
            Some(meow_pkg::Lockfile::read(&lock_path).map_err(|err| err.to_string())?)
        } else {
            None
        };
        if trace {
            eprintln!("[trace] lockfile read: {}ms", t_lock.elapsed().as_millis());
        }
        let mut installer =
            meow_pkg::Installer::new(registry.clone(), &cache, registry.base_url(), meow_req)
                .with_overrides(overrides);
        if let Some(lockfile) = prior_lockfile {
            installer = installer.with_reuse_lockfile(lockfile);
        }
        let t_resolve = std::time::Instant::now();
        let lockfile = installer
            .resolve_with_progress_async(&direct_deps, |progress| {
                let (cached, resolved_total) = match progress {
                    meow_pkg::InstallProgress::MetadataFetched {
                        cached,
                        resolved_total,
                        ..
                    } => (cached, resolved_total),
                    meow_pkg::InstallProgress::PackageDownloaded {
                        cached,
                        resolved_total,
                        ..
                    } => (cached, resolved_total),
                    meow_pkg::InstallProgress::PackageCached {
                        cached,
                        resolved_total,
                        ..
                    } => (cached, resolved_total),
                };
                if bar_animate {
                    let label = match progress {
                        meow_pkg::InstallProgress::MetadataFetched { package, .. } => {
                            format!("resolving · {package}")
                        }
                        meow_pkg::InstallProgress::PackageDownloaded { package, .. } => {
                            format!("downloading · {package}")
                        }
                        meow_pkg::InstallProgress::PackageCached { package, .. } => {
                            format!("linking · {package}")
                        }
                    };
                    bar.update(cached as u64, resolved_total as u64, label);
                } else {
                    bar.update_counts(cached as u64, resolved_total as u64);
                }
            })
            .await
            .map_err(|err| err.to_string())?;
        if trace {
            eprintln!("[trace] resolve: {}ms", t_resolve.elapsed().as_millis());
        }
        let installed = lockfile.len();
        let t_lockwrite = std::time::Instant::now();
        lockfile
            .write_canonical(&lock_path)
            .map_err(|err| err.to_string())?;
        if trace {
            eprintln!(
                "[trace] lockfile write: {}ms",
                t_lockwrite.elapsed().as_millis()
            );
        }

        // === PKG-004 ===
        let t_graph = std::time::Instant::now();
        let roots =
            meow_pkg::resolve_roots(&direct_deps, &lockfile).map_err(|err| err.to_string())?;
        let graph = meow_pkg::ResolutionGraph::assemble(std::sync::Arc::new(lockfile), roots)
            .map_err(|err| err.to_string())?;
        if trace {
            eprintln!(
                "[trace] graph assemble: {}ms",
                t_graph.elapsed().as_millis()
            );
        }
        bar.set_label("materializing node_modules");
        let t_mat = std::time::Instant::now();
        let report = meow_pkg::Materializer::new(&cache, &graph, &root)
            .materialize_async(&projection)
            .await
            .map_err(|err| err.to_string())?;
        if trace {
            eprintln!("[trace] materialize: {}ms", t_mat.elapsed().as_millis());
        }
        if trace {
            eprintln!("[trace] TOTAL: {}ms", t0.elapsed().as_millis());
        }
        Ok(InstallSuccess::Materialized {
            installed,
            lock_path,
            report,
        })
        // === /PKG-004 ===
    });
    bar.clear();

    match outcome {
        Ok(InstallSuccess::Materialized {
            installed,
            lock_path,
            report,
        }) => {
            let u = ui();
            let lock_name = lock_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("meow.lock.jsonl");
            let payload = if report.bytes_written == 0 && report.packages > 0 && !report.skipped {
                "copy-on-write".to_owned()
            } else {
                meow_ui::fmt::bytes(report.bytes_written)
            };
            let headline = format!(
                "{} {}",
                u.sigil(
                    meow_ui::Tone::Purr,
                    &format!("{} packages ready", meow_ui::fmt::count(installed as u64)),
                ),
                u.stdout_caps()
                    .dim(&format!("· {}", meow_ui::fmt::duration(started.elapsed()))),
            );
            let mut lines = vec![headline];
            lines.extend(u.kv(&[
                (
                    "materialized".to_owned(),
                    format!("{} packages · {} edges", report.packages, report.edges),
                ),
                ("disk".to_owned(), payload),
                ("lockfile".to_owned(), lock_name.to_owned()),
            ]));
            if report.skipped {
                lines.push(
                    u.stdout_caps()
                        .muted("node_modules/ skipped (already up to date)"),
                );
            }
            u.panel("meow install", &lines);
            ExitCode::SUCCESS
        }
        Err(err) => {
            hiss(&format!("meow install: {err}"));
            ExitCode::FAILURE
        }
    }
}

// === CFG-003 ===
fn load_install_package_json(root: &Path) -> Result<meow_config::PackageJson, String> {
    match meow_config::PackageJson::read(root) {
        Ok(package_json) => Ok(package_json),
        Err(meow_config::ConfigError::PackageJsonNotFound(_)) => {
            Ok(meow_config::PackageJson::default())
        }
        Err(err) => Err(err.to_string()),
    }
}
// === /CFG-003 ===

async fn requested_dependency(
    registry: &NpmRegistry,
    raw: &str,
) -> Result<(meow_pkg::PackageName, meow_pkg::VersionReq), String> {
    let (name, maybe_req) = split_package_arg(raw)?;
    let requirement = match maybe_req {
        Some(req) => resolve_requested_requirement(registry, &name, req).await?,
        None => dist_tag_requirement(registry, &name, "latest").await?,
    };
    Ok((name, requirement))
}

fn split_package_arg(raw: &str) -> Result<(meow_pkg::PackageName, Option<&str>), String> {
    if raw.is_empty() {
        return Err("empty package specifier".to_owned());
    }
    let split_at = raw
        .rmatch_indices('@')
        .find_map(|(idx, _)| (idx > 0).then_some(idx));
    let (name, req) = match split_at {
        Some(idx) => (&raw[..idx], Some(&raw[idx + 1..])),
        None => (raw, None),
    };
    if name.is_empty() {
        return Err(format!(
            "invalid package specifier {raw:?}: missing package name"
        ));
    }
    if matches!(req, Some("")) {
        return Err(format!(
            "invalid package specifier {raw:?}: missing range after `@`"
        ));
    }
    Ok((meow_pkg::PackageName::new(name), req))
}

async fn resolve_requested_requirement(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    raw_req: &str,
) -> Result<meow_pkg::VersionReq, String> {
    match meow_pkg::VersionReq::parse(raw_req) {
        Ok(req) => Ok(req),
        Err(_) => dist_tag_requirement(registry, name, raw_req).await,
    }
}

async fn dist_tag_requirement(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    tag: &str,
) -> Result<meow_pkg::VersionReq, String> {
    let metadata = registry
        .fetch_metadata_async(name)
        .await
        .map_err(|err| format!("cannot resolve {name}: {err}"))?;
    let version = metadata
        .dist_tags
        .get(tag)
        .ok_or_else(|| {
            format!(
                "unsupported requirement {tag:?} for {name}: meow install accepts semver ranges or npm dist-tags"
            )
        })?;
    meow_pkg::VersionReq::parse(&format!("^{}", version.as_str())).map_err(|err| {
        format!("cannot record resolved dist-tag {tag:?} for {name} as a semver requirement: {err}")
    })
}

fn runtime_meow_requirement() -> Result<meow_pkg::VersionReq, String> {
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|err| format!("invalid CLI version metadata: {err}"))?;
    meow_pkg::VersionReq::parse(&format!("^{}.{}", version.major, version.minor))
        .map_err(|err| format!("cannot derive runtime meow requirement: {err}"))
}

// === PKG-004 ===
fn install_projection(args: &InstallArgs) -> Result<meow_pkg::MaterializeOptions, String> {
    let selection = if args.materialize {
        InstallMode::Materialize
    } else if args.vendor {
        InstallMode::Vendor
    } else {
        args.mode.clone()
    };

    if matches!(selection, InstallMode::Materialize) && args.vendor_dir != Path::new("vendor") {
        return Err("`--vendor-dir` requires `--vendor` or `--mode vendor`".to_owned());
    }

    match selection {
        InstallMode::Materialize => {
            let mut opts = meow_pkg::MaterializeOptions::node_modules();
            opts.clean = args.clean;
            Ok(opts)
        }
        InstallMode::Vendor => {
            let mut opts = meow_pkg::MaterializeOptions::vendor();
            opts.clean = args.clean;
            opts.vendor_dir = args.vendor_dir.clone();
            Ok(opts)
        }
    }
}
// === /PKG-004 ===

// === /PKG-002 ===

// === RT-006 ===
#[derive(Debug, Clone, Copy)]
struct RunFlagView<'a> {
    argv: &'a [String],
    allow_clock: bool,
    allow_random: bool,
    allow_env: &'a Option<String>,
    trust: bool,
    max_old_space_size: Option<usize>,
    no_snapshot: bool,
    v8_flags: Option<&'a str>,
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
enum RunCommandError {
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
struct RuntimeContext {
    project_dir: PathBuf,
    project_root: meow_runtime::ModuleSpecifier,
    node_mode: meow_runtime::node::NodeMode,
    graph: std::sync::Arc<meow_pkg::ResolutionGraph>,
    cache: std::sync::Arc<meow_pkg::Cache>,
}

#[derive(Debug, Clone)]
struct NativeRunRequest {
    project_dir: PathBuf,
    process_cwd: PathBuf,
    spec: meow_runtime::ModuleSpecifier,
    main_module: Option<String>,
    argv1: Option<String>,
    argv: Vec<String>,
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
fn cmd_node_eval(args: NodeEvalArgs) -> ExitCode {
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

fn cmd_run(args: &RunArgs) -> ExitCode {
    cmd_run_inner("run", &args.target, run_flags(args))
}

// === RUN-001 ===
fn cmd_dev(args: &RunScriptArgs) -> ExitCode {
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

struct RuntimeNodeBridge {
    resolver: meow_loader::Resolver,
    store: meow_pkg::UnpackedStore,
}

impl RuntimeNodeBridge {
    fn new(
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

fn host_env_map(
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

async fn run_native_request(
    request: &NativeRunRequest,
    flags: RunFlagView<'_>,
    env: BTreeMap<String, String>,
) -> Result<ExitCode, RunCommandError> {
    let mut env = env;
    let host_home = crate::host::host_home();
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

fn build_runtime_context(
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
    let cache = std::sync::Arc::new(meow_pkg::Cache::in_home(crate::host::host_home()));
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
    let home = crate::host::host_home();
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
    std::fs::copy(exe, shim_path).map(|_| ()).map_err(|source| {
        RunCommandError::Message(format!(
            "cannot install node shim {} from {}: {source}",
            shim_path.display(),
            exe.display()
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

// === LOAD-001 ===
/// Walk UP from `start` to the nearest directory holding a `meow.lock.jsonl` or
/// `package.json` and return it; fall back to `start` when neither exists (a
/// local-only run). Host-pure: it inspects only the given path, reads no env
/// (`$HOME` stays in `host/`, I-6).
fn find_project_root(start: &std::path::Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join("meow.lock.jsonl").is_file()
            || (dir.join("package.json").is_file() && !is_inside_node_modules(dir))
        {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

fn is_inside_node_modules(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(name) if name == "node_modules"
        )
    })
}
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
// === LOAD-001 ===

/// Read `meow.lock.jsonl` from the project root when present. A missing lockfile is
/// the empty execution contract (local-only run); a malformed lockfile is an honest
/// error, never silently ignored (I-7).
fn load_lockfile(
    project_root: &std::path::Path,
) -> Result<meow_pkg::Lockfile, meow_pkg::LockError> {
    let lock_path = project_root.join("meow.lock.jsonl");
    if !lock_path.exists() {
        return Ok(meow_pkg::Lockfile::new());
    }
    meow_pkg::Lockfile::read(&lock_path)
}

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
// === /LOAD-001 ===

// === UX-002 (terminal UX engine wiring) ===
/// Wall-clock instant captured at process entry, for the cold-start flex on
/// `meow dev`. Set once from `main` before clap parsing.
pub static PROCESS_START: OnceLock<std::time::Instant> = OnceLock::new();

/// Record the process-entry instant. Called first thing in `main`.
pub fn mark_start() {
    let _ = PROCESS_START.set(std::time::Instant::now());
}

fn cold_start() -> std::time::Duration {
    PROCESS_START.get().map(|t| t.elapsed()).unwrap_or_default()
}

fn meow_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn mode_label(mode: meow_runtime::node::NodeMode) -> &'static str {
    match mode {
        meow_runtime::node::NodeMode::StrictWeb => "strict-web",
        _ => "node-compat",
    }
}

/// On-brand clap help styling using the meow-ui palette (truecolor when supported,
/// graceful ANSI fallback otherwise). clap honors NO_COLOR.
fn meow_help_styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Styles};
    use meow_ui::palette::Rgb;
    // Map brand RGB → ANSI for terminals that don't support truecolor
    fn ansi(rgb: Rgb) -> AnsiColor {
        // Approximate the brand palette to the closest standard ANSI color
        if rgb == Rgb::FLOSS || rgb == Rgb::MAGENTA {
            AnsiColor::BrightMagenta
        } else if rgb == Rgb::VIOLET {
            AnsiColor::Magenta
        } else if rgb == Rgb::SKY {
            AnsiColor::Cyan
        } else if rgb == Rgb::HONEY {
            AnsiColor::BrightYellow
        } else if rgb == Rgb::CATNIP {
            AnsiColor::BrightGreen
        } else {
            AnsiColor::White
        }
    }
    Styles::styled()
        .header(ansi(Rgb::VIOLET).on_default().bold())
        .usage(ansi(Rgb::VIOLET).on_default().bold())
        .literal(ansi(Rgb::FLOSS).on_default())
        .placeholder(ansi(Rgb::SKY).on_default())
        .error(ansi(Rgb::HISS).on_default().bold())
        .valid(ansi(Rgb::CATNIP).on_default())
}

/// The no-args landing screen: wordmark, tagline, grouped commands, then system
/// telemetry (installation method, cache size, shadow-binary detection).
fn cmd_landing() -> ExitCode {
    let u = ui();
    u.landing(meow_version(), &command_catalog());

    // === UX-003 (landing page telemetry) ===
    let home = crate::host::host_home();
    let meow_home = home.join(".meow");

    // Installation method detection
    let install_method = detect_install_method();

    // Cache info
    let cache_dir = meow_home.join("cache").join("unpacked");
    let (cache_size, cache_packages) = dir_stats(&cache_dir);

    let caps = u.stdout_caps();
    let mut lines = Vec::new();

    lines.push(format!(
        "{} {}",
        caps.brand("meow"),
        caps.dim(&format!("v{} ({})", meow_version(), install_method,)),
    ));

    if cache_packages > 0 {
        lines.push(caps.muted(&format!(
            "Cache: {} ({} · {} packages)",
            cache_dir.display(),
            meow_ui::fmt::bytes(cache_size),
            cache_packages,
        )));
    }

    // Shadow binary detection — scan PATH for other meow installations
    let current_exe = std::env::current_exe().ok();
    let mut shadows: Vec<String> = Vec::new();
    if let Some(ref cur) = current_exe {
        if let Ok(cur_resolved) = std::fs::canonicalize(cur) {
            if let Ok(paths) = std::env::var("PATH") {
                for dir in paths.split(':') {
                    let candidate = std::path::Path::new(dir).join("meow");
                    if !candidate.exists() {
                        continue;
                    }
                    let cand_resolved = std::fs::canonicalize(&candidate).unwrap_or(candidate);
                    if cand_resolved != cur_resolved {
                        if let Some(name) = cand_resolved.to_str() {
                            shadows.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    if !shadows.is_empty() {
        lines.push(String::new());
        lines.push(caps.paint(
            meow_ui::palette::Rgb::HONEY,
            "⚠ Multiple meow installations detected!",
        ));
        for sh in &shadows {
            lines.push(format!("  You are running: {}", caps.dim(sh)));
        }
    }

    if !lines.is_empty() {
        u.out("");
        for line in &lines {
            u.out(line);
        }
    }
    // === /UX-003 ===

    ExitCode::SUCCESS
}

/// Heuristic: detect how the current binary was installed.
fn detect_install_method() -> &'static str {
    let exe = match std::env::current_exe() {
        Ok(p) => format!("{}", p.display()),
        Err(_) => return "Manual",
    };
    if exe.contains(".meow/bin") {
        "Official Installer"
    } else if exe.contains(".cargo") {
        "Cargo"
    } else if exe.contains("homebrew") || exe.contains("Cellar") || exe.contains("brew") {
        "Homebrew"
    } else if exe.contains("node_modules") || exe.contains(".nvm") {
        "NPM"
    } else {
        "Manual"
    }
}

/// Quick directory stats: total file size and count of entries one level deep.
fn dir_stats(dir: &std::path::Path) -> (u64, usize) {
    let dir_entry = match std::fs::read_dir(dir) {
        Ok(d) => d,
        Err(_) => return (0, 0),
    };
    let mut total_size = 0u64;
    let mut count = 0usize;
    for entry in dir_entry.flatten() {
        count += 1;
        if let Ok(meta) = entry.metadata() {
            total_size += meta.len();
        }
    }
    (total_size, count)
}

fn command_catalog() -> Vec<meow_ui::CommandGroup<'static>> {
    use meow_ui::CommandGroup as Group;
    vec![
        Group {
            title: "RUN",
            commands: &[
                ("run", "Execute a file or a package.json script"),
                ("dev", "Start the dev script (meow run dev)"),
                ("task", "Run a typed task from meow.tasks.ts"),
                ("test", "Run the isolate-backed test runner"),
            ],
        },
        Group {
            title: "PACKAGES",
            commands: &[
                ("install", "Resolve and install dependencies"),
                ("add", "Add a dependency and update the lockfile"),
                ("remove", "Remove a dependency"),
                ("why-dep", "Explain why a package is in the tree"),
            ],
        },
        Group {
            title: "QUALITY",
            commands: &[
                ("check", "Typecheck the project"),
                ("lint", "Lint over the shared pipeline"),
                ("fmt", "Format the project"),
                ("bundle", "Bundle the module graph"),
            ],
        },
        Group {
            title: "INSIGHT",
            commands: &[
                ("why-slow", "Module-load timeline (cold-start)"),
                ("why-large", "Largest modules and duplicates"),
                ("doctor", "Environment, config and lockfile health"),
                ("sync", "Regenerate TypeScript configuration and types"),
                ("ls", "List active dev servers and processes"),
            ],
        },
    ]
}

// === ADR-5 ===
/// `meow check` — delegate to tsc over the shadow config and render diagnostics
/// through meow-ui. The shadow tsconfig is generated by `meow sync`.
fn cmd_check(args: &PathArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow check: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let root = find_project_root(&cwd);
    let shadow_tsconfig = root.join(".meow/tsconfig.json");

    if !shadow_tsconfig.exists() {
        hiss("meow check: no .meow/tsconfig.json found — run `meow sync` first");
        return ExitCode::FAILURE;
    }

    let tsc = find_tsc(&root);
    let tsc = match tsc {
        Some(path) => path,
        None => {
            hiss("meow check: tsc not found — install TypeScript (`npm install -D typescript`) or run `meow sync` if you already have it");
            return ExitCode::FAILURE;
        }
    };

    let targets: Vec<String> = if args.paths.is_empty() {
        Vec::new()
    } else {
        args.paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };

    let mut cmd = std::process::Command::new(&tsc);
    cmd.arg("--project")
        .arg(&shadow_tsconfig)
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false");
    for target in &targets {
        cmd.arg(target);
    }

    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) => {
            hiss(&format!("meow check: failed to run tsc: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.is_empty() { &stdout } else { &stderr };

    let errors = parse_tsc_diagnostics(combined);

    if errors.is_empty() && output.status.success() {
        purr("meow check: no type errors");
        return ExitCode::SUCCESS;
    }

    for diag in &errors {
        let source = match std::fs::read_to_string(&diag.file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Convert 1-based line/col to byte offset
        let span = line_col_to_span(&source, diag.line, diag.col);
        ui().diagnostic(&meow_ui::diagnostic::SourceDiagnostic {
            path: diag.file_path.to_str().unwrap_or("<unknown>"),
            source: &source,
            span,
            message: &diag.message,
            label: Some(&diag.code),
            help: None,
            note: None,
        });
    }

    if errors.len() == 1 {
        hiss("meow check: found 1 type error");
    } else {
        hiss(&format!("meow check: found {} type errors", errors.len()));
    }
    ExitCode::FAILURE
}

struct TscDiagnostic {
    file_path: std::path::PathBuf,
    line: usize,
    col: usize,
    code: String,
    message: String,
}

/// Parse tsc's --pretty false output: `file(line,col): error TS{code}: {message}`
fn parse_tsc_diagnostics(output: &str) -> Vec<TscDiagnostic> {
    let re = regex::Regex::new(r"^(.+)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$")
        .expect("valid tsc diagnostic regex");
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        if let Some(caps) = re.captures(line) {
            let file_path = std::path::PathBuf::from(caps.get(1).unwrap().as_str());
            let line: usize = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
            let col: usize = caps.get(3).unwrap().as_str().parse().unwrap_or(0);
            let code = caps.get(5).unwrap().as_str().to_string();
            let message = caps.get(6).unwrap().as_str().to_string();
            diagnostics.push(TscDiagnostic {
                file_path,
                line,
                col,
                code,
                message,
            });
        }
    }
    diagnostics
}

/// Convert a 1-based line/column to a byte offset (start, end) span.
/// The end is estimated as the end of the line.
fn line_col_to_span(source: &str, line: usize, col: usize) -> (usize, usize) {
    let mut current_line = 1usize;
    let mut line_start = 0usize;
    for (idx, ch) in source.char_indices() {
        if current_line == line {
            let start = (line_start + col.saturating_sub(1)).min(source.len());
            // Find end of the line for the span
            let end = source[start..]
                .find('\n')
                .map(|rel| start + rel)
                .unwrap_or(source.len());
            return (start, end);
        }
        if ch == '\n' {
            current_line += 1;
            line_start = idx + 1;
        }
    }
    // Fallback: if line is past the end, return (0, 0)
    if current_line == line {
        let start = (line_start + col.saturating_sub(1)).min(source.len());
        return (start, source.len());
    }
    (0, 0)
}

/// Find the tsc binary: check node_modules/.bin/tsc, then PATH.
fn find_tsc(project_root: &std::path::Path) -> Option<std::path::PathBuf> {
    let local = project_root.join("node_modules/.bin/tsc");
    if local.is_file() {
        return Some(local);
    }
    let local_exe = project_root.join("node_modules/.bin/tsc.cmd");
    if local_exe.is_file() {
        return Some(local_exe);
    }
    // Fall back to PATH lookup
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let candidate = dir.join("tsc");
            if candidate.is_file() {
                Some(candidate)
            } else {
                let candidate_exe = dir.join("tsc.exe");
                if candidate_exe.is_file() {
                    Some(candidate_exe)
                } else {
                    None
                }
            }
        })
    })
}

// === TEST-001 ===
// === TASK-001 ===
/// `meow task <name>` — run a package.json script by name. Delegates to the same
/// script resolution as `meow run`.
fn cmd_task(args: &TaskArgs) -> ExitCode {
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

/// `meow test` — discover test files, execute each through a hermetic isolate,
/// and render results through meow-ui. Tests import from `meow:test` for the
/// test/expect API.
fn cmd_test(_args: &TestArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow test: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    // Use cwd for test discovery — find_project_root may walk past project boundaries
    // when no meow.config.json or package.json exists in the project tree.
    let root = cwd.clone();

    let test_files = discover_test_files(&root);
    if test_files.is_empty() {
        purr("meow test: no test files found");
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            hiss(&format!("meow test: cannot start async runtime: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let test_count = test_files.len();
    let mut total_passed = 0usize;
    let mut total_failed = 0usize;

    for file in &test_files {
        let display = file.strip_prefix(&root).unwrap_or(file).display();
        let label = display.to_string();
        ui().pounce(&label);

        match runtime.block_on(run_test_file_inner(&root, file)) {
            Ok(results) => {
                for result in &results {
                    let name = result["name"].as_str().unwrap_or("<unknown>");
                    let passed = result["passed"].as_bool().unwrap_or(false);
                    if passed {
                        total_passed += 1;
                        ui().purr(&format!("  ✓ {name}"));
                    } else {
                        total_failed += 1;
                        let msg = result["error"].as_str().unwrap_or("unknown error");
                        ui().hiss(&format!("  ✗ {name}"));
                        if let Some(stack) = result["stack"].as_str() {
                            let first_line = stack.lines().next().unwrap_or(msg);
                            ui().hiss(&format!("    {first_line}"));
                        } else {
                            ui().hiss(&format!("    {msg}"));
                        }
                    }
                }
            }
            Err(err) => {
                total_failed += 1;
                ui().hiss(&format!("  ✗ {} — file error: {err}", file.display()));
            }
        }
    }

    let summary = if total_failed == 0 {
        format!(
            "{} passed · {} file{}",
            total_passed,
            test_count,
            if test_count == 1 { "" } else { "s" },
        )
    } else {
        format!(
            "{} passed, {} failed · {} file{}",
            total_passed,
            total_failed,
            test_count,
            if test_count == 1 { "" } else { "s" },
        )
    };

    let u = ui();
    let tone = if total_failed == 0 {
        meow_ui::Tone::Purr
    } else {
        meow_ui::Tone::Hiss
    };
    let lines = vec![u.sigil(tone, &summary)];
    u.panel("meow test", &lines);

    if total_failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn run_test_file_inner(root: &Path, file: &Path) -> Result<Vec<serde_json::Value>, String> {
    let ctx = build_runtime_context(root, false).map_err(|e| e.to_string())?;
    let resolver = meow_loader::Resolver::from_resolution(
        &ctx.graph,
        ctx.cache.clone(),
        ctx.project_root.clone(),
        meow_runtime::native::native_module_registry(),
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

    let caps: meow_runtime::web::NetCaps = std::sync::Arc::new(meow_runtime::AllowAll);
    let mut extensions = vec![
        meow_runtime::http_extension(),
        meow_runtime::ui_extension(),
        meow_runtime::test_extension(),
        meow_loader::cjs_resolve_extension(resolver.clone()),
    ];

    // Tests run with full hermetic by default (deterministic).
    let hermetic = meow_runtime::hermetic::HermeticConfig::default();
    meow_runtime::hermetic::pin_deterministic_intl(&hermetic);
    extensions.extend(meow_runtime::hermetic::extensions(hermetic));

    let node_argv = vec!["meow".to_owned(), file.to_string_lossy().into_owned()];
    extensions.extend(meow_runtime::node::extensions(
        meow_runtime::node::NodeOptions {
            mode: meow_runtime::node::NodeMode::StrictWeb,
            argv: node_argv,
            main_module: Some(file.to_string_lossy().into_owned()),
            cwd: root.to_path_buf(),
            env: BTreeMap::new(),
            deno_node_services: Some(deno_node_services),
            caps: Some(caps),
            user_agent: Some(format!("meow/{}", env!("CARGO_PKG_VERSION"))),
        },
    ));

    let mut runtime = meow_runtime::Runtime::new(meow_runtime::RuntimeOptions {
        module_loader: loader,
        extensions,
        max_heap_size: None,
        startup_snapshot: Some(crate::SNAPSHOT_BLOB),
        residual_lazy_js_sources: crate::RESIDUAL_LAZY_JS,
        residual_lazy_esm_sources: crate::RESIDUAL_LAZY_ESM,
        v8_flags: None,
    })
    .map_err(|e| e.to_string())?;
    runtime
        .apply_hermetic_shadows()
        .map_err(|e| e.to_string())?;

    let spec = meow_runtime::ModuleSpecifier::from_file_path(file)
        .map_err(|()| format!("invalid test file path: {}", file.display()))?;

    runtime
        .run_main_module(&spec)
        .await
        .map_err(|e| format!("{}", e))?;

    // After module evaluation, call the test runner. Results are stored in OpState
    // via the op_test_store_results op.
    runtime
        .execute_script(
            "meow:test/runner",
            String::from("globalThis.__meowTestRunAll()"),
        )
        .map_err(|e| format!("test runner error: {e}"))?;

    let result_str = runtime
        .take_test_results()
        .ok_or_else(|| "no test results stored — did the test file call test()?".to_owned())?;

    let entries: Vec<serde_json::Value> =
        serde_json::from_str(&result_str).map_err(|e| format!("test results parse error: {e}"))?;

    Ok(entries)
}

/// Discover test files in the project tree. Filters ignored directories at push time
/// to avoid traversing into node_modules, target, .git, and hidden directories.
fn discover_test_files(root: &Path) -> Vec<PathBuf> {
    const TEST_EXTENSIONS: &[&str] = &["ts", "js", "tsx", "jsx", "mts", "mjs", "cts", "cjs"];
    let mut files = Vec::new();
    let mut queue = vec![root.to_path_buf()];

    while let Some(dir) = queue.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name,
                None => continue,
            };
            if path.is_dir() {
                // Skip ignored directories before pushing
                if !file_name.starts_with('.')
                    && file_name != "node_modules"
                    && file_name != "target"
                    && file_name != "vendor"
                {
                    queue.push(path);
                }
            } else if path.is_file() {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if TEST_EXTENSIONS.contains(&ext)
                    && (stem.ends_with(".test") || stem.ends_with(".spec"))
                {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    files
}

// === WHY-LARGE ===
/// `meow why-large` — list packages in the lockfile sorted by cached size,
/// so the user can see which dependencies are the heaviest.
fn cmd_why_large(_args: &PathArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow why-large: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let root = find_project_root(&cwd);
    let lockfile = match load_lockfile(&root) {
        Ok(lf) => lf,
        Err(err) => {
            hiss(&format!("meow why-large: {err}"));
            return ExitCode::FAILURE;
        }
    };

    if lockfile.is_empty() {
        purr("meow why-large: lockfile is empty");
        return ExitCode::SUCCESS;
    }

    let cache = meow_pkg::Cache::in_home(crate::host::host_home());
    let mut entries: Vec<(String, u64)> = Vec::new();

    for entry in lockfile.iter() {
        let path = cache.path_for(&entry.integrity);
        let size = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        let label = format!("{}@{}", entry.name.as_str(), entry.version.as_str());
        entries.push((label, size));
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.1));

    let total: u64 = entries.iter().map(|(_, s)| s).sum();
    let u = ui();

    let mut lines = vec![u.sigil(
        meow_ui::Tone::Info,
        &format!(
            "{} packages · {} total",
            entries.len(),
            meow_ui::fmt::bytes(total),
        ),
    )];

    // Show top packages
    let max_show = entries.len().min(20);
    for (label, size) in &entries[..max_show] {
        let pct = if total > 0 {
            (*size as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };
        lines.push(format!(
            "{}  {:>6}  {:>3}%",
            u.stdout_caps().muted(&meow_ui::width::pad_end(label, 35)),
            meow_ui::fmt::bytes(*size),
            pct,
        ));
    }

    if entries.len() > max_show {
        lines.push(u.stdout_caps().muted(&format!(
            "… {} more packages not shown",
            entries.len() - max_show
        )));
    }

    u.panel("meow why-large", &lines);
    ExitCode::SUCCESS
}

// === WHY-SLOW ===
/// `meow why-slow [target]` — measure and report cold-start timing: process
/// init, module resolution, and total elapsed.
fn cmd_why_slow(args: &PathArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow why-slow: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let cold = cold_start();
    let root = find_project_root(&cwd);
    let u = ui();

    let mut lines = vec![u.sigil(
        meow_ui::Tone::Info,
        &format!("cold start · {}", meow_ui::fmt::duration(cold)),
    )];
    lines.push(u.stdout_caps().muted(&format!(
        "{} process init · {} meow version {}",
        meow_ui::fmt::duration(cold),
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    )));

    // If a target path was provided, measure module resolution time
    if let Some(target) = args.paths.first() {
        let resolve_start = std::time::Instant::now();
        let lockfile = load_lockfile(&root).ok();

        // Quick stat to measure filesystem access

        // Quick stat to measure filesystem access
        let resolve_time = resolve_start.elapsed();
        let meta = std::fs::metadata(target);

        lines.push(String::new());
        match meta {
            Ok(m) => {
                let file_size = meow_ui::fmt::bytes(m.len());
                lines.push(format!(
                    "{}  {}  {}",
                    u.stdout_caps()
                        .muted(&meow_ui::width::pad_end("module", 12)),
                    meow_ui::fmt::duration(resolve_time),
                    file_size,
                ));
                lines.push(format!(
                    "{}  {}",
                    u.stdout_caps().muted(&meow_ui::width::pad_end("path", 12)),
                    target.display(),
                ));
            }
            Err(_) => {
                lines.push(format!("target not found: {}", target.display()));
            }
        }

        if let Some(ref lf) = lockfile {
            if !lf.is_empty() {
                let lock_start = std::time::Instant::now();
                let dep_count = lf.len();
                let lock_time = lock_start.elapsed();
                lines.push(format!(
                    "{}  {}  {} packages",
                    u.stdout_caps()
                        .muted(&meow_ui::width::pad_end("lockfile", 12)),
                    meow_ui::fmt::duration(lock_time),
                    dep_count,
                ));
            }
        }
    }

    u.panel("meow why-slow", &lines);
    ExitCode::SUCCESS
}

// === EPHEMERAL-X ===
/// `meow x <package> [-- <args>]` — install a package into a transient temp
/// directory, run its binary immediately, then discard the workspace.
fn cmd_x(args: &XArgs) -> ExitCode {
    let u = ui();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(err) => {
            hiss(&format!("meow x: cannot resolve cwd: {err}"));
            return ExitCode::FAILURE;
        }
    };

    // 1. Parse the package spec, handling flags that might be trailing in argv.
    //    This lets users put --trust at the end like `meow x wrangler deploy --trust`.
    //    Also checks MEOW_DANGEROUSLY_DISABLE_SECURITY env var for persistent opt-out.
    let env_trust = std::env::var("MEOW_DANGEROUSLY_DISABLE_SECURITY").is_ok_and(|v| v == "1");
    let mut trust = args.trust || env_trust;
    let mut allow_clock = args.allow_clock || env_trust;
    let mut allow_random = args.allow_random || env_trust;
    let mut allow_env = if env_trust {
        Some(String::new())
    } else {
        args.allow_env.clone()
    };
    let mut package_argv: Vec<String> = Vec::with_capacity(args.argv.len());
    {
        let mut i = 0;
        while i < args.argv.len() {
            let arg = &args.argv[i];
            match arg.as_str() {
                "--trust" => {
                    trust = true;
                    i += 1;
                    continue;
                }
                "--allow-clock" => {
                    allow_clock = true;
                    i += 1;
                    continue;
                }
                "--allow-random" => {
                    allow_random = true;
                    i += 1;
                    continue;
                }
                "--allow-env" => {
                    allow_env = Some(String::new());
                    i += 1;
                    continue;
                }
                a if a.starts_with("--allow-env=") => {
                    allow_env = Some(a.strip_prefix("--allow-env=").unwrap_or("").to_string());
                    i += 1;
                    continue;
                }
                _ => {}
            }
            package_argv.push(args.argv[i].clone());
            i += 1;
        }
    }

    let (name, maybe_req) = match split_package_arg(&args.package) {
        Ok(tuple) => tuple,
        Err(err) => {
            hiss(&format!("meow x: {err}"));
            return ExitCode::FAILURE;
        }
    };

    // 2. Generate a temp workspace path
    let pid = std::process::id();
    let temp_dir = std::env::temp_dir().join(format!("meow-x-{pid}-{}", name.as_str()));
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
    if let Err(err) = std::fs::create_dir_all(&temp_dir) {
        hiss(&format!(
            "meow x: cannot create temp workspace {}: {err}",
            temp_dir.display()
        ));
        return ExitCode::FAILURE;
    }

    // 3. Create a minimal package.json
    let pkg_json_path = temp_dir.join("package.json");
    if let Err(err) = std::fs::write(&pkg_json_path, b"{}\n") {
        hiss(&format!(
            "meow x: cannot write {}: {err}",
            pkg_json_path.display()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        return ExitCode::FAILURE;
    }

    // 4. Resolve the version requirement and add the dependency
    let result: Result<(), String> = (|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("cannot start async runtime: {err}"))?;
        runtime.block_on(async {
            let registry = NpmRegistry::npm()?;
            let req = resolve_dependency_req(&registry, &name, maybe_req).await?;
            meow_config::add_dependency(&temp_dir, name.clone(), req)
                .map_err(|err| err.to_string())?;
            // 5. Install into the temp dir
            u.note(&format!("resolving {}...", name.as_str()));
            let dep_map: BTreeMap<meow_pkg::PackageName, meow_pkg::DepSpec> = {
                let pj = load_install_package_json(&temp_dir)?;
                pj.direct_dependencies().map_err(|e| format!("{e}"))?
            };
            let overrides: BTreeMap<meow_pkg::PackageName, meow_pkg::DepSpec> = {
                let pj = load_install_package_json(&temp_dir)?;
                pj.package_overrides().map_err(|e| format!("{e}"))?
            };
            let cache = std::sync::Arc::new(meow_pkg::Cache::in_home(crate::host::host_home()));
            let meow_req = runtime_meow_requirement()?;

            let installer =
                meow_pkg::Installer::new(registry.clone(), &cache, registry.base_url(), meow_req)
                    .with_overrides(overrides);
            let lockfile = installer
                .resolve_with_progress_async(&dep_map, |progress| {
                    let label = match progress {
                        meow_pkg::InstallProgress::MetadataFetched { package, .. } => {
                            format!("resolving {package}")
                        }
                        meow_pkg::InstallProgress::PackageDownloaded { package, .. } => {
                            format!("downloading {package}")
                        }
                        meow_pkg::InstallProgress::PackageCached { package, .. } => {
                            format!("linking {package}")
                        }
                    };
                    // Quick feedback via stderr — no spinner needed for ephemeral installs
                    eprint!("\r  \u{1b}[2m{label}\u{1b}[0m");
                })
                .await
                .map_err(|err| format!("{err}"))?;
            eprint!("\r\u{1b}[2K"); // clear the progress line
            let lock_path = temp_dir.join("meow.lock.jsonl");
            lockfile
                .write_canonical(&lock_path)
                .map_err(|err| format!("{err}"))?;

            let roots =
                meow_pkg::resolve_roots(&dep_map, &lockfile).map_err(|err| format!("{err}"))?;
            let graph = meow_pkg::ResolutionGraph::assemble(std::sync::Arc::new(lockfile), roots)
                .map_err(|err| format!("{err}"))?;
            let projection = meow_pkg::MaterializeOptions::node_modules();
            meow_pkg::Materializer::new(&cache, &graph, &temp_dir)
                .materialize_async(&projection)
                .await
                .map_err(|err| format!("{err}"))?;
            Ok(())
        })
    })();
    if let Err(err) = result {
        hiss(&format!(
            "meow x: failed to install {}: {err}",
            args.package
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        return ExitCode::FAILURE;
    }

    // 6. Look up the binary
    let bin_name = name.as_str().rsplit('/').next().unwrap_or(name.as_str());
    let bin_path = {
        let pkg_json_path = temp_dir
            .join("node_modules")
            .join(name.as_str())
            .join("package.json");
        let bytes = match std::fs::read(&pkg_json_path) {
            Ok(b) => b,
            Err(err) => {
                hiss(&format!(
                    "meow x: cannot find installed package `{}` (expected at {}): {err}",
                    name.as_str(),
                    pkg_json_path.display(),
                ));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        };
        let pkg_json: meow_loader::package::PackageJson = match serde_json::from_slice(&bytes) {
            Ok(pj) => pj,
            Err(err) => {
                hiss(&format!(
                    "meow x: cannot parse manifest for `{}`: {err}",
                    name.as_str(),
                ));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        };
        match pkg_json.bin_entry(bin_name) {
            Some(entry) => pkg_json_path.parent().unwrap().join(entry),
            None => {
                // Fall back to main / index.js
                hiss(&format!(
                    "meow x: package `{}` has no bin entry for `{bin_name}`",
                    name.as_str(),
                ));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        }
    };

    // 7. Print the security envelope
    if trust {
        u.warn(&format!(
            "Executing {} with full host access (--trust).",
            args.package,
        ));
    } else if allow_clock || allow_random || allow_env.is_some() {
        u.purr(&format!(
            "Executing ephemeral package {} with partial host access.",
            args.package,
        ));
    } else {
        u.pounce(&format!(
            "Executing {} in strict isolation. Set MEOW_DANGEROUSLY_DISABLE_SECURITY=1 or pass --trust to bypass.",
            args.package,
        ));
    }

    // 8. Construct and run the request
    let spec = match meow_runtime::ModuleSpecifier::from_file_path(&bin_path) {
        Ok(s) => s,
        Err(_) => {
            hiss(&format!("meow x: invalid bin path: {}", bin_path.display(),));
            let _ = std::fs::remove_dir_all(&temp_dir);
            return ExitCode::FAILURE;
        }
    };
    let project_dir = temp_dir.clone();
    let request = NativeRunRequest {
        project_dir,
        process_cwd: cwd,
        spec,
        main_module: Some(bin_path.to_string_lossy().into_owned()),
        argv1: Some(bin_path.to_string_lossy().into_owned()),
        argv: args.argv.clone(),
    };
    let flags = RunFlagView {
        argv: &package_argv,
        allow_clock,
        allow_random,
        allow_env: &allow_env,
        trust,
        max_old_space_size: args.max_old_space_size,
        no_snapshot: args.no_snapshot,
        v8_flags: args.v8_flags.as_deref(),
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
                hiss(&format!("meow x: execution failed: {err}"));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        },
        Err(err) => {
            hiss(&format!("meow x: cannot start V8 runtime: {err}"));
            let _ = std::fs::remove_dir_all(&temp_dir);
            return ExitCode::FAILURE;
        }
    };

    // 9. Clean up after the process exits
    if let Err(err) = std::fs::remove_dir_all(&temp_dir) {
        u.out(&u.stdout_caps().muted(&format!(
            "meow x: could not clean up temp workspace {}: {err}",
            temp_dir.display(),
        )));
    }

    code
}

/// Resolve a version requirement from the registry, defaulting to `latest`.
async fn resolve_dependency_req(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    maybe_req: Option<&str>,
) -> Result<meow_pkg::VersionReq, String> {
    match maybe_req {
        Some(req) => resolve_requested_requirement(registry, name, req).await,
        None => dist_tag_requirement(registry, name, "latest").await,
    }
}

/// `meow doctor` — environment, config, and lockfile health as a panel.
fn cmd_doctor() -> ExitCode {
    let u = ui();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = find_project_root(&cwd);

    let mut lines = vec![u.sigil(meow_ui::Tone::Info, &format!("meow {}", meow_version()))];

    let has_pkg = root.join("package.json").is_file();
    lines.push(doctor_row(
        &u,
        has_pkg,
        "package.json",
        if has_pkg { "found" } else { "missing" },
    ));

    let lock = root.join("meow.lock.jsonl");
    let (lock_ok, lock_status) = if lock.is_file() {
        match load_lockfile(&root) {
            Ok(lf) => (true, format!("{} packages", lf.len())),
            Err(err) => (false, format!("unreadable: {err}")),
        }
    } else {
        (false, "none — run `meow install`".to_owned())
    };
    lines.push(doctor_row(&u, lock_ok, "lockfile", &lock_status));

    let nm = root.join("node_modules").is_dir();
    lines.push(doctor_row(
        &u,
        nm,
        "node_modules",
        if nm {
            "materialized"
        } else {
            "not materialized"
        },
    ));

    let cache = crate::host::host_home().join(".meow").join("cache");
    lines.push(doctor_row(
        &u,
        cache.is_dir(),
        "cache",
        &cache.display().to_string(),
    ));

    u.panel("meow doctor", &lines);
    ExitCode::SUCCESS
}

fn doctor_row(u: &Ui, ok: bool, key: &str, value: &str) -> String {
    let tone = if ok {
        meow_ui::Tone::Purr
    } else {
        meow_ui::Tone::Warn
    };
    format!(
        "{} {}  {}",
        u.sigil(tone, ""),
        u.stdout_caps().muted(&meow_ui::width::pad_end(key, 13)),
        value
    )
}
// === /UX-002 ===
#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, unique temp dir (pid + monotonic counter — no rand/clock, P16).
    fn unit_tmp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("meow-cli-unit-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn find_project_root_walks_up_to_the_nearest_project_marker() {
        // A project root is now identified by either lockfile or package.json.
        let root = unit_tmp("root");
        std::fs::write(root.join("package.json"), "{}").expect("write package.json");
        let nested = root.join("src").join("inner");
        std::fs::create_dir_all(&nested).expect("nested dirs");

        let found = find_project_root(&nested);
        assert_eq!(
            std::fs::canonicalize(&found).expect("canon found"),
            std::fs::canonicalize(&root).expect("canon root"),
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn install_materialize_flag_parses_as_default_projection() {
        let cli = Cli::try_parse_from(["meow", "install", "--materialize"]).expect("parse cli");
        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.materialize);
        assert!(matches!(args.mode, InstallMode::Materialize));
        assert!(!args.vendor);
    }

    #[test]
    fn install_default_mode_is_materialize() {
        let cli = Cli::try_parse_from(["meow", "install"]).expect("parse cli");
        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(!args.materialize);
        assert!(matches!(args.mode, InstallMode::Materialize));
        assert!(!args.vendor);
        let opts = install_projection(&args).expect("projection selection");
        assert!(matches!(opts.projection, meow_pkg::Projection::NodeModules));
        assert!(matches!(opts.link, meow_pkg::LinkStrategy::Symlink));
    }

    #[test]
    fn dev_shorthand_parses_trailing_args() {
        let cli = Cli::try_parse_from(["meow", "dev", "--", "watch"]).expect("parse cli");
        let Some(Command::Dev(args)) = cli.command else {
            panic!("expected dev command");
        };
        assert_eq!(args.argv, vec!["watch".to_string()]);
    }

    #[test]
    fn normalize_argv_injects_run_for_entry_path() {
        let argv = normalize_argv(vec![
            OsString::from("meow"),
            OsString::from("/Users/me/.meow/cache/unpacked/sha512-abc/dist/server/start-server.js"),
            OsString::from("--flag"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("meow"),
                OsString::from("run"),
                OsString::from(
                    "/Users/me/.meow/cache/unpacked/sha512-abc/dist/server/start-server.js"
                ),
                OsString::from("--flag"),
            ]
        );
    }

    #[test]
    fn normalize_argv_maps_translated_node_run_script_to_run_command() {
        let argv = normalize_argv(vec![
            OsString::from("/Users/me/.meow/bin/node"),
            OsString::from("run"),
            OsString::from("/Users/me/project/script.js"),
            OsString::from("alpha"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("/Users/me/.meow/bin/node"),
                OsString::from("run"),
                OsString::from("/Users/me/project/script.js"),
                OsString::from("--"),
                OsString::from("alpha"),
            ]
        );
    }

    #[test]
    fn normalize_argv_maps_translated_meow_run_eval_to_internal_eval_command() {
        let argv = normalize_argv(vec![
            OsString::from("/Users/me/.meow/bin/node"),
            OsString::from("run"),
            OsString::from("-e"),
            OsString::from("console.log(123)"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("/Users/me/.meow/bin/node"),
                OsString::from("node-eval"),
                OsString::from("-e"),
                OsString::from("console.log(123)"),
            ]
        );
    }

    #[test]
    fn normalize_argv_maps_meow_eval_to_internal_eval_command() {
        let argv = normalize_argv(vec![
            OsString::from("/Users/me/meow/target/debug/meow"),
            OsString::from("-e"),
            OsString::from("console.log(123)"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("/Users/me/meow/target/debug/meow"),
                OsString::from("node-eval"),
                OsString::from("-e"),
                OsString::from("console.log(123)"),
            ]
        );
    }

    #[test]
    fn normalize_argv_maps_node_eval_to_internal_eval_command() {
        let argv = normalize_argv(vec![
            OsString::from("/Users/me/.meow/bin/node"),
            OsString::from("-e"),
            OsString::from("console.log(123)"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("/Users/me/.meow/bin/node"),
                OsString::from("node-eval"),
                OsString::from("-e"),
                OsString::from("console.log(123)"),
            ]
        );
    }

    #[test]
    fn normalize_argv_maps_node_shim_script_to_run_with_trailing_args() {
        let argv = normalize_argv(vec![
            OsString::from("/Users/me/.meow/bin/node"),
            OsString::from("/Users/me/project/node_modules/next/dist/compiled/turbopack/worker.js"),
            OsString::from("56556"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("/Users/me/.meow/bin/node"),
                OsString::from("run"),
                OsString::from(
                    "/Users/me/project/node_modules/next/dist/compiled/turbopack/worker.js"
                ),
                OsString::from("--"),
                OsString::from("56556"),
            ]
        );
    }

    #[test]
    fn normalize_argv_maps_node_shim_after_preload_flags() {
        let argv = normalize_argv(vec![
            OsString::from("node"),
            OsString::from("--require"),
            OsString::from("source-map-support/register"),
            OsString::from("worker.js"),
            OsString::from("56556"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("node"),
                OsString::from("run"),
                OsString::from("worker.js"),
                OsString::from("--"),
                OsString::from("56556"),
            ]
        );
    }

    #[test]
    fn normalize_argv_strips_deno_run_compat_flags() {
        let argv = normalize_argv(vec![
            OsString::from("meow"),
            OsString::from("run"),
            OsString::from("-A"),
            OsString::from("--unstable-bare-node-builtins"),
            OsString::from("/Users/me/.meow/cache/unpacked/sha512-abc/dist/server/start-server.js"),
        ]);
        assert_eq!(
            argv,
            vec![
                OsString::from("meow"),
                OsString::from("run"),
                OsString::from(
                    "/Users/me/.meow/cache/unpacked/sha512-abc/dist/server/start-server.js"
                ),
            ]
        );
    }

    #[test]
    fn tokenize_simple_script_rejects_shell_operators() {
        assert_eq!(
            tokenize_simple_script("node ./dev.cjs --watch").expect("simple script"),
            vec!["node", "./dev.cjs", "--watch"]
        );
        assert!(tokenize_simple_script("echo hi && echo ok").is_none());
    }

    #[test]
    fn find_project_root_ignores_dependency_package_manifests() {
        let root = unit_tmp("dependency-root");
        std::fs::write(root.join("package.json"), "{}").expect("write package.json");
        let dep = root
            .join("node_modules")
            .join(".meow")
            .join("next@15.3.5")
            .join("node_modules")
            .join("next");
        std::fs::create_dir_all(&dep).expect("dependency dirs");
        std::fs::write(dep.join("package.json"), "{}").expect("dependency package.json");
        let found = find_project_root(&dep);
        assert_eq!(
            std::fs::canonicalize(&found).expect("canon found"),
            std::fs::canonicalize(&root).expect("canon root"),
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn find_project_root_falls_back_to_start_without_project_markers() {
        // No lockfile/package.json anywhere on the way up: a local-only run uses the entry dir.
        let dir = unit_tmp("nolock");
        let found = find_project_root(&dir);
        assert_eq!(found, dir);
        std::fs::remove_dir_all(&dir).ok();
    }
}
