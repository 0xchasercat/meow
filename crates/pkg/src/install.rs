//! Deterministic dependency resolution + cache population (PKG-002).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use base64::Engine as _;
use sha2::{Digest, Sha512};

use crate::registry::{sha512_sri, DepSpec, PackageMetadata, RegistryError, RegistrySource};
use crate::{
    Cache, CacheError, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
};

#[derive(Copy, Clone)]
struct HostPlatform {
    os: &'static str,
    cpu: &'static str,
}

impl HostPlatform {
    fn current() -> HostPlatform {
        HostPlatform {
            os: if cfg!(target_os = "macos") {
                "darwin"
            } else if cfg!(target_os = "ios") {
                "ios"
            } else if cfg!(target_os = "linux") {
                "linux"
            } else if cfg!(target_os = "windows") {
                "win32"
            } else if cfg!(target_os = "android") {
                "android"
            } else if cfg!(target_os = "freebsd") {
                "freebsd"
            } else if cfg!(target_os = "openbsd") {
                "openbsd"
            } else if cfg!(target_os = "netbsd") {
                "netbsd"
            } else if cfg!(target_os = "dragonfly") {
                "dragonflybsd"
            } else if cfg!(target_os = "solaris") {
                "sunos"
            } else {
                "unknown"
            },
            cpu: if cfg!(target_arch = "x86_64") {
                "x64"
            } else if cfg!(target_arch = "aarch64") {
                "arm64"
            } else if cfg!(target_arch = "x86") {
                "ia32"
            } else if cfg!(target_arch = "arm") {
                "arm"
            } else if cfg!(target_arch = "riscv64") {
                "riscv64"
            } else {
                "unknown"
            },
        }
    }
}

fn is_compatible_optional_dependency(
    package: &PackageName,
    version_meta: &crate::VersionMetadata,
    host: HostPlatform,
) -> bool {
    if !version_meta.os.is_empty() || !version_meta.cpu.is_empty() {
        return constraints_match(&version_meta.os, host.os)
            && constraints_match(&version_meta.cpu, host.cpu);
    }

    package_name_is_next_swc_for_host(package, host)
}

fn package_name_is_next_swc_for_host(name: &PackageName, host: HostPlatform) -> bool {
    let Some(rest) = name.as_str().strip_prefix("@next/swc-") else {
        return true;
    };

    let mut parts = rest.split('-');
    let package_os = match parts.next() {
        Some(package_os) => package_os,
        None => return true,
    };
    match package_os {
        "darwin" | "linux" | "win32" | "freebsd" | "android" => {}
        _ => return true,
    }
    if !package_os.eq_ignore_ascii_case(host.os) {
        return false;
    }
    let package_cpu = match parts.next() {
        Some(package_cpu) => package_cpu,
        None => return false,
    };

    package_cpu.eq_ignore_ascii_case(host.cpu)
}

fn constraints_match(values: &[String], target: &str) -> bool {
    if values.is_empty() {
        return true;
    }

    let mut has_allowed = false;
    let mut is_allowed = false;

    for value in values {
        if let Some(unset) = value.strip_prefix('!') {
            if unset.eq_ignore_ascii_case(target) {
                return false;
            }
        } else {
            has_allowed = true;
            if value.eq_ignore_ascii_case(target) {
                is_allowed = true;
            }
        }
    }

    !has_allowed || is_allowed
}

const INSTALL_HTTP_CONCURRENCY: usize = 40;

#[derive(Debug, Clone)]
struct ResolvedNode {
    name: PackageName,
    version: Version,
    tarball: String,
    integrity: String,
    dependencies: BTreeMap<PackageName, Version>,
}

#[derive(Debug)]
struct DownloadedNode {
    node: ResolvedNode,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct QueueNode {
    name: PackageName,
    registry_name: PackageName,
    version: Version,
}

#[derive(Debug, Clone)]
struct PlannedNode {
    name: PackageName,
    version: Version,
    version_meta: crate::VersionMetadata,
}

/// Resolve a project's declared dependencies into a complete pinned graph.
pub struct Installer<'a> {
    source: Arc<dyn RegistrySource>,
    cache: &'a Cache,
    registry_url: String,
    meow_req: VersionReq,
    reuse_lockfile: Option<Lockfile>,
}

