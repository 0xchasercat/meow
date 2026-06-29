//! The `meow` command tree (DIST-001). Every parsed verb dispatches to a concrete
//! implementation in [`commands`] — no stubs remain.
//!
//! This file owns only: the `clap` definitions (`Cli`, `Command`, the `*Args`
//! structs), the Omni-Router (`normalize_argv`), the host-edge UI helpers
//! (`ui`/`purr`/`hiss`), the shared project-root / lockfile helpers, the
//! no-args landing screen, and the `Cli::run` match that dispatches into the
//! `commands::*` submodules.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;

use clap::{Args, Parser, Subcommand, ValueEnum};
use meow_ui::Ui;

mod commands;

/// (lazy JS, lazy ESM) residual sources re-fed to deno_core under a snapshot.
pub(crate) type ResidualLazySources = (
    &'static [(&'static str, &'static str)],
    &'static [(&'static str, &'static str)],
);

pub(crate) fn ui() -> Ui {
    Ui::from_env(&crate::host::term_env())
}

pub(crate) fn purr(body: &str) {
    ui().success(body);
}

pub(crate) fn hiss(body: &str) {
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

const KNOWN_COMMANDS: &[&str] = &[
    "init",
    "run",
    "x",
    "execute",
    "dev",
    "install",
    "i",
    "add",
    "remove",
    "rm",
    "del",
    "delete",
    "uninstall",
    "task",
    "test",
    "check",
    "lint",
    "fmt",
    "bundle",
    "ls",
    "why-slow",
    "why-large",
    "why-dep",
    "doctor",
    "sync",
    "types",
];

pub(crate) fn maybe_print_command_suggestion(argv: &[OsString]) -> Option<ExitCode> {
    let raw = argv.get(1)?.to_string_lossy();
    if raw.starts_with('-') || is_known_command(&raw) || should_inject_run(&raw) {
        return None;
    }
    let suggestion = closest_command(&raw)?;
    hiss(&format!(
        "Unknown command '{}'. Did you mean '{}'?",
        raw, suggestion
    ));
    Some(ExitCode::FAILURE)
}

fn closest_command(input: &str) -> Option<&'static str> {
    KNOWN_COMMANDS
        .iter()
        .copied()
        .map(|command| (command, levenshtein(input, command)))
        .filter(|(_, distance)| *distance <= 2)
        .min_by_key(|(_, distance)| *distance)
        .map(|(command, _)| command)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0; b_chars.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != *cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_chars.len()]
}

fn is_known_command(arg: &str) -> bool {
    KNOWN_COMMANDS.contains(&arg)
}

fn is_deno_run_compat_flag(arg: &str) -> bool {
    matches!(arg, "-A" | "--allow-all" | "--unstable") || arg.starts_with("--unstable-")
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new meow project in the current directory.
    Init(InitArgs),
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
pub struct InitArgs {
    /// Project mode: strict-web (deterministic, no host access) or node-compat (full Node.js compat).
    #[arg(long, value_parser = ["strict-web", "node-compat"], default_value = "strict-web")]
    pub mode: String,
    /// Overwrite existing files if they already exist.
    #[arg(long)]
    pub force: bool,
    /// Skip installing dependencies after creating config files.
    #[arg(long)]
    pub no_install: bool,
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
#[command(disable_version_flag = true)]
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
#[command(disable_version_flag = true)]
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

// === EPHEMERAL-X ===
/// Arguments for `meow x <package> [-- <args>]`.
/// Arguments for `meow x [flags] <package> [args...]`.
/// Flags come BEFORE the package name; everything after the package is treated
/// as trailing arguments (no `--` separator required).
#[derive(Debug, Args)]
#[command(disable_version_flag = true)]
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
    /// Add package specifier(s) to devDependencies before installing.
    #[arg(short = 'D', long = "dev")]
    pub dev: bool,
    /// Write a lightweight package-lock.json compatibility marker for framework detectors.
    #[arg(long = "compat-lockfile")]
    pub compat_lockfile: bool,
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
    /// Add packages to devDependencies instead of dependencies.
    #[arg(short = 'D', long = "dev")]
    pub dev: bool,
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
            Command::Sync => commands::cmd_sync(),
            // === /CFG-001 ===
            // === PKG-002 ===
            Command::Install(args) => commands::cmd_install(&args),
            Command::Add(args) => commands::cmd_add(&args),
            Command::Remove(args) => commands::cmd_remove(&args),
            // === /PKG-002 ===
            // === RT-005 ===
            Command::Types(args) => commands::cmd_types(&args),
            // === /RT-005 ===
            // === ADR-5 ===
            Command::Check(args) => commands::cmd_check(&args),
            // === /ADR-5 ===
            // === TEST-001 ===
            Command::Test(args) => commands::cmd_test(&args),
            // === /TEST-001 ===
            // === TASK-001 ===
            Command::Task(args) => commands::cmd_task(&args),
            // === /TASK-001 ===
            // === RUN-001 ===
            Command::Dev(args) => commands::cmd_dev(&args),
            // === /RUN-001 ===
            // === RT-001 ===
            // === TOOL-003 ===
            Command::Bundle(args) => commands::cmd_bundle(&args),
            Command::Fmt(args) => commands::cmd_fmt(&args),
            Command::Lint(args) => commands::cmd_lint(&args),
            // === /TOOL-003 ===
            // === RT-001 ===
            Command::Init(args) => commands::cmd_init(&args),
            Command::Run(args) => commands::cmd_run(&args),
            Command::NodeEval(args) => commands::cmd_node_eval(args),
            // === OBS-001 ===
            Command::WhyDep(args) => commands::cmd_why_dep(&args),
            // === /OBS-001 ===
            // === WHY-LARGE ===
            Command::WhyLarge(args) => commands::cmd_why_large(&args),
            // === /WHY-LARGE ===
            // === WHY-SLOW ===
            Command::WhySlow(args) => commands::cmd_why_slow(&args),
            // === /WHY-SLOW ===
            // === EPHEMERAL-X ===
            Command::X(args) => commands::cmd_x(&args),
            // === /EPHEMERAL-X ===
            Command::Doctor => commands::cmd_doctor(),
            Command::Ls => commands::cmd_ls(),
        }
    }
}

