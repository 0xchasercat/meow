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
    /// Execute a program (default mode = strict-web).
    Run(RunArgs),
    /// Watch-mode run.
    Dev(RunArgs),
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
    /// Regenerate shadow configs (.meow/tsconfig.json, root package.json) — ADR-8.
    Sync,
    /// Regenerate or verify the committed `meow:*` declarations (RT-005 / types-fresh).
    Types(TypesArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Entry module to execute.
    pub entry: PathBuf,
    /// Arguments after `--`, to forward to the program (forwarding is not yet wired).
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

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Virtual-store install mode (CANON §18; PKG owns the final flag surface).
    #[arg(long, value_enum, default_value_t = InstallMode::Pnp)]
    pub mode: InstallMode,
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
            // === RT-001 ===
            Command::Run(args) => {
                // === RT-006 ===
                let hermetic = hermetic_config(&args);
                // === /RT-006 ===
                cmd_run(&args.entry, &args.argv, hermetic)
            }
            // === /RT-001 ===
            // HONEST stub (CRAFT — "the action must DO the work"): no fake success path.
            // Structured, single-line, machine-greppable; stderr only; non-zero exit.
            // === CFG-002 ===
            Command::Doctor => cmd_doctor(),
            // === /CFG-002 ===
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
    // === CFG-002 ===
    // Generate the OWNED root package.json projection from meow.config (ADR-8) —
    // the surface npm/pnpm/IDEs/`npm publish` read. `meow` owns + overwrites it.
    if let Err(err) = meow_config::generate_root_package_json(&cfg, &root) {
        eprintln!("meow sync: {err}");
        return ExitCode::FAILURE;
    }
    // === /CFG-002 ===
    println!(
        "meow sync: regenerated .meow/tsconfig.json + .meow/strict-web.d.ts + .meow/types/meow/*.d.ts + tsconfig.json shim + package.json"
    );
    ExitCode::SUCCESS
}
// === /CFG-001 ===

// === CFG-002 ===
/// `meow doctor` — CFG-002 owns ONLY the root package.json staleness/hand-edit
/// check (the full doctor surface is a later spec). Read-only; never mutates.
fn cmd_doctor() -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("meow doctor: cannot resolve the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = match meow_config::MeowConfig::load(&root) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("meow doctor: {err}");
            return ExitCode::FAILURE;
        }
    };
    match meow_config::classify_root_package_json(&cfg, &root) {
        Ok(status) => {
            use meow_config::PackageJsonStatus::{Fresh, HandEdited, Missing, Stale};
            match status {
                Fresh => println!("package.json: in sync"),
                Missing => println!("package.json: missing — run `meow sync`"),
                Stale => eprintln!(
                    "warning: root package.json is out of sync with meow.config; meow owns it \
                     (ADR-8) and `meow sync` will regenerate it, overwriting any manual edits. \
                     Edit publishing metadata in meow.config.ts."
                ),
                HandEdited => eprintln!(
                    "warning: root package.json was hand-edited; meow owns it (ADR-8) and \
                     `meow sync` will overwrite it. Move publishing metadata into meow.config.ts."
                ),
            }
            // NOTE: package.json staleness is the ONLY check CFG-002's doctor performs;
            // the full environment/config/lockfile health surface is a later spec.
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("meow doctor: {err}");
            ExitCode::FAILURE
        }
    }
}
// === /CFG-002 ===

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
    let config = match meow_config::MeowConfig::load(&root) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("meow why-dep: {err}");
            return ExitCode::FAILURE;
        }
    };
    let roots = match meow_pkg::resolve_roots(&config.dependencies, &lockfile) {
        Ok(roots) => roots,
        Err(err) => {
            eprintln!("meow why-dep: {err}");
            return ExitCode::FAILURE;
        }
    };
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
            "meow: `{}` is not in the dependency tree (no path from any direct dependency in meow.lock.jsonl)",
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
    match args.mode {
        InstallMode::Pnp => {}
        InstallMode::Vfs | InstallMode::Materialize | InstallMode::Vendor => {
            eprintln!(
                "meow install: `--mode {}` lands in PKG-004 (P2)",
                install_mode_name(&args.mode)
            );
            return ExitCode::from(EXIT_UNIMPLEMENTED);
        }
    }

    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("meow install: cannot resolve the current directory: {err}");
            return ExitCode::FAILURE;
        }
    };

    let registry = NpmRegistry::npm();
    let mut cfg = match load_install_config(&root) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };

    for package in &args.packages {
        let (name, req) = match requested_dependency(&registry, package) {
            Ok(dep) => dep,
            Err(err) => {
                eprintln!("meow install: {err}");
                return ExitCode::FAILURE;
            }
        };
        cfg.dependencies.insert(name, req);
    }

    if !args.packages.is_empty() {
        if let Err(err) = meow_config::write_json_config(&cfg, &root) {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    }

    let cache = meow_pkg::Cache::in_home(crate::host::host_home());
    let meow_req = match runtime_meow_requirement() {
        Ok(req) => req,
        Err(err) => {
            eprintln!("meow install: {err}");
            return ExitCode::FAILURE;
        }
    };
    let direct = cfg
        .dependencies
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

    let lock_path = root.join("meow.lock.jsonl");
    if let Err(err) = lockfile.write_canonical(&lock_path) {
        eprintln!("meow install: {err}");
        return ExitCode::FAILURE;
    }
    if let Err(err) = meow_config::generate_root_package_json(&cfg, &root) {
        eprintln!("meow install: {err}");
        return ExitCode::FAILURE;
    }

    println!(
        "installed {} packages → {} (no node_modules)",
        lockfile.len(),
        lock_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("meow.lock.jsonl")
    );
    ExitCode::SUCCESS
}