#[derive(Debug, Clone)]
pub enum InstallProgress {
    MetadataFetched {
        package: PackageName,
        fetched: usize,
    },
    PackageDownloaded {
        package: PackageName,
        downloaded: usize,
        total: usize,
    },
    PackageCached {
        package: PackageName,
        cached: usize,
        pending: usize,
    },
}

impl<'a> Installer<'a> {
    pub fn new<R>(
        source: R,
        cache: &'a Cache,
        registry_url: impl Into<String>,
        meow_req: VersionReq,
    ) -> Installer<'a>
    where
        R: RegistrySource + 'static,
    {
        Installer {
            source: Arc::new(source),
            cache,
            registry_url: registry_url.into(),
            meow_req,
            reuse_lockfile: None,
        }
    }

    pub fn with_reuse_lockfile(mut self, lockfile: Lockfile) -> Installer<'a> {
        self.reuse_lockfile = Some(lockfile);
        self
    }

    /// Resolve, verify, cache, and pin the complete dependency graph.
    pub fn resolve(
        &self,
        direct: &BTreeMap<PackageName, DepSpec>,
    ) -> Result<Lockfile, InstallError> {
        self.resolve_with_progress(direct, |_| {})
    }

    pub fn resolve_with_progress<F>(
        &self,
        direct: &BTreeMap<PackageName, DepSpec>,
        on_progress: F,
    ) -> Result<Lockfile, InstallError>
    where
        F: FnMut(InstallProgress),
    {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| {
                InstallError::Registry(RegistryError::Fetch {
                    target: "tokio runtime".to_owned(),
                    reason: err.to_string(),
                })
            })?;
        runtime.block_on(self.resolve_with_progress_async(direct, on_progress))
    }

    pub async fn resolve_async(
        &self,
        direct: &BTreeMap<PackageName, DepSpec>,
    ) -> Result<Lockfile, InstallError> {
        self.resolve_with_progress_async(direct, |_| {}).await
    }

    pub async fn resolve_with_progress_async<F>(
        &self,
        direct: &BTreeMap<PackageName, DepSpec>,
        mut on_progress: F,
    ) -> Result<Lockfile, InstallError>
    where
        F: FnMut(InstallProgress),
    {
        let mut metadata = BTreeMap::new();
        let mut queue = VecDeque::new();
        let mut selected: BTreeMap<PackageName, BTreeSet<Version>> = BTreeMap::new();
        let host = HostPlatform::current();

        let direct_names = direct
            .iter()
            .map(|(name, spec)| spec.registry_package(name).clone())
            .collect();
        self.prefetch_metadata_parallel(&mut metadata, direct_names, &mut on_progress)
            .await?;
        for (name, spec) in direct {
            let registry_name = spec.registry_package(name).clone();
            let meta = metadata.get(&registry_name).cloned().ok_or_else(|| {
                InstallError::Registry(RegistryError::Metadata {
                    name: registry_name.to_string(),
                    reason: "metadata missing after prefetch".to_owned(),
                })
            })?;
            let version = select_version(&registry_name, &meta, spec.selection_spec())?;
            selected
                .entry(name.clone())
                .or_default()
                .insert(version.clone());
            queue.push_back(QueueNode {
                name: name.clone(),
                registry_name,
                version,
            });
        }

        let mut done = BTreeSet::new();
        let mut jobs = Vec::new();

        while !queue.is_empty() {
            let batch: Vec<QueueNode> = queue.drain(..).collect();
            let batch_names = batch
                .iter()
                .map(|node| node.registry_name.clone())
                .collect();
            self.prefetch_metadata_parallel(&mut metadata, batch_names, &mut on_progress)
                .await?;

            let mut planned = Vec::new();
            let mut dep_names = BTreeSet::new();

            for node in batch {
                if done.contains(&(node.name.clone(), node.version.clone())) {
                    continue;
                }
                let meta = metadata.get(&node.registry_name).ok_or_else(|| {
                    InstallError::Registry(RegistryError::Metadata {
                        name: node.registry_name.to_string(),
                        reason: "metadata missing after prefetch".to_owned(),
                    })
                })?;
                let version_meta = meta.versions.get(&node.version).cloned().ok_or_else(|| {
                    InstallError::MissingVersion {
                        name: node.registry_name.to_string(),
                        version: node.version.to_string(),
                    }
                })?;
                dep_names.extend(
                    version_meta.dependencies.iter().map(|(dep, raw_req)| {
                        DepSpec::parse(raw_req).registry_package(dep).clone()
                    }),
                );
                dep_names.extend(version_meta.optional_dependencies.iter().filter_map(
                    |(dep, raw_req)| {
                        let dep_spec = DepSpec::parse(raw_req);
                        let dep_registry_name = dep_spec.registry_package(dep);
                        package_name_is_next_swc_for_host(dep_registry_name, host)
                            .then(|| dep_registry_name.clone())
                    },
                ));
                dep_names.extend(version_meta.peer_dependencies.iter().filter_map(
                    |(dep, raw_req)| {
                        if version_meta
                            .peer_dependencies_meta
                            .get(dep)
                            .map(|m| m.optional)
                            .unwrap_or(false)
                        {
                            return None;
                        }
                        let dep_spec = DepSpec::parse(raw_req);
                        Some(dep_spec.registry_package(dep).clone())
                    },
                ));
                planned.push(PlannedNode {
                    name: node.name,
                    version: node.version,
                    version_meta,
                });
            }

            self.prefetch_metadata_parallel(&mut metadata, dep_names, &mut on_progress)
                .await?;

            for node in planned {
                let mut dependencies = BTreeMap::new();
                for (dep, raw_req) in &node.version_meta.dependencies {
                    let dep_spec = DepSpec::parse(raw_req);
                    let dep_registry_name = dep_spec.registry_package(dep).clone();
                    let dep_meta = metadata.get(&dep_registry_name).ok_or_else(|| {
                        InstallError::Registry(RegistryError::Metadata {
                            name: dep_registry_name.to_string(),
                            reason: "metadata missing after prefetch".to_owned(),
                        })
                    })?;
                    let dep_version =
                        select_version(&dep_registry_name, dep_meta, dep_spec.selection_spec())?;
                    dependencies.insert(dep.clone(), dep_version.clone());
                    queue.push_back(QueueNode {
                        name: dep.clone(),
                        registry_name: dep_registry_name,
                        version: dep_version,
                    });
                }
                for (dep, raw_req) in &node.version_meta.optional_dependencies {
                    let dep_spec = DepSpec::parse(raw_req);
                    let dep_registry_name = dep_spec.registry_package(dep).clone();
                    if !package_name_is_next_swc_for_host(&dep_registry_name, host) {
                        continue;
                    }
                    let dep_meta = metadata.get(&dep_registry_name).ok_or_else(|| {
                        InstallError::Registry(RegistryError::Metadata {
                            name: dep_registry_name.to_string(),
                            reason: "metadata missing after prefetch".to_owned(),
                        })
                    })?;
                    let dep_version =
                        select_version(&dep_registry_name, dep_meta, dep_spec.selection_spec())?;
                    let dep_locked_meta = dep_meta.versions.get(&dep_version).ok_or_else(|| {
                        InstallError::MissingVersion {
                            name: dep_registry_name.to_string(),
                            version: dep_version.to_string(),
                        }
                    })?;
                    if !is_compatible_optional_dependency(&dep_registry_name, dep_locked_meta, host)
                    {
                        continue;
                    }
                    dependencies.insert(dep.clone(), dep_version.clone());
                    queue.push_back(QueueNode {
                        name: dep.clone(),
                        registry_name: dep_registry_name,
                        version: dep_version,
                    });
                }

                for (dep_name, dep_version) in &dependencies {
                    selected
                        .entry(dep_name.clone())
                        .or_default()
                        .insert(dep_version.clone());
                }
                for (peer, raw_req) in &node.version_meta.peer_dependencies {
                    if node
                        .version_meta
                        .peer_dependencies_meta
                        .get(peer)
                        .map(|m| m.optional)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    if dependencies.contains_key(peer) {
                        continue;
                    }
                    let dep_spec = DepSpec::parse(raw_req);
                    if let Some(existing) = selected
                        .get(peer)
                        .and_then(|set| max_selected_satisfying(set, &dep_spec))
                    {
                        dependencies.insert(peer.clone(), existing);
                        continue;
                    }
                    let dep_registry_name = dep_spec.registry_package(peer).clone();
                    let Some(dep_meta) = metadata.get(&dep_registry_name) else {
                        continue;
                    };
                    let Ok(dep_version) =
                        select_version(&dep_registry_name, dep_meta, dep_spec.selection_spec())
                    else {
                        continue;
                    };
                    dependencies.insert(peer.clone(), dep_version.clone());
                    selected
                        .entry(peer.clone())
                        .or_default()
                        .insert(dep_version.clone());
                    queue.push_back(QueueNode {
                        name: peer.clone(),
                        registry_name: dep_registry_name,
                        version: dep_version,
                    });
                }
                jobs.push(ResolvedNode {
                    name: node.name.clone(),
                    version: node.version.clone(),
                    tarball: node.version_meta.dist.tarball,
                    integrity: node.version_meta.dist.integrity,
                    dependencies,
                });
                done.insert((node.name, node.version));
            }
        }

        let total = jobs.len();
        let mut lockfile = Lockfile::new();
        let mut cached = 0usize;
        let mut downloads = Vec::new();

        for node in jobs {
            if let Some(entry) = self.reusable_lock_entry(&node)? {
                lockfile.upsert(entry);
                cached += 1;
                on_progress(InstallProgress::PackageCached {
                    package: node.name,
                    cached,
                    pending: total.saturating_sub(cached),
                });
            } else {
                downloads.push(node);
            }
        }

        let downloaded = self
            .fetch_tarballs_parallel(downloads, &mut on_progress)
            .await?;
        for downloaded in downloaded {
            let integrity = self.cache.store(&downloaded.bytes)?;
            let node = downloaded.node;
            lockfile.upsert(LockEntry {
                name: node.name.clone(),
                version: node.version.clone(),
                integrity,
                dependencies: node.dependencies,
                registry: RegistryProvenance::new(&self.registry_url),
                capabilities: vec![],
                wasm: vec![],
                meow: self.meow_req.clone(),
            });
            cached += 1;
            on_progress(InstallProgress::PackageCached {
                package: node.name,
                cached,
                pending: total.saturating_sub(cached),
            });
        }

        Ok(lockfile)
    }

    fn reusable_lock_entry(&self, node: &ResolvedNode) -> Result<Option<LockEntry>, InstallError> {
        let Some(entry) = self
            .reuse_lockfile
            .as_ref()
            .and_then(|lockfile| lockfile.get(&node.name, &node.version))
        else {
            return Ok(None);
        };
        if entry.dependencies != node.dependencies
            || entry.registry.registry != self.registry_url
            || entry.meow != self.meow_req
        {
            return Ok(None);
        }

        match self.cache.read(&entry.integrity) {
            Ok(_) => Ok(Some(entry.clone())),
            Err(CacheError::NotFound(_) | CacheError::IntegrityMismatch { .. }) => Ok(None),
            Err(err) => Err(InstallError::Cache(err)),
        }
    }

    async fn fetch_tarballs_parallel<F>(
        &self,
        jobs: Vec<ResolvedNode>,
        on_progress: &mut F,
    ) -> Result<Vec<DownloadedNode>, InstallError>
    where
        F: FnMut(InstallProgress),
    {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }

        let total = jobs.len();
        let mut pending = VecDeque::from(jobs);
        let mut set = tokio::task::JoinSet::new();
        let mut out = Vec::with_capacity(total);

        while !pending.is_empty() || !set.is_empty() {
            while set.len() < INSTALL_HTTP_CONCURRENCY {
                let Some(node) = pending.pop_front() else {
                    break;
                };
                let source = Arc::clone(&self.source);
                set.spawn(async move {
                    let bytes = source.fetch_tarball(&node.tarball).await?;
                    verify_npm_integrity(&node.name, &node.version, &node.integrity, &bytes)?;
                    Ok::<DownloadedNode, InstallError>(DownloadedNode { node, bytes })
                });
            }

            let Some(item) = set.join_next().await else {
                break;
            };
            match item.map_err(worker_join_error)? {
                Ok(done) => {
                    let package = done.node.name.clone();
                    out.push(done);
                    let downloaded = out.len();
                    on_progress(InstallProgress::PackageDownloaded {
                        package,
                        downloaded,
                        total,
                    });
                }
                Err(err) => {
                    set.abort_all();
                    return Err(err);
                }
            }
        }

        Ok(out)
    }

    async fn prefetch_metadata_parallel<F>(
        &self,
        memo: &mut BTreeMap<PackageName, PackageMetadata>,
        names: BTreeSet<PackageName>,
        on_progress: &mut F,
    ) -> Result<(), InstallError>
    where
        F: FnMut(InstallProgress),
    {
        let mut pending = names
            .into_iter()
            .filter(|name| !memo.contains_key(name))
            .collect::<VecDeque<_>>();
        if pending.is_empty() {
            return Ok(());
        }

        let mut set = tokio::task::JoinSet::new();
        while !pending.is_empty() || !set.is_empty() {
            while set.len() < INSTALL_HTTP_CONCURRENCY {
                let Some(name) = pending.pop_front() else {
                    break;
                };
                let source = Arc::clone(&self.source);
                set.spawn(async move {
                    source
                        .fetch_metadata(&name)
                        .await
                        .map(|metadata| (name, metadata))
                });
            }

            let Some(item) = set.join_next().await else {
                break;
            };
            match item.map_err(worker_join_error)? {
                Ok((name, fetched)) => {
                    memo.insert(name.clone(), fetched);
                    on_progress(InstallProgress::MetadataFetched {
                        package: name,
                        fetched: memo.len(),
                    });
                }
                Err(err) => {
                    set.abort_all();
                    return Err(InstallError::Registry(err));
                }
            }
        }

        Ok(())
    }
}

