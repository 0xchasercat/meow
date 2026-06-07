//! The `meow` command tree (DIST-001).
//!
//! Every §19 verb exists from day one as an *honest stub*: it prints a structured,
//! machine-greppable "not yet implemented" line to stderr and exits
//! [`EXIT_UNIMPLEMENTED`]. There is no fake success path (CRAFT — "the action must
//! DO the work"). `--version`/`--help` are real (clap). Later specs flip a stub to
//! real by replacing its dispatch arm; the exhaustive `match` in [`Command::landing`]
//! makes it a compile error to add a verb without wiring it.

use std::path::PathBuf;
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
        let root_deps = match root_deps_from_lockfile(&lockfile) {
            Ok(deps) => deps,
            Err(err) => {
                eprintln!("meow run: {err}");
                return ExitCode::FAILURE;
            }
        };
        let loader: std::rc::Rc<dyn meow_runtime::deno_core::ModuleLoader> =
            std::rc::Rc::new(meow_loader::MeowModuleLoader::new(
                meow_loader::Resolver::new(
                    std::sync::Arc::new(meow_pkg::Cache::in_home(crate::host::host_home())),
                    std::sync::Arc::new(lockfile),
                    root_deps,
                    project_root,
                    // === RT-005 ===
                    meow_runtime::native::native_module_registry(),
                    // === /RT-005 ===
                ),
                std::rc::Rc::new(std::cell::RefCell::new(meow_graph::GraphDb::new())),
            ));
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

/// Derive the project's direct-dependency map from the lockfile as a P1 stand-in:
/// real direct-vs-transitive tracking arrives with `meow install` (PKG-002/P2).
/// Each name maps to its single pinned version; a name pinned at multiple versions
/// is an ambiguous root (which is the direct dependency?) and fails honestly rather
/// than silently picking one (LOAD-003 supports multi-version transitively, but a
/// root must be unambiguous).
fn root_deps_from_lockfile(
    lockfile: &meow_pkg::Lockfile,
) -> Result<std::collections::BTreeMap<meow_pkg::PackageName, meow_pkg::Version>, String> {
    let mut deps = std::collections::BTreeMap::new();
    for entry in lockfile.iter() {
        if let Some(prev) = deps.insert(entry.name.clone(), entry.version.clone()) {
            return Err(format!(
                "lockfile pins multiple versions of `{}` ({prev}, {}) — which is the \
                 direct dependency is ambiguous; `meow install` will pin it (P2)",
                entry.name, entry.version
            ));
        }
    }
    Ok(deps)
}
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
