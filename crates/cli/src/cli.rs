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
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Entry module to execute.
    pub entry: PathBuf,
    /// Arguments after `--`, to forward to the program (forwarding is not yet wired).
    #[arg(last = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Virtual-store install mode (CANON §18; PKG owns the final flag surface).
    #[arg(long, value_enum, default_value_t = InstallMode::Pnp)]
    pub mode: InstallMode,
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
        }
    }
}

impl Cli {
    pub fn run(self) -> ExitCode {
        match self.command {
            // === CFG-001 ===
            Command::Sync => cmd_sync(),
            // === /CFG-001 ===
            // === RT-001 ===
            Command::Run(args) => cmd_run(&args.entry, &args.argv),
            // === /RT-001 ===
            // HONEST stub (CRAFT — "the action must DO the work"): no fake success path.
            // Structured, single-line, machine-greppable; stderr only; non-zero exit.
            other => {
                let (verb, phase) = other.landing();
                eprintln!("meow: not yet implemented — `{verb}` lands in PLAN {phase}");
                ExitCode::from(EXIT_UNIMPLEMENTED)
            }
        }
    }
}

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
    if let Err(err) = meow_config::write_shadow_types(
        &root,
        &[(
            meow_config::STRICT_WEB_DTS_FILE,
            meow_runtime::web::STRICT_WEB_DTS,
        )],
    ) {
        eprintln!("meow sync: {err}");
        return ExitCode::FAILURE;
    }
    // === /RT-004 ===
    println!(
        "meow sync: regenerated .meow/tsconfig.json + .meow/strict-web.d.ts + tsconfig.json shim"
    );
    ExitCode::SUCCESS
}
// === /CFG-001 ===

// === RT-001 ===
/// `meow run <file>`: canonicalize the named entry -> `file:` URL -> drive one
/// ESM module to completion through V8. The binary edge owns host access (cwd via
/// canonicalize) and error rendering; `meow-runtime` stays free of ambient reads
/// (I-6). Plain JS/ESM (`.js`/`.mjs`) executes; a `.ts` entry fails honestly
/// (TypeScript needs the type-strip, RT-003) via a `RuntimeError::Module`.
fn cmd_run(entry: &std::path::Path, argv: &[String]) -> ExitCode {
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
        // Build THE resolver + content-addressed cache loader (replaces RT-001's
        // TrivialModuleLoader). The binary edge owns ambient reads (I-6): it resolves
        // the project root + host home and translates meow.lock.jsonl into the
        // resolver's name->hash stand-in map (full lockfile-driven resolution is
        // LOAD-003). The graph is shared (I-1); no node_modules is ever touched (I-5).
        let entry_dir = abs.parent().unwrap_or(&abs);
        // Walk UP to the nearest meow.lock.jsonl so `meow run src/main.ts` finds the
        // repo-root lockfile, not `src/meow.lock.jsonl` (host-pure path walk, I-6).
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
        let bare = match load_bare_map(&project_dir) {
            Ok(map) => map,
            Err(err) => {
                eprintln!("meow run: {err}");
                return ExitCode::FAILURE;
            }
        };
        let loader: std::rc::Rc<dyn meow_runtime::deno_core::ModuleLoader> =
            std::rc::Rc::new(meow_loader::MeowModuleLoader::new(
                meow_loader::Resolver::new(
                    std::sync::Arc::new(meow_pkg::Cache::in_home(crate::host::host_home())),
                    bare,
                    project_root,
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
        let extensions = meow_runtime::web::extensions(meow_runtime::web::WebOptions {
            caps,
            user_agent: format!("meow/{}", env!("CARGO_PKG_VERSION")),
        });
        // === /RT-004 ===

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

/// Honest failure translating a lockfile into the resolver's bare map. Either the
/// lockfile itself is malformed ([`Lock`]) or it pins two versions of one package
/// — which the name-keyed map cannot represent until LOAD-003 adds real version
/// selection, so it fails loudly rather than silently dropping a version.
///
/// [`Lock`]: BareMapError::Lock
#[derive(Debug, thiserror::Error)]
enum BareMapError {
    #[error(transparent)]
    Lock(#[from] meow_pkg::LockError),
    #[error(
        "lockfile has multiple versions of `{name}` ({versions}); \
         version selection lands in LOAD-003"
    )]
    AmbiguousVersion { name: String, versions: String },
}

/// Read `meow.lock.jsonl` (if present) from the project root and translate it into
/// the resolver's `name -> ContentHash` stand-in map. The edge does the lockfile
/// I/O; the resolver stays a pure `name -> hash` lookup (full lockfile-driven
/// resolution is LOAD-003). A missing lockfile is an empty map (local-only run); a
/// malformed lockfile is an honest error, never silently ignored (I-7). Two
/// entries sharing a package name is an honest [`BareMapError::AmbiguousVersion`]
/// (not a silent collapse to one).
fn load_bare_map(
    project_root: &std::path::Path,
) -> Result<std::collections::HashMap<String, meow_pkg::ContentHash>, BareMapError> {
    let lock_path = project_root.join("meow.lock.jsonl");
    if !lock_path.exists() {
        return Ok(std::collections::HashMap::new());
    }
    let lockfile = meow_pkg::Lockfile::read(&lock_path)?;
    let mut map: std::collections::HashMap<String, meow_pkg::ContentHash> =
        std::collections::HashMap::with_capacity(lockfile.len());
    for entry in lockfile.iter() {
        let name = entry.name.to_string();
        if map.insert(name.clone(), entry.integrity.clone()).is_some() {
            // Two lines share a name: collapsing them would silently drop a
            // version. Gather every pinned version of this name for the diagnostic.
            let versions = lockfile
                .iter()
                .filter(|other| other.name == entry.name)
                .map(|other| other.version.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BareMapError::AmbiguousVersion { name, versions });
        }
    }
    Ok(map)
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
