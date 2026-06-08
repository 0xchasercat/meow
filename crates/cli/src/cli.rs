//! The `meow` command tree (DIST-001).
//!
//! Every §19 verb exists from day one as an *honest stub*: it prints a structured,
//! machine-greppable "not yet implemented" line to stderr and exits
//! [`EXIT_UNIMPLEMENTED`]. There is no fake success path (CRAFT — "the action must
//! DO the work"). `--version`/`--help` are real (clap). Later specs flip a stub to
//! real by replacing its dispatch arm; the exhaustive `match` in [`Command::landing`]
//! makes it a compile error to add a verb without wiring it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Reserved exit code: command recognized, but its implementation has not landed
/// yet. Distinct from 0 (success), 1 (generic failure), and clap's 2 (usage error)
/// so a harness can tell "not built yet" from "you held it wrong".
pub const EXIT_UNIMPLEMENTED: u8 = 3;

/// meow — a standards-first JavaScript/TypeScript runtime + unified toolchain.
#[derive(Debug, Parser)]
#[command(name = "meow", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Execute a file or a package.json script (default mode = node-compat; `strict-web` is opt-in).
    Run(RunArgs),
    /// Shorthand for `meow run dev`.
    Dev(RunScriptArgs),
    /// Install dependencies into the virtual store.
    Install(InstallArgs),
    /// Add a dependency + update the lockfile.
    Add(PkgArgs),
    /// Remove a dependency + update the lockfile.
    Remove(PkgArgs),
    /// Run a typed task from meow.tasks.ts.
    Task(TaskArgs),
    /// Isolate-backed test runner.
    Test(TestArgs),
    /// Typecheck — delegated to the tsc/tsgo daemon (ADR-5).
    Check(PathArgs),
    /// Lint over the shared pipeline.
    Lint(PathArgs),
    /// Format over the shared pipeline.
    Fmt(PathArgs),
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
    /// Execution trace.
    Trace(RunArgs),
    /// Sampling/allocation profile.
    Profile(RunArgs),
    /// Environment / config / lockfile health.
    Doctor,
    /// Regenerate shadow configs (.meow/tsconfig.json + root tsconfig.json shim).
    Sync,
    /// Regenerate or verify the committed `meow:*` declarations (RT-005 / types-fresh).
    Types(TypesArgs),
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
}
// === /RUN-001 ===

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Virtual-store install mode (CANON §18; PKG owns the final flag surface).
    #[arg(long, value_enum, default_value_t = InstallMode::Pnp)]
    pub mode: InstallMode,
    // === PKG-004 ===
    /// Write a real node_modules/ tree (escape hatch for tools that stat() packages). §24.4.
    #[arg(long, conflicts_with_all = ["mode", "vendor"])]
    pub materialize: bool,
    /// Write a self-contained vendor/ copy (air-gapped deploys). §12.2.
    #[arg(long, conflicts_with_all = ["mode", "materialize"])]
    pub vendor: bool,
    /// Vendor directory (default "vendor"); meaningful only for vendor projection.
    #[arg(long, default_value = "vendor")]
    pub vendor_dir: PathBuf,
    /// Force full copies instead of symlinks.
    #[arg(long)]
    pub copy: bool,
    /// Remove any existing projection tree before writing.
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
    /// Regenerate `crates/runtime/types/meow/*.d.ts`.
    #[arg(long, conflicts_with = "check")]
    pub emit: bool,
    /// Verify the committed declarations are fresh (default).
    #[arg(long, conflicts_with = "emit")]
    pub check: bool,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum InstallMode {
    Pnp,
    Vfs,
    Materialize,
    Vendor,
}