fn worker_join_error(err: tokio::task::JoinError) -> InstallError {
    InstallError::Registry(RegistryError::Fetch {
        target: "registry worker".to_owned(),
        reason: err.to_string(),
    })
}

/// Typed install failures. No network / metadata / integrity path panics.
#[derive(thiserror::Error, Debug)]
pub enum InstallError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("no published version of {name} satisfies {req}")]
    NoMatchingVersion { name: String, req: String },
    #[error(
        "unsupported requirement {req:?} for {name}: meow resolves npm semver clauses, `||` disjunctions, hyphen ranges, dist-tags, and npm aliases; unsupported npm specifier forms (git/file/workspace/url) still fail honestly"
    )]
    UnsupportedRange { name: String, req: String },
    #[error("registry metadata for {name} is missing version {version}")]
    MissingVersion { name: String, version: String },
    #[error("package {name}@{version} has no usable sha512 integrity (only sha1/`shasum`); meow refuses sha1")]
    UnsupportedIntegrity { name: String, version: String },
    #[error("integrity metadata for {name}@{version} is malformed: {reason}")]
    MalformedIntegrity {
        name: String,
        version: String,
        reason: String,
    },
    #[error("integrity check FAILED for {name}@{version}: registry published {expected}, downloaded bytes hash to {got}")]
    IntegrityMismatch {
        name: String,
        version: String,
        expected: String,
        got: String,
    },
}