// === LOAD-001 ===
/// Walk UP from `start` to the nearest independent project boundary. A meow
/// lockfile wins, but a non-dependency `package.json` is also a boundary: nested
/// package directories must not leak into an unrelated parent package.json.
/// Host-pure: it inspects only the given path, reads no env (`$HOME` stays in
/// `host/`, I-6).
pub(crate) fn find_project_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join("meow.lock.jsonl").is_file() {
            return dir.to_path_buf();
        }
        if dir.join("package.json").is_file() && !is_inside_node_modules(dir) {
            return dir.to_path_buf();
        }
        if dir.join(".git").exists() {
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
// === /LOAD-001 ===

/// Read `meow.lock.jsonl` from the project root when present. A missing lockfile is
/// the empty execution contract (local-only run); a malformed lockfile is an honest
/// error, never silently ignored (I-7).
pub(crate) fn load_lockfile(
    project_root: &Path,
) -> Result<meow_pkg::Lockfile, meow_pkg::LockError> {
    let lock_path = project_root.join("meow.lock.jsonl");
    if !lock_path.exists() {
        return Ok(meow_pkg::Lockfile::new());
    }
    meow_pkg::Lockfile::read(&lock_path)
}

// === UX-002 (terminal UX engine wiring) ===
/// Wall-clock instant captured at process entry, for the cold-start flex on
/// `meow dev`. Set once from `main` before clap parsing.
pub static PROCESS_START: OnceLock<std::time::Instant> = OnceLock::new();

/// Record the process-entry instant. Called first thing in `main`.
pub fn mark_start() {
    let _ = PROCESS_START.set(std::time::Instant::now());
}

pub(crate) fn cold_start() -> std::time::Duration {
    PROCESS_START.get().map(|t| t.elapsed()).unwrap_or_default()
}

pub(crate) fn meow_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub(crate) fn mode_label(mode: meow_runtime::node::NodeMode) -> &'static str {
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
fn dir_stats(dir: &Path) -> (u64, usize) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

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
    fn install_dev_flag_parses() {
        let cli = Cli::try_parse_from(["meow", "install", "-D", "typescript"]).expect("parse cli");
        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.dev);
        assert_eq!(args.packages, vec!["typescript".to_string()]);
    }

    #[test]
    fn add_dev_flag_parses() {
        let cli = Cli::try_parse_from(["meow", "add", "-D", "vitest"]).expect("parse cli");
        let Some(Command::Add(args)) = cli.command else {
            panic!("expected add command");
        };
        assert!(args.dev);
        assert_eq!(args.packages, vec!["vitest".to_string()]);
    }

    #[test]
    fn command_typo_suggests_nearest_command() {
        let argv = vec![OsString::from("meow"), OsString::from("installc")];
        assert_eq!(maybe_print_command_suggestion(&argv), Some(ExitCode::FAILURE));
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
    fn find_project_root_stops_at_nearest_nested_package() {
        let outer = unit_tmp("outer-package");
        std::fs::write(outer.join("package.json"), r#"{"dependencies":{"patchright":"^1"}}"#)
            .expect("write outer package.json");
        let inner = outer.join("examples").join("fluffybench");
        std::fs::create_dir_all(&inner).expect("inner dirs");
        std::fs::write(inner.join("package.json"), r#"{"private":true}"#)
            .expect("write inner package.json");
        let nested = inner.join("src");
        std::fs::create_dir_all(&nested).expect("nested dirs");
        let found = find_project_root(&nested);
        assert_eq!(
            std::fs::canonicalize(&found).expect("canon found"),
            std::fs::canonicalize(&inner).expect("canon inner"),
        );
        std::fs::remove_dir_all(&outer).ok();
    }

    #[test]
    fn find_project_root_stops_at_git_boundary_without_package_json() {
        let root = unit_tmp("git-boundary");
        std::fs::create_dir_all(root.join(".git")).expect("git dir");
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
    fn find_project_root_falls_back_to_start_without_project_markers() {
        // No lockfile/package.json anywhere on the way up: a local-only run uses the entry dir.
        let dir = unit_tmp("nolock");
        let found = find_project_root(&dir);
        assert_eq!(found, dir);
        std::fs::remove_dir_all(&dir).ok();
    }
}
