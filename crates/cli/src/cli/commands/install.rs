//! `meow install` / `add` / `remove` — resolve declared deps, populate the
//! content-addressed cache, write the lockfile, and materialize `node_modules`.
//! Hosts the production npm registry client ([`NpmRegistry`]) shared with `x`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use crate::cli::{hiss, purr, ui, InstallArgs, InstallMode, PkgArgs};
use crate::host;

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
pub(super) struct NpmRegistry {
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
        let home = host::host_home();
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
    /// and `meow x` which always need the registry).
    pub(super) fn npm() -> Result<NpmRegistry, String> {
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

    pub(super) fn base_url(&self) -> &str {
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
        dev: false,
        compat_lockfile: false,
        packages: Vec::new(),
    }
}

pub fn cmd_add(args: &PkgArgs) -> ExitCode {
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
            let dependency_section = if args.dev {
                meow_config::DependencySection::DevDependencies
            } else {
                meow_config::DependencySection::Dependencies
            };
            let mut resolved = Vec::with_capacity(args.packages.len());
            for package in &args.packages {
                let (name, req) = requested_dependency(&registry, package)
                    .await
                    .map_err(|err| err.to_string())?;
                meow_config::add_dependency_to(&root, name.clone(), req.clone(), dependency_section)
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
    let meow_home = host::host_home().join(".meow");
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

pub fn cmd_remove(args: &PkgArgs) -> ExitCode {
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
            let meow_home = host::host_home().join(".meow");
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
pub fn cmd_install(args: &InstallArgs) -> ExitCode {
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
        let dependency_section = if args.dev {
            meow_config::DependencySection::DevDependencies
        } else {
            meow_config::DependencySection::Dependencies
        };
        for package in &args.packages {
            let (name, req) = requested_dependency(&registry, package)
                .await
                .map_err(|err| err.to_string())?;
            meow_config::add_dependency_to(&root, name, req, dependency_section)
                .map_err(|err| err.to_string())?;
        }
        let package_json = load_install_package_json(&root)?;
        let direct_deps = package_json
            .direct_dependencies()
            .map_err(|err| err.to_string())?;
        let overrides = package_json
            .package_overrides()
            .map_err(|err| err.to_string())?;
        // === /CFG-003 ===

        let cache = meow_pkg::Cache::in_home(host::host_home());
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
        if args.compat_lockfile {
            write_compat_package_lockfile(&root).map_err(|err| err.to_string())?;
        }
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
fn write_compat_package_lockfile(root: &Path) -> std::io::Result<()> {
    let path = root.join("package-lock.json");
    if path.exists() {
        return Ok(());
    }
    let package_name = meow_config::PackageJson::read(root)
        .ok()
        .and_then(|package_json| package_json.name)
        .unwrap_or_else(|| "meow-project".to_owned());
    let body = serde_json::json!({
        "name": package_name,
        "lockfileVersion": 3,
        "requires": true,
        "packages": {
            "": {
                "name": package_name
            }
        },
        "meowCompatibilityLockfile": true
    });
    let mut contents = serde_json::to_string_pretty(&body)?;
    contents.push('\n');
    std::fs::write(path, contents)
}

pub(super) fn load_install_package_json(root: &Path) -> Result<meow_config::PackageJson, String> {
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

pub(super) fn split_package_arg(
    raw: &str,
) -> Result<(meow_pkg::PackageName, Option<&str>), String> {
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

pub(super) async fn resolve_requested_requirement(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    raw_req: &str,
) -> Result<meow_pkg::VersionReq, String> {
    match meow_pkg::VersionReq::parse(raw_req) {
        Ok(req) => Ok(req),
        Err(_) => dist_tag_requirement(registry, name, raw_req).await,
    }
}

pub(super) async fn dist_tag_requirement(
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

pub(super) fn runtime_meow_requirement() -> Result<meow_pkg::VersionReq, String> {
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

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Command, InstallMode};
    use clap::Parser;

    #[test]
    fn install_default_mode_is_materialize() {
        let cli = Cli::try_parse_from(["meow", "install"]).expect("parse cli");
        let Some(Command::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(!args.materialize);
        assert!(matches!(args.mode, InstallMode::Materialize));
        assert!(!args.vendor);
        let opts = super::install_projection(&args).expect("projection selection");
        assert!(matches!(opts.projection, meow_pkg::Projection::NodeModules));
        assert!(matches!(opts.link, meow_pkg::LinkStrategy::Symlink));
    }
}