/// Re-derive root dependency pins for the runtime from declared package.json deps + lockfile.
pub fn resolve_roots(
    declared: &BTreeMap<PackageName, VersionReq>,
    lockfile: &Lockfile,
) -> Result<BTreeMap<PackageName, Version>, RootResolveError> {
    let mut roots = BTreeMap::new();

    for (name, req) in declared {
        let req_arms = req
            .disjunctions()
            .map_err(|_| RootResolveError::UnsupportedRange {
                name: name.to_string(),
                req: req.to_string(),
            })?;

        let mut selected: Option<(semver::Version, Version)> = None;
        for entry in lockfile.iter() {
            if &entry.name != name {
                continue;
            }
            let version = semver::Version::parse(entry.version.as_str()).map_err(|_| {
                RootResolveError::NotInLockfile {
                    name: name.to_string(),
                }
            })?;
            if !req_arms.iter().any(|arm| arm.matches(&version)) {
                continue;
            }
            match &selected {
                Some((best, _)) if version <= *best => {}
                _ => selected = Some((version, entry.version.clone())),
            }
        }

        let Some((_, version)) = selected else {
            return Err(RootResolveError::NotInLockfile {
                name: name.to_string(),
            });
        };
        roots.insert(name.clone(), version);
    }

    Ok(roots)
}