fn load_install_config(root: &Path) -> Result<meow_config::MeowConfig, String> {
    match meow_config::MeowConfig::load(root) {
        Ok(cfg) => Ok(cfg),
        Err(meow_config::ConfigError::NotFound(_)) => Ok(meow_config::MeowConfig::default()),
        Err(err) => Err(err.to_string()),
    }
}

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
/// Build the hermetic (clock/rng/env) config from the `meow run` grant flags. No
/// flags = fully deterministic (I-6); each `--allow-*` flips one source (A6).
fn hermetic_config(args: &RunArgs) -> meow_runtime::hermetic::HermeticConfig {
    let mut cfg = meow_runtime::hermetic::HermeticConfig::default();
    if args.allow_clock {
        cfg = cfg.with_real_clock();
    }
    if args.allow_random {
        cfg = cfg.with_os_rng();
    }
    if let Some(names) = &args.allow_env {
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

// === RT-001 ===
/// `meow run <file>`: canonicalize the named entry -> `file:` URL -> drive one
/// ESM module to completion through V8. The binary edge owns host access (cwd via
/// canonicalize) and error rendering; `meow-runtime` stays free of ambient reads
/// (I-6). Plain JS/ESM (`.js`/`.mjs`) executes; a `.ts` entry fails honestly
/// (TypeScript needs the type-strip, RT-003) via a `RuntimeError::Module`.
fn cmd_run(
    entry: &std::path::Path,
    argv: &[String],
    hermetic: meow_runtime::hermetic::HermeticConfig,
) -> ExitCode {
    if !argv.is_empty() {
        eprintln!("meow run: forwarding program arguments (after `--`) is not yet supported");
        return ExitCode::FAILURE;
    }
    let async_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("meow run: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    async_rt.block_on(async {
        // Explicit: the program the user named (canonicalize against the real cwd).
        let abs = match std::fs::canonicalize(entry) {
            Ok(path) => path,
            Err(err) => {
                eprintln!("meow run: cannot find {}: {err}", entry.display());
                return ExitCode::FAILURE;
            }
        };
        let spec = match meow_runtime::ModuleSpecifier::from_file_path(&abs) {
            Ok(spec) => spec,
            Err(()) => {
                eprintln!("meow run: invalid entry path {}", abs.display());
                return ExitCode::FAILURE;
            }
        };

        // === LOAD-001 ===
        // Build THE resolver + content-addressed cache loader. The binary edge owns
        // ambient reads (I-6): it resolves the project root + host home and reads
        // meow.lock.jsonl. P1 does not yet have a root-entry mechanism, so the
        // direct-dependency map is empty by default; LOAD-003 still owns all real
        // package resolution once the caller supplies that map.
        let entry_dir = abs.parent().unwrap_or(&abs);
        let project_dir = find_project_root(entry_dir);
        let project_root = match meow_runtime::ModuleSpecifier::from_directory_path(&project_dir) {
            Ok(url) => url,
            Err(()) => {
                eprintln!(
                    "meow run: invalid project directory {}",
                    project_dir.display()
                );
                return ExitCode::FAILURE;
            }
        };
        let lockfile = match load_lockfile(&project_dir) {
            Ok(lockfile) => lockfile,
            Err(err) => {
                eprintln!("meow run: {err}");
                return ExitCode::FAILURE;
            }
        };
        // === PKG-002 ===
        let root_deps = match load_declared_root_deps(&project_dir, &lockfile) {
            Ok(deps) => deps,
            Err(err) => {
                eprintln!("meow run: {err}");
                return ExitCode::FAILURE;
            }
        };
        // === /PKG-002 ===
        // === PKG-003 ===
        let cache = std::sync::Arc::new(meow_pkg::Cache::in_home(crate::host::host_home()));
        let graph =
            match meow_pkg::ResolutionGraph::assemble(std::sync::Arc::new(lockfile), root_deps) {
                Ok(graph) => std::sync::Arc::new(graph),
                Err(err) => {
                    eprintln!("meow run: {err}");
                    return ExitCode::FAILURE;
                }
            };
        if let Err(err) = graph.verify_cached(&cache) {
            eprintln!("meow run: {err}");
            return ExitCode::FAILURE;
        }
        let loader: std::rc::Rc<dyn meow_runtime::deno_core::ModuleLoader> =
            std::rc::Rc::new(meow_loader::MeowModuleLoader::new(
                meow_loader::Resolver::from_resolution(
                    &graph,
                    cache,
                    project_root,
                    // === RT-005 ===
                    meow_runtime::native::native_module_registry(),
                    // === /RT-005 ===
                ),
                std::rc::Rc::new(std::cell::RefCell::new(meow_graph::GraphDb::new())),
            ));
        // === /PKG-003 ===
        // === /LOAD-001 ===

        // === RT-004 ===
        // Install the strict-web Stateless-Edge globals (CANON §8.1) on the
        // default runtime so `meow run` sees fetch/URL/crypto.subtle/etc. The
        // `fetch` network gate consults this capability seam; at P1 the default
        // is `AllowAll` (seam, not enforcement — SEC-001/P6). The seam value is
        // `Send + Sync` because deno_fetch resolves hosts on a spawned task.
        let caps: meow_runtime::web::NetCaps = std::sync::Arc::new(meow_runtime::AllowAll);
        let mut extensions = meow_runtime::web::extensions(meow_runtime::web::WebOptions {
            caps,
            user_agent: format!("meow/{}", env!("CARGO_PKG_VERSION")),
        });
        // === RT-005 ===
        // The `meow:http` native module resolves through the shared loader; append
        // its op layer here so `serve()` can bind, accept, respond, and shut down.
        extensions.push(meow_runtime::http_extension());
        // === /RT-005 ===
        // === /RT-004 ===

        // === RT-006 ===
        // Determinism shadows AFTER the Web globals so they rebind the real
        // Date/crypto/performance. Deterministic by default; the --allow-* flags
        // built `hermetic` (A6). Append, never replace.
        // Pin TZ=UTC + a fixed default locale under the virtual clock BEFORE the
        // isolate is created, so V8 renders Date/Intl deterministically across host
        // timezones + locales (craft-7 M3).
        meow_runtime::hermetic::pin_deterministic_intl(&hermetic);
        extensions.extend(meow_runtime::hermetic::extensions(hermetic));
        // === /RT-006 ===

        let mut runtime = match meow_runtime::Runtime::new(meow_runtime::RuntimeOptions {
            module_loader: loader,
            extensions,
        }) {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("meow run: {err}");
                return ExitCode::FAILURE;
            }
        };

        // A typed error renders as a diagnostic; never a panic / backtrace.
        match runtime.run_main_module(&spec).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("meow run: {err}");
                ExitCode::FAILURE
            }
        }
    })
}
// === /RT-001 ===