#[derive(Debug, Args)]
pub struct PkgArgs {
    /// Package specifier(s), e.g. `lodash@^4`.
    #[arg(required = true)]
    pub packages: Vec<String>,
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

impl Command {
    /// `(canonical verb as the user typed it, PLAN phase where the real impl lands)`.
    /// Single source of truth for the stub message AND the per-command test table.
    /// The exhaustive `match` makes adding a `Command` variant without a landing a
    /// compile error, so the surface can never silently regress.
    pub const fn landing(&self) -> (&'static str, &'static str) {
        match self {
            Command::Run(_) => ("run", "P1"),
            Command::Dev(_) => ("dev", "P1"),
            Command::Install(_) => ("install", "P2"),
            Command::Add(_) => ("add", "P2"),
            Command::Remove(_) => ("remove", "P2"),
            Command::Task(_) => ("task", "P4"),
            Command::Test(_) => ("test", "P6"),
            Command::Check(_) => ("check", "P3"),
            Command::Lint(_) => ("lint", "P3"),
            Command::Fmt(_) => ("fmt", "P3"),
            Command::Bundle(_) => ("bundle", "P3"),
            Command::WhySlow(_) => ("why-slow", "P6"),
            Command::WhyLarge(_) => ("why-large", "P6"),
            Command::WhyDep(_) => ("why-dep", "P2"),
            Command::Trace(_) => ("trace", "P6"),
            Command::Profile(_) => ("profile", "P6"),
            Command::Doctor => ("doctor", "P6"),
            Command::Sync => ("sync", "P1"),
            Command::Types(_) => ("types", "P1"),
        }
    }
}

impl Cli {
    pub fn run(self) -> ExitCode {
        match self.command {
            // === CFG-001 ===
            Command::Sync => cmd_sync(),
            // === /CFG-001 ===
            // === PKG-002 ===
            Command::Install(args) => cmd_install(&args),
            // === /PKG-002 ===
            // === RT-005 ===
            Command::Types(args) => cmd_types(&args),
            // === /RT-005 ===
            // === RUN-001 ===
            Command::Dev(args) => cmd_dev(&args),
            // === /RUN-001 ===
            // === RT-001 ===
            Command::Run(args) => cmd_run(&args),
            // === /RT-001 ===
            // HONEST stub (CRAFT — "the action must DO the work"): no fake success path.
            // Structured, single-line, machine-greppable; stderr only; non-zero exit.
            // === OBS-001 ===
            Command::WhyDep(args) => cmd_why_dep(&args),
            // === /OBS-001 ===
            other => {
                let (verb, phase) = other.landing();
                eprintln!("meow: not yet implemented — `{verb}` lands in PLAN {phase}");
                ExitCode::from(EXIT_UNIMPLEMENTED)
            }
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
            eprintln!("meow types: cannot resolve the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let workspace_root = match find_runtime_workspace_root(&cwd) {
        Some(root) => root,
        None => {
            eprintln!(
                "meow types: cannot find the meow workspace root from {}; run this inside the repo checkout",
                cwd.display()
            );
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
                println!("meow types: regenerated crates/runtime/types/meow/*.d.ts");
            } else {
                println!("meow types: declarations are fresh");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("meow types: {err}");
            ExitCode::FAILURE
        }
    }
}
// === /RT-005 ===

// === CFG-001 ===
/// `meow sync` — regenerate the shadow configs from `meow.config.json` (ADR-8).
/// The binary edge owns host access (cwd) and error rendering; the library
/// (`meow-config`) stays free of ambient reads (I-6).
fn cmd_sync() -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("meow sync: cannot resolve the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = match meow_config::MeowConfig::load(&root) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("meow sync: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = meow_config::generate_shadow_tsconfig(&cfg, &root) {
        eprintln!("meow sync: {err}");
        return ExitCode::FAILURE;
    }
    if let Err(err) = meow_config::write_root_tsconfig_shim(&root) {
        eprintln!("meow sync: {err}");
        return ExitCode::FAILURE;
    }
    // === RT-004 ===
    // Drop the curated strict-web ambient decl into `.meow/` so editors + `meow check`
    // resolve the §8.1 globals (fetch/URL/crypto.subtle/…) with nothing installed. The
    // runtime owns the content (I-9, curated-from-upstream); config owns the shadow dir.
    // === RT-005 ===
    // `meow sync` also refreshes the shipped `meow:*` declarations into `.meow/types/`
    // so editors resolve `meow:http` without any registry package or install step.
    let shadow_types = shadow_type_files();
    let shadow_refs = shadow_types
        .iter()
        .map(|(path, content)| (path.as_str(), *content))
        .collect::<Vec<_>>();
    if let Err(err) = meow_config::write_shadow_types(&root, &shadow_refs) {
        eprintln!("meow sync: {err}");
        return ExitCode::FAILURE;
    }
    // === /RT-005 ===
    // === /RT-004 ===
    // === CFG-003 ===
    // `package.json` is user-owned after CANON Amendment 001; sync refreshes only
    // the tsconfig/type shadows and never rewrites package.json.
    // === /CFG-003 ===
    println!(
        "meow sync: regenerated .meow/tsconfig.json + .meow/strict-web.d.ts + .meow/types/meow/*.d.ts + tsconfig.json shim"
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
            eprintln!("meow why-dep: cannot resolve the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let lockfile = match load_lockfile(&root) {
        Ok(lf) => lf,
        Err(err) => {
            eprintln!("meow why-dep: {err}");
            return ExitCode::FAILURE;
        }
    };
    // === CFG-003 ===
    let package_json = match meow_config::PackageJson::read(&root) {
        Ok(package_json) => package_json,
        Err(err) => {
            eprintln!("meow why-dep: {err}");
            return ExitCode::FAILURE;
        }
    };
    let direct = match package_json.direct_dependencies() {
        Ok(direct) => direct,
        Err(err) => {
            eprintln!("meow why-dep: {err}");
            return ExitCode::FAILURE;
        }
    };
    let roots = match meow_pkg::resolve_roots(&direct, &lockfile) {
        Ok(roots) => roots,
        Err(err) => {
            eprintln!("meow why-dep: {err}");
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
                eprintln!("meow why-dep: {err}");
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
        eprintln!(
            "meow: `{}` is not in the dependency tree (no path from any direct dependency in package.json)",
            args.pkg
        );
        return ExitCode::FAILURE;
    }
    render_why_dep(&report);
    ExitCode::SUCCESS
}

/// Render a found `why-dep` report as prose chains (stdout).
fn render_why_dep(report: &meow_obs::WhyDep) {
    println!(
        "{} is in the dependency tree — {} version(s).",
        report.target,
        report.versions.len()
    );
    println!("(each chain starts at a project direct dependency)");
    for tv in &report.versions {
        println!();
        let integrity = match &tv.integrity {
            Some(hash) => hash.to_sri(),
            None => "none — referenced but not in lockfile".to_string(),
        };
        let direct = if tv.direct {
            "  (direct dependency)"
        } else {
            ""
        };
        println!(
            "{}@{}  integrity {integrity}{direct}",
            tv.node.name, tv.node.version
        );
        for path in &tv.paths {
            let chain: Vec<String> = path
                .nodes
                .iter()
                .map(|n| format!("{}@{}", n.name, n.version))
                .collect();
            println!("  {}", chain.join(" → "));
        }
        if tv.truncated {
            println!(
                "  … showing first {} of more chains (raise with --limit)",
                tv.paths.len()
            );
        }
    }
}
// === /OBS-001 ===

// === PKG-002 ===
const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org";

/// Production npm registry client. Lives at the CLI edge so `meow-pkg` stays
/// network-free; all registry I/O is explicit here.
struct NpmRegistry {
    base: String,
}

impl NpmRegistry {
    fn npm() -> NpmRegistry {
        NpmRegistry {
            base: NPM_REGISTRY_URL.to_owned(),
        }
    }

    fn base_url(&self) -> &str {
        &self.base
    }

    fn metadata_url(&self, name: &meow_pkg::PackageName) -> String {
        format!("{}/{}", self.base, encode_package_name(name))
    }
}

impl meow_pkg::RegistrySource for NpmRegistry {
    fn fetch_metadata(
        &self,
        name: &meow_pkg::PackageName,
    ) -> Result<meow_pkg::PackageMetadata, meow_pkg::RegistryError> {
        let url = self.metadata_url(name);
        let mut response = match ureq::get(&url).call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(status)) => {
                return Err(meow_pkg::RegistryError::Status { url, status });
            }
            Err(err) => {
                return Err(meow_pkg::RegistryError::Fetch {
                    target: url,
                    reason: err.to_string(),
                });
            }
        };
        response
            .body_mut()
            .read_json::<meow_pkg::PackageMetadata>()
            .map_err(|err| meow_pkg::RegistryError::Metadata {
                name: name.to_string(),
                reason: err.to_string(),
            })
    }

    fn fetch_tarball(&self, url: &str) -> Result<Vec<u8>, meow_pkg::RegistryError> {
        let mut response = match ureq::get(url).call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(status)) => {
                return Err(meow_pkg::RegistryError::Status {
                    url: url.to_owned(),
                    status,
                });
            }
            Err(err) => {
                return Err(meow_pkg::RegistryError::Fetch {
                    target: url.to_owned(),
                    reason: err.to_string(),
                });
            }
        };
        response
            .body_mut()
            .read_to_vec()
            .map_err(|err| meow_pkg::RegistryError::Fetch {
                target: url.to_owned(),
                reason: err.to_string(),
            })
    }
}

/// `meow install`: resolve declared deps, populate the cache, and write the lockfile.
fn cmd_install(args: &InstallArgs) -> ExitCode {
    if matches!(args.mode, InstallMode::Vfs) {
        eprintln!(
            "meow install: `--mode {}` lands in PKG-004 (P2)",
            install_mode_name(&args.mode)
        );
        return ExitCode::from(EXIT_UNIMPLEMENTED);
    }

    let projection = match install_projection(args) {
        Ok(projection) => projection,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };

    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("meow install: cannot resolve the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };

    let registry = NpmRegistry::npm();
    // === CFG-003 ===
    for package in &args.packages {
        let (name, req) = match requested_dependency(&registry, package) {
            Ok(dep) => dep,
            Err(err) => {
                eprintln!("meow install: {err}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(err) = meow_config::add_dependency(&root, name, req) {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    }
    let package_json = match load_install_package_json(&root) {
        Ok(package_json) => package_json,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };
    let direct_deps = match package_json.direct_dependencies() {
        Ok(direct_deps) => direct_deps,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };
    // === /CFG-003 ===

    let cache = meow_pkg::Cache::in_home(crate::host::host_home());
    let meow_req = match runtime_meow_requirement() {
        Ok(req) => req,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };
    let direct = direct_deps
        .iter()
        .map(|(name, req)| (name.clone(), meow_pkg::DepSpec::Range(req.clone())))
        .collect::<BTreeMap<_, _>>();
    let lockfile = match meow_pkg::Installer::new(&registry, &cache, registry.base_url(), meow_req)
        .resolve(&direct)
    {
        Ok(lockfile) => lockfile,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };

    let installed = lockfile.len();
    let lock_path = root.join("meow.lock.jsonl");
    if let Err(err) = lockfile.write_canonical(&lock_path) {
        eprintln!("meow install: {err}");
        return ExitCode::FAILURE;
    }

    // === PKG-004 ===
    if let Some(opts) = projection {
        let roots = match meow_pkg::resolve_roots(&direct_deps, &lockfile) {
            Ok(roots) => roots,
            Err(err) => {
                eprintln!("meow install: {err}");
                return ExitCode::FAILURE;
            }
        };
        let graph = match meow_pkg::ResolutionGraph::assemble(std::sync::Arc::new(lockfile), roots)
        {
            Ok(graph) => graph,
            Err(err) => {
                eprintln!("meow install: {err}");
                return ExitCode::FAILURE;
            }
        };
        let report = match meow_pkg::Materializer::new(&cache, &graph, &root).materialize(&opts) {
            Ok(report) => report,
            Err(err) => {
                eprintln!("meow install: {err}");
                return ExitCode::FAILURE;
            }
        };
        println!(
            "installed {} packages → {}",
            installed,
            lock_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("meow.lock.jsonl")
        );
        println!(
            "materialized {} packages / {} edges / {} bytes → {}{}",
            report.packages,
            report.edges,
            report.bytes_written,
            report.root.display(),
            if report.skipped { " (skipped)" } else { "" }
        );
        return ExitCode::SUCCESS;
    }
    // === /PKG-004 ===

    println!(
        "installed {} packages → {} (no node_modules)",
        installed,
        lock_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("meow.lock.jsonl")
    );
    ExitCode::SUCCESS
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

fn requested_dependency(
    registry: &NpmRegistry,
    raw: &str,
) -> Result<(meow_pkg::PackageName, meow_pkg::VersionReq), String> {
    let (name, maybe_req) = split_package_arg(raw)?;
    let requirement = match maybe_req {
        Some(req) => resolve_requested_requirement(registry, &name, req)?,
        None => dist_tag_requirement(registry, &name, "latest")?,
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

fn resolve_requested_requirement(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    raw_req: &str,
) -> Result<meow_pkg::VersionReq, String> {
    match meow_pkg::VersionReq::parse(raw_req) {
        Ok(req) => Ok(req),
        Err(_) => dist_tag_requirement(registry, name, raw_req),
    }
}

fn dist_tag_requirement(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    tag: &str,
) -> Result<meow_pkg::VersionReq, String> {
    let metadata = meow_pkg::RegistrySource::fetch_metadata(registry, name)
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

fn encode_package_name(name: &meow_pkg::PackageName) -> String {
    name.as_str().replace('/', "%2f")
}

// === PKG-004 ===
fn install_projection(args: &InstallArgs) -> Result<Option<meow_pkg::MaterializeOptions>, String> {
    let selection = if args.materialize {
        Some(InstallMode::Materialize)
    } else if args.vendor {
        Some(InstallMode::Vendor)
    } else {
        match args.mode {
            InstallMode::Pnp => None,
            InstallMode::Vfs => None,
            InstallMode::Materialize => Some(InstallMode::Materialize),
            InstallMode::Vendor => Some(InstallMode::Vendor),
        }
    };

    if selection.is_none() {
        if args.copy {
            return Err(
                "`--copy` requires `--materialize`, `--vendor`, or `--mode materialize|vendor`"
                    .to_owned(),
            );
        }
        if args.clean {
            return Err(
                "`--clean` requires `--materialize`, `--vendor`, or `--mode materialize|vendor`"
                    .to_owned(),
            );
        }
        if args.vendor_dir != Path::new("vendor") {
            return Err("`--vendor-dir` requires `--vendor` or `--mode vendor`".to_owned());
        }
        return Ok(None);
    }

    if matches!(selection, Some(InstallMode::Materialize)) && args.vendor_dir != Path::new("vendor")
    {
        return Err("`--vendor-dir` requires `--vendor` or `--mode vendor`".to_owned());
    }

    match selection {
        Some(InstallMode::Materialize) => {
            let mut opts = meow_pkg::MaterializeOptions::node_modules();
            if args.copy {
                opts.link = meow_pkg::LinkStrategy::Copy;
            }
            opts.clean = args.clean;
            Ok(Some(opts))
        }
        Some(InstallMode::Vendor) => {
            let mut opts = meow_pkg::MaterializeOptions::vendor();
            opts.clean = args.clean;
            opts.vendor_dir = args.vendor_dir.clone();
            Ok(Some(opts))
        }
        _ => Ok(None),
    }
}
// === /PKG-004 ===

fn install_mode_name(mode: &InstallMode) -> &'static str {
    match mode {
        InstallMode::Pnp => "pnp",
        InstallMode::Vfs => "vfs",
        InstallMode::Materialize => "materialize",
        InstallMode::Vendor => "vendor",
    }
}
// === /PKG-002 ===

// === RT-006 ===
#[derive(Debug, Clone, Copy)]
struct RunFlagView<'a> {
    argv: &'a [String],
    allow_clock: bool,
    allow_random: bool,
    allow_env: &'a Option<String>,
}

fn run_flags(args: &RunArgs) -> RunFlagView<'_> {
    RunFlagView {
        argv: &args.argv,
        allow_clock: args.allow_clock,
        allow_random: args.allow_random,
        allow_env: &args.allow_env,
    }
}

// === RUN-001 ===
fn run_script_flags(args: &RunScriptArgs) -> RunFlagView<'_> {
    RunFlagView {
        argv: &args.argv,
        allow_clock: args.allow_clock,
        allow_random: args.allow_random,
        allow_env: &args.allow_env,
    }
}
// === /RUN-001 ===

/// Build the hermetic (clock/rng/env) config from the `meow run` grant flags. No
/// flags = fully deterministic (I-6); each `--allow-*` flips one source (A6).
fn hermetic_config(args: &RunFlagView<'_>) -> meow_runtime::hermetic::HermeticConfig {
    let mut cfg = meow_runtime::hermetic::HermeticConfig::default();
    if args.allow_clock {
        cfg = cfg.with_real_clock();
    }
    if args.allow_random {
        cfg = cfg.with_os_rng();
    }
    if let Some(names) = args.allow_env {
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
    if matches!(mode, meow_runtime::node::NodeMode::Enabled) && args.allow_env.is_none() {
        cfg = cfg.with_env_all();
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
    argv1: String,
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
fn cmd_run(args: &RunArgs) -> ExitCode {
    cmd_run_inner("run", &args.target, run_flags(args))
}

// === RUN-001 ===
fn cmd_dev(args: &RunScriptArgs) -> ExitCode {
    cmd_run_inner("dev", "dev", run_script_flags(args))
}

fn cmd_run_inner(verb: &'static str, target: &str, flags: RunFlagView<'_>) -> ExitCode {
    match cmd_run_result(target, flags) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("meow {verb}: {err}");
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
        run_native_request(&request, flags, BTreeMap::new()).await
    })
}

async fn execute_package_script(
    package_json: &meow_config::PackageJson,
    project_dir: &Path,
    init_cwd: &Path,
    script_name: &str,
    flags: RunFlagView<'_>,
) -> Result<ExitCode, RunCommandError> {
    let ctx = build_runtime_context(project_dir)?;
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
    let env = lifecycle_env(event, script, &ctx.project_dir, init_cwd);
    match plan_script(ctx, script, cli_argv)? {
        PlannedScript::Native(request) => run_native_request(&request, flags, env).await,
        PlannedScript::Shell { script, argv } => {
            run_shell_command(&ctx.project_dir, &script, &argv, env)
        }
    }
}

async fn run_native_request(
    request: &NativeRunRequest,
    flags: RunFlagView<'_>,
    env: BTreeMap<String, String>,
) -> Result<ExitCode, RunCommandError> {
    let ctx = build_runtime_context(&request.project_dir)?;
    let loader: std::rc::Rc<dyn meow_runtime::deno_core::ModuleLoader> =
        std::rc::Rc::new(meow_loader::MeowModuleLoader::new(
            meow_loader::Resolver::from_resolution(
                &ctx.graph,
                ctx.cache.clone(),
                ctx.project_root.clone(),
                // === RT-005 ===
                meow_runtime::native::native_module_registry(),
                // === /RT-005 ===
            ),
            std::rc::Rc::new(std::cell::RefCell::new(meow_graph::GraphDb::new())),
        ));

    // === RT-004 ===
    let caps: meow_runtime::web::NetCaps = std::sync::Arc::new(meow_runtime::AllowAll);
    let mut extensions = meow_runtime::web::extensions(meow_runtime::web::WebOptions {
        caps,
        user_agent: format!("meow/{}", env!("CARGO_PKG_VERSION")),
    });
    // === RT-005 ===
    extensions.push(meow_runtime::http_extension());
    // === /RT-005 ===
    // === /RT-004 ===

    // === RT-006 ===
    let hermetic = run_hermetic_config(&flags, ctx.node_mode);
    meow_runtime::hermetic::pin_deterministic_intl(&hermetic);
    extensions.extend(meow_runtime::hermetic::extensions(hermetic));
    // === RT-007 ===
    let mut node_argv = Vec::with_capacity(request.argv.len() + 2);
    node_argv.push("meow".to_owned());
    node_argv.push(request.argv1.clone());
    node_argv.extend(request.argv.iter().cloned());
    extensions.extend(meow_runtime::node::extensions(
        meow_runtime::node::NodeOptions {
            mode: ctx.node_mode,
            argv: node_argv,
            cwd: request.process_cwd.clone(),
            env,
        },
    ));
    // === /RT-007 ===
    // === /RT-006 ===

    let mut runtime = meow_runtime::Runtime::new(meow_runtime::RuntimeOptions {
        module_loader: loader,
        extensions,
    })
    .map_err(|err| RunCommandError::Message(err.to_string()))?;

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
    native_file_request(
        find_project_root(abs.parent().unwrap_or(&abs)),
        cwd.to_path_buf(),
        abs,
        argv,
    )
}

fn native_file_request(
    project_dir: PathBuf,
    process_cwd: PathBuf,
    abs: PathBuf,
    argv: &[String],
) -> Result<NativeRunRequest, RunCommandError> {
    let spec = meow_runtime::ModuleSpecifier::from_file_path(&abs)
        .map_err(|()| RunCommandError::InvalidEntryPath(abs.display().to_string()))?;
    Ok(NativeRunRequest {
        project_dir,
        process_cwd,
        spec,
        argv1: abs.to_string_lossy().into_owned(),
        argv: argv.to_vec(),
    })
}

fn build_runtime_context(project_dir: &Path) -> Result<RuntimeContext, RunCommandError> {
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
    graph
        .verify_cached(&cache)
        .map_err(|err| RunCommandError::Message(err.to_string()))?;
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
        let bin_path = root.join(&member);
        if !bin_path.is_file() {
            return Err(RunCommandError::MissingBinFile {
                package: name.to_string(),
                command: command.to_owned(),
                target: member,
            });
        }
        matches.push((
            name.to_string(),
            meow_loader::encode_cache_url(&entry.integrity, &member),
            bin_path,
        ));
    }

    match matches.len() {
        0 => Ok(None),
        1 => {
            let (_package, spec, bin_path) = matches.pop().expect("one match");
            Ok(Some(NativeRunRequest {
                project_dir: ctx.project_dir.clone(),
                process_cwd: ctx.project_dir.clone(),
                spec,
                argv1: bin_path.to_string_lossy().into_owned(),
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
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
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
    env
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
        if dir.join("meow.lock.jsonl").is_file() || dir.join("package.json").is_file() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return start.to_path_buf(),
        }
    }
}

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
    fn install_materialize_flag_parses_with_default_mode() {
        let cli = Cli::try_parse_from(["meow", "install", "--materialize"]).expect("parse cli");
        let Command::Install(args) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.materialize);
        assert!(matches!(args.mode, InstallMode::Pnp));
        assert!(!args.vendor);
    }

    #[test]
    fn dev_shorthand_parses_trailing_args() {
        let cli = Cli::try_parse_from(["meow", "dev", "--", "watch"]).expect("parse cli");
        let Command::Dev(args) = cli.command else {
            panic!("expected dev command");
        };
        assert_eq!(args.argv, vec!["watch".to_string()]);
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
    fn find_project_root_falls_back_to_start_without_project_markers() {
        // No lockfile/package.json anywhere on the way up: a local-only run uses the entry dir.
        let dir = unit_tmp("nolock");
        let found = find_project_root(&dir);
        assert_eq!(found, dir);
        std::fs::remove_dir_all(&dir).ok();
    }
}