#[derive(thiserror::Error, Debug)]
pub enum RootResolveError {
    #[error("dependency {name} is declared but not in meow.lock.jsonl — run `meow install`")]
    NotInLockfile { name: String },
    #[error("unsupported requirement {req:?} for {name} (see InstallError::UnsupportedRange)")]
    UnsupportedRange { name: String, req: String },
}

/// Pick the highest already-selected version satisfying `spec` (peer dedupe).
/// Returns None for non-range specs or when nothing in the set matches.
fn max_selected_satisfying(versions: &BTreeSet<Version>, spec: &DepSpec) -> Option<Version> {
    let req = match spec {
        DepSpec::Range(req) => req,
        _ => return None,
    };
    let arms = req.disjunctions().ok()?;
    let mut best: Option<(semver::Version, Version)> = None;
    for version in versions {
        let Ok(candidate) = semver::Version::parse(version.as_str()) else {
            continue;
        };
        if !arms.iter().any(|arm| arm.matches(&candidate)) {
            continue;
        }
        match &best {
            Some((b, _)) if candidate <= *b => {}
            _ => best = Some((candidate, version.clone())),
        }
    }
    best.map(|(_, version)| version)
}

fn select_version(
    name: &PackageName,
    meta: &PackageMetadata,
    spec: &DepSpec,
) -> Result<Version, InstallError> {
    match spec {
        DepSpec::Range(req) => select_version_for_range(name, meta, req),
        DepSpec::Tag(tag) => {
            meta.dist_tags
                .get(tag)
                .cloned()
                .ok_or_else(|| InstallError::UnsupportedRange {
                    name: name.to_string(),
                    req: tag.clone(),
                })
        }
        DepSpec::Alias { spec, .. } => select_version(name, meta, spec),
    }
}