// === LOAD-001 ===

/// Walk UP from `start` to the nearest directory holding a `meow.lock.jsonl` (the
/// project-root marker) and return it; fall back to `start` when none exists (a
/// local-only run). Host-pure: it inspects only the given path, reads no env
/// (`$HOME` stays in `host/`, I-6).
fn find_project_root(start: &std::path::Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join("meow.lock.jsonl").is_file() {
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

// === PKG-002 ===
/// Derive runtime roots from the declared config when available. A missing config
/// keeps the P1 fallback (unique lockfile names only) so local-only runs still work;
/// a TS-only config with a non-empty lockfile is an honest error until config TS
/// evaluation lands.
fn load_declared_root_deps(
    project_root: &Path,
    lockfile: &meow_pkg::Lockfile,
) -> Result<BTreeMap<meow_pkg::PackageName, meow_pkg::Version>, String> {
    match meow_config::MeowConfig::load(project_root) {
        Ok(cfg) => meow_pkg::resolve_roots(&cfg.dependencies, lockfile).map_err(|err| err.to_string()),
        Err(meow_config::ConfigError::NotFound(_)) => fallback_root_deps_from_lockfile(lockfile),
        Err(meow_config::ConfigError::TsNotSupported) if lockfile.is_empty() => Ok(BTreeMap::new()),
        Err(meow_config::ConfigError::TsNotSupported) => Err(
            "meow.config.ts exists but cannot be evaluated yet, so root dependencies cannot be derived from a non-empty meow.lock.jsonl; provide meow.config.json for now".to_owned(),
        ),
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
                "lockfile pins multiple versions of `{}` ({prev}, {}) — root resolution is ambiguous without meow.config dependencies; run `meow install` or add meow.config.json",
                entry.name, entry.version
            ));
        }
    }
    Ok(deps)
}
// === /PKG-002 ===
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
    fn find_project_root_walks_up_to_the_nearest_lockfile() {
        // Lockfile at the ROOT, entry nested two levels down: discovery must climb
        // to the root, not stop at the entry's own directory (finding 1).
        let root = unit_tmp("root");
        std::fs::write(root.join("meow.lock.jsonl"), "").expect("write lockfile");
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
    fn find_project_root_falls_back_to_start_without_a_lockfile() {
        // No lockfile anywhere on the way up: a local-only run uses the entry dir.
        let dir = unit_tmp("nolock");
        let found = find_project_root(&dir);
        assert_eq!(found, dir);
        std::fs::remove_dir_all(&dir).ok();
    }
}