fn select_version_for_range(
    name: &PackageName,
    meta: &PackageMetadata,
    req: &VersionReq,
) -> Result<Version, InstallError> {
    let req_arms = req
        .disjunctions()
        .map_err(|_| InstallError::UnsupportedRange {
            name: name.to_string(),
            req: req.to_string(),
        })?;

    let mut selected: Option<(semver::Version, Version)> = None;
    for version in meta.versions.keys() {
        let candidate = semver::Version::parse(version.as_str()).map_err(|_| {
            InstallError::Registry(RegistryError::Metadata {
                name: name.to_string(),
                reason: format!("published version {} failed semver re-parse", version),
            })
        })?;
        if !req_arms.iter().any(|arm| arm.matches(&candidate)) {
            continue;
        }
        match &selected {
            Some((best, _)) if candidate <= *best => {}
            _ => selected = Some((candidate, version.clone())),
        }
    }

    selected
        .map(|(_, version)| version)
        .ok_or_else(|| InstallError::NoMatchingVersion {
            name: name.to_string(),
            req: req.to_string(),
        })
}

fn verify_npm_integrity(
    name: &PackageName,
    version: &Version,
    sri: &str,
    bytes: &[u8],
) -> Result<(), InstallError> {
    let Some(expected) = sri
        .split_whitespace()
        .find(|entry| entry.starts_with("sha512-"))
    else {
        return Err(InstallError::UnsupportedIntegrity {
            name: name.to_string(),
            version: version.to_string(),
        });
    };

    let Some(encoded) = expected.strip_prefix("sha512-") else {
        return Err(InstallError::UnsupportedIntegrity {
            name: name.to_string(),
            version: version.to_string(),
        });
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| InstallError::MalformedIntegrity {
            name: name.to_string(),
            version: version.to_string(),
            reason: err.to_string(),
        })?;
    if decoded.len() != Sha512::output_size() {
        return Err(InstallError::MalformedIntegrity {
            name: name.to_string(),
            version: version.to_string(),
            reason: format!(
                "decoded sha512 digest has length {}, expected {}",
                decoded.len(),
                Sha512::output_size()
            ),
        });
    }

    let mut hasher = Sha512::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    if digest.as_slice() == decoded.as_slice() {
        return Ok(());
    }

    Err(InstallError::IntegrityMismatch {
        name: name.to_string(),
        version: version.to_string(),
        expected: expected.to_owned(),
        got: sha512_sri(bytes),
    })
}
