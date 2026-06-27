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

#[derive(Debug, Clone)]
struct PendingRoot {
    name: PackageName,
    registry_name: PackageName,
    selection_spec: DepSpec,
}

/// Resolve a project's declared dependencies into a complete pinned graph.
pub struct Installer<'a> {
    source: Arc<dyn RegistrySource>,
    cache: &'a Cache,
    registry_url: String,
    meow_req: VersionReq,
    reuse_lockfile: Option<Lockfile>,
    overrides: BTreeMap<PackageName, DepSpec>,
}

/// Phase of the install pipeline — used by the UI to pick an informative label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressPhase {
    Metadata,
    Tarball,
    Cache,
}

/// Every variant carries `(cached, resolved_total)` — two counters that ONLY
/// ever increase.  The UI computes its fraction as `cached / resolved_total`,
/// which is strictly monotonic.  `phase` hints at the current pipeline stage
/// for label generation.
#[derive(Debug, Clone)]
pub enum InstallProgress {
    MetadataFetched {
        package: PackageName,
        cached: usize,
        resolved_total: usize,
        phase: ProgressPhase,
    },
    PackageDownloaded {
        package: PackageName,
        cached: usize,
        resolved_total: usize,
        phase: ProgressPhase,
    },
    PackageCached {
        package: PackageName,
        cached: usize,
        resolved_total: usize,
        phase: ProgressPhase,
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
            overrides: BTreeMap::new(),
        }
    }

    pub fn with_reuse_lockfile(mut self, lockfile: Lockfile) -> Installer<'a> {
        self.reuse_lockfile = Some(lockfile);
        self
    }

    pub fn with_overrides(mut self, overrides: BTreeMap<PackageName, DepSpec>) -> Installer<'a> {
        self.overrides = overrides;
        self
    }

    fn effective_spec<'spec>(
        &'spec self,
        name: &PackageName,
        spec: &'spec DepSpec,
    ) -> &'spec DepSpec {
        self.overrides.get(name).unwrap_or(spec)
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
        let mut metadata_inflight = BTreeSet::new();
        let mut metadata_set = tokio::task::JoinSet::new();
        let mut tarball_set = tokio::task::JoinSet::new();
        let mut pending_tarballs = VecDeque::new();
        let mut root_waiters: BTreeMap<PackageName, Vec<PendingRoot>> = BTreeMap::new();
        let mut exact_waiters: BTreeMap<PackageName, Vec<QueueNode>> = BTreeMap::new();
        let mut ready_exact = VecDeque::new();
        let mut blocked_plans = Vec::new();
        let mut selected: BTreeMap<PackageName, BTreeSet<Version>> = BTreeMap::new();
        let mut done = BTreeSet::new();
        let mut lockfile = Lockfile::new();
        let host = HostPlatform::current();
        let mut resolved_total = 0usize;
        let mut cached = 0usize;

        for (name, spec) in direct {
            let effective_spec = self.effective_spec(name, spec);
            if let Some(entry) = self.cached_spec_lock_entry(name, effective_spec) {
                let package = entry.name.clone();
                if self.accept_reusable_lock_entry(
                    entry,
                    &mut selected,
                    &mut done,
                    &mut ready_exact,
                    &mut lockfile,
                ) {
                    resolved_total += 1;
                    cached += 1;
                    on_progress(InstallProgress::PackageCached {
                        package,
                        cached,
                        resolved_total,
                        phase: ProgressPhase::Cache,
                    });
                }
                continue;
            }

            let registry_name = effective_spec.registry_package(name).clone();
            root_waiters
                .entry(registry_name.clone())
                .or_default()
                .push(PendingRoot {
                    name: name.clone(),
                    registry_name: registry_name.clone(),
                    selection_spec: effective_spec.selection_spec().clone(),
                });
            spawn_metadata_fetch(
                &self.source,
                registry_name,
                &metadata,
                &mut metadata_inflight,
                &mut metadata_set,
            );
        }

        self.drain_ready_resolution(
            &metadata,
            &mut metadata_inflight,
            &mut metadata_set,
            &mut ready_exact,
            &mut exact_waiters,
            &mut blocked_plans,
            &mut selected,
            &mut done,
            &mut lockfile,
            &mut pending_tarballs,
            &mut resolved_total,
            &mut cached,
            &mut on_progress,
            host,
        )?;
        pump_tarball_downloads(&self.source, &mut pending_tarballs, &mut tarball_set);

        while !metadata_set.is_empty() || !tarball_set.is_empty() || !pending_tarballs.is_empty() {
            pump_tarball_downloads(&self.source, &mut pending_tarballs, &mut tarball_set);

            if !metadata_set.is_empty() && !tarball_set.is_empty() {
                tokio::select! {
                    item = metadata_set.join_next() => {
                        self.handle_metadata_result(
                            item,
                            &mut metadata,
                            &mut metadata_inflight,
                            &mut root_waiters,
                            &mut exact_waiters,
                            &mut ready_exact,
                            &mut selected,
                            cached,
                            resolved_total,
                            &mut on_progress,
                        )?;
                        self.drain_ready_resolution(
                            &metadata,
                            &mut metadata_inflight,
                            &mut metadata_set,
                            &mut ready_exact,
                            &mut exact_waiters,
                            &mut blocked_plans,
                            &mut selected,
                            &mut done,
                            &mut lockfile,
                            &mut pending_tarballs,
                            &mut resolved_total,
                            &mut cached,
                            &mut on_progress,
                            host,
                        )?;
                        pump_tarball_downloads(&self.source, &mut pending_tarballs, &mut tarball_set);
                    }
                    item = tarball_set.join_next() => {
                        self.handle_tarball_result(
                            item,
                            &mut lockfile,
                            &mut cached,
                            resolved_total,
                            &mut on_progress,
                        )?;
                        pump_tarball_downloads(&self.source, &mut pending_tarballs, &mut tarball_set);
                    }
                }
            } else if !metadata_set.is_empty() {
                let item = metadata_set.join_next().await;
                self.handle_metadata_result(
                    item,
                    &mut metadata,
                    &mut metadata_inflight,
                    &mut root_waiters,
                    &mut exact_waiters,
                    &mut ready_exact,
                    &mut selected,
                    cached,
                    resolved_total,
                    &mut on_progress,
                )?;
                self.drain_ready_resolution(
                    &metadata,
                    &mut metadata_inflight,
                    &mut metadata_set,
                    &mut ready_exact,
                    &mut exact_waiters,
                    &mut blocked_plans,
                    &mut selected,
                    &mut done,
                    &mut lockfile,
                    &mut pending_tarballs,
                    &mut resolved_total,
                    &mut cached,
                    &mut on_progress,
                    host,
                )?;
                pump_tarball_downloads(&self.source, &mut pending_tarballs, &mut tarball_set);
            } else {
                let item = tarball_set.join_next().await;
                self.handle_tarball_result(
                    item,
                    &mut lockfile,
                    &mut cached,
                    resolved_total,
                    &mut on_progress,
                )?;
                pump_tarball_downloads(&self.source, &mut pending_tarballs, &mut tarball_set);
            }
        }

        if !root_waiters.is_empty() || !exact_waiters.is_empty() || !blocked_plans.is_empty() {
            return Err(InstallError::Registry(RegistryError::Fetch {
                target: "resolver pipeline".to_owned(),
                reason: "metadata pipeline drained before every waiting package was resolved"
                    .to_owned(),
            }));
        }

        Ok(lockfile)
    }

    fn cached_spec_lock_entry(&self, name: &PackageName, spec: &DepSpec) -> Option<LockEntry> {
        let entry = self.locked_entry_for_spec(name, spec)?;
        self.lock_entry_is_reusable(entry).then(|| entry.clone())
    }

    fn cached_exact_lock_entry(&self, name: &PackageName, version: &Version) -> Option<LockEntry> {
        let entry = self
            .reuse_lockfile
            .as_ref()
            .and_then(|lockfile| lockfile.get(name, version))?;
        self.lock_entry_is_reusable(entry).then(|| entry.clone())
    }

    fn lock_entry_is_reusable(&self, entry: &LockEntry) -> bool {
        entry.registry.registry == self.registry_url
            && entry.meow == self.meow_req
            && self.cache.contains(&entry.integrity)
    }

    fn accept_reusable_lock_entry(
        &self,
        entry: LockEntry,
        selected: &mut BTreeMap<PackageName, BTreeSet<Version>>,
        done: &mut BTreeSet<(PackageName, Version)>,
        ready_exact: &mut VecDeque<QueueNode>,
        lockfile: &mut Lockfile,
    ) -> bool {
        if !done.insert((entry.name.clone(), entry.version.clone())) {
            return false;
        }
        selected
            .entry(entry.name.clone())
            .or_default()
            .insert(entry.version.clone());
        for (dep_name, dep_version) in &entry.dependencies {
            selected
                .entry(dep_name.clone())
                .or_default()
                .insert(dep_version.clone());
            ready_exact.push_back(QueueNode {
                name: dep_name.clone(),
                registry_name: dep_name.clone(),
                version: dep_version.clone(),
            });
        }
        lockfile.upsert(entry);
        true
    }

    fn locked_entry_for_spec(&self, name: &PackageName, spec: &DepSpec) -> Option<&LockEntry> {
        let lockfile = self.reuse_lockfile.as_ref()?;
        let req = match spec {
            DepSpec::Range(req) => req,
            DepSpec::Tag(_) => return None,
            DepSpec::Alias { spec, .. } => return self.locked_entry_for_spec(name, spec),
        };
        let arms = req.disjunctions().ok()?;
        let mut selected: Option<(semver::Version, &LockEntry)> = None;
        for entry in lockfile.iter() {
            if &entry.name != name {
                continue;
            }
            if entry.registry.registry != self.registry_url || entry.meow != self.meow_req {
                continue;
            }
            let Ok(version) = semver::Version::parse(entry.version.as_str()) else {
                continue;
            };
            if !arms.iter().any(|arm| arm.matches(&version)) {
                continue;
            }
            match &selected {
                Some((best, _)) if version <= *best => {}
                _ => selected = Some((version, entry)),
            }
        }
        selected.map(|(_, entry)| entry)
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

        if self.cache.contains(&entry.integrity) {
            Ok(Some(entry.clone()))
        } else {
            Ok(None)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn drain_ready_resolution<F>(
        &self,
        metadata: &BTreeMap<PackageName, PackageMetadata>,
        metadata_inflight: &mut BTreeSet<PackageName>,
        metadata_set: &mut tokio::task::JoinSet<
            Result<(PackageName, PackageMetadata), RegistryError>,
        >,
        ready_exact: &mut VecDeque<QueueNode>,
        exact_waiters: &mut BTreeMap<PackageName, Vec<QueueNode>>,
        blocked_plans: &mut Vec<PlannedNode>,
        selected: &mut BTreeMap<PackageName, BTreeSet<Version>>,
        done: &mut BTreeSet<(PackageName, Version)>,
        lockfile: &mut Lockfile,
        pending_tarballs: &mut VecDeque<ResolvedNode>,
        resolved_total: &mut usize,
        cached: &mut usize,
        on_progress: &mut F,
        host: HostPlatform,
    ) -> Result<(), InstallError>
    where
        F: FnMut(InstallProgress),
    {
        loop {
            let mut advanced = false;
            while let Some(node) = ready_exact.pop_front() {
                advanced = true;
                if done.contains(&(node.name.clone(), node.version.clone())) {
                    continue;
                }
                if let Some(entry) = self.cached_exact_lock_entry(&node.name, &node.version) {
                    let package = entry.name.clone();
                    if self.accept_reusable_lock_entry(entry, selected, done, ready_exact, lockfile)
                    {
                        *resolved_total += 1;
                        *cached += 1;
                        on_progress(InstallProgress::PackageCached {
                            package,
                            cached: *cached,
                            resolved_total: *resolved_total,
                            phase: ProgressPhase::Cache,
                        });
                    }
                    continue;
                }
                let Some(meta) = metadata.get(&node.registry_name) else {
                    exact_waiters
                        .entry(node.registry_name.clone())
                        .or_default()
                        .push(node.clone());
                    spawn_metadata_fetch(
                        &self.source,
                        node.registry_name,
                        metadata,
                        metadata_inflight,
                        metadata_set,
                    );
                    continue;
                };
                let version_meta = meta.versions.get(&node.version).cloned().ok_or_else(|| {
                    InstallError::MissingVersion {
                        name: node.registry_name.to_string(),
                        version: node.version.to_string(),
                    }
                })?;
                blocked_plans.push(PlannedNode {
                    name: node.name,
                    version: node.version,
                    version_meta,
                });
            }

            let mut still_blocked = Vec::new();
            for node in blocked_plans.drain(..) {
                let missing =
                    self.missing_metadata_for_planned_node(&node, host, selected, metadata);
                if !missing.is_empty() {
                    for name in missing {
                        spawn_metadata_fetch(
                            &self.source,
                            name,
                            metadata,
                            metadata_inflight,
                            metadata_set,
                        );
                    }
                    still_blocked.push(node);
                    continue;
                }
                advanced = true;
                let resolved =
                    self.finish_planned_node(node, host, selected, metadata, ready_exact)?;
                done.insert((resolved.name.clone(), resolved.version.clone()));
                *resolved_total += 1;
                if let Some(entry) = self.reusable_lock_entry(&resolved)? {
                    let package = entry.name.clone();
                    lockfile.upsert(entry);
                    *cached += 1;
                    on_progress(InstallProgress::PackageCached {
                        package,
                        cached: *cached,
                        resolved_total: *resolved_total,
                        phase: ProgressPhase::Cache,
                    });
                } else {
                    pending_tarballs.push_back(resolved);
                }
            }
            *blocked_plans = still_blocked;

            if !advanced {
                break;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn handle_metadata_result<F>(
        &self,
        item: Option<
            Result<Result<(PackageName, PackageMetadata), RegistryError>, tokio::task::JoinError>,
        >,
        metadata: &mut BTreeMap<PackageName, PackageMetadata>,
        metadata_inflight: &mut BTreeSet<PackageName>,
        root_waiters: &mut BTreeMap<PackageName, Vec<PendingRoot>>,
        exact_waiters: &mut BTreeMap<PackageName, Vec<QueueNode>>,
        ready_exact: &mut VecDeque<QueueNode>,
        selected: &mut BTreeMap<PackageName, BTreeSet<Version>>,
        cached: usize,
        resolved_total: usize,
        on_progress: &mut F,
    ) -> Result<(), InstallError>
    where
        F: FnMut(InstallProgress),
    {
        let Some(item) = item else {
            return Ok(());
        };
        let (name, fetched) = item
            .map_err(worker_join_error)?
            .map_err(InstallError::Registry)?;
        metadata_inflight.remove(&name);
        metadata.insert(name.clone(), fetched);
        on_progress(InstallProgress::MetadataFetched {
            package: name.clone(),
            cached,
            resolved_total,
            phase: ProgressPhase::Metadata,
        });

        if let Some(waiters) = root_waiters.remove(&name) {
            let meta = metadata.get(&name).ok_or_else(|| {
                InstallError::Registry(RegistryError::Metadata {
                    name: name.to_string(),
                    reason: "metadata missing after fetch completion".to_owned(),
                })
            })?;
            for root in waiters {
                let version = select_version(&root.registry_name, meta, &root.selection_spec)?;
                selected
                    .entry(root.name.clone())
                    .or_default()
                    .insert(version.clone());
                ready_exact.push_back(QueueNode {
                    name: root.name,
                    registry_name: root.registry_name,
                    version,
                });
            }
        }
        if let Some(waiters) = exact_waiters.remove(&name) {
            ready_exact.extend(waiters);
        }
        Ok(())
    }

    fn handle_tarball_result<F>(
        &self,
        item: Option<Result<Result<DownloadedNode, InstallError>, tokio::task::JoinError>>,
        lockfile: &mut Lockfile,
        cached: &mut usize,
        resolved_total: usize,
        on_progress: &mut F,
    ) -> Result<(), InstallError>
    where
        F: FnMut(InstallProgress),
    {
        let Some(item) = item else {
            return Ok(());
        };
        let done = item.map_err(worker_join_error)??;
        let package = done.node.name.clone();
        on_progress(InstallProgress::PackageDownloaded {
            package: package.clone(),
            cached: *cached,
            resolved_total,
            phase: ProgressPhase::Tarball,
        });
        let integrity = self.cache.store(&done.bytes)?;
        let node = done.node;
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
        *cached += 1;
        on_progress(InstallProgress::PackageCached {
            package,
            cached: *cached,
            resolved_total,
            phase: ProgressPhase::Cache,
        });
        Ok(())
    }

    fn missing_metadata_for_planned_node(
        &self,
        node: &PlannedNode,
        host: HostPlatform,
        selected: &BTreeMap<PackageName, BTreeSet<Version>>,
        metadata: &BTreeMap<PackageName, PackageMetadata>,
    ) -> BTreeSet<PackageName> {
        let mut missing = BTreeSet::new();
        for (dep, raw_req) in &node.version_meta.dependencies {
            let dep_spec = DepSpec::parse(raw_req);
            let effective_spec = self.effective_spec(dep, &dep_spec);
            let dep_registry_name = effective_spec.registry_package(dep);
            if self
                .locked_entry_for_spec(dep, effective_spec.selection_spec())
                .is_none()
                && !metadata.contains_key(dep_registry_name)
            {
                missing.insert(dep_registry_name.clone());
            }
        }
        for (dep, raw_req) in &node.version_meta.optional_dependencies {
            let dep_spec = DepSpec::parse(raw_req);
            let effective_spec = self.effective_spec(dep, &dep_spec);
            let dep_registry_name = effective_spec.registry_package(dep);
            if !package_name_is_next_swc_for_host(dep_registry_name, host) {
                continue;
            }
            if self
                .locked_entry_for_spec(dep, effective_spec.selection_spec())
                .is_none()
                && !metadata.contains_key(dep_registry_name)
            {
                missing.insert(dep_registry_name.clone());
            }
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
            let dep_spec = DepSpec::parse(raw_req);
            let effective_spec = self.effective_spec(peer, &dep_spec);
            if selected
                .get(peer)
                .and_then(|set| max_selected_satisfying(set, effective_spec.selection_spec()))
                .is_some()
            {
                continue;
            }
            let dep_registry_name = effective_spec.registry_package(peer);
            if self
                .locked_entry_for_spec(peer, effective_spec.selection_spec())
                .is_none()
                && !metadata.contains_key(dep_registry_name)
            {
                missing.insert(dep_registry_name.clone());
            }
        }
        missing
    }

    fn finish_planned_node(
        &self,
        node: PlannedNode,
        host: HostPlatform,
        selected: &mut BTreeMap<PackageName, BTreeSet<Version>>,
        metadata: &BTreeMap<PackageName, PackageMetadata>,
        ready_exact: &mut VecDeque<QueueNode>,
    ) -> Result<ResolvedNode, InstallError> {
        let mut dependencies = BTreeMap::new();
        for (dep, raw_req) in &node.version_meta.dependencies {
            let dep_spec = DepSpec::parse(raw_req);
            let effective_spec = self.effective_spec(dep, &dep_spec);
            let dep_registry_name = effective_spec.registry_package(dep).clone();
            if let Some(entry) = self.locked_entry_for_spec(dep, effective_spec.selection_spec()) {
                dependencies.insert(dep.clone(), entry.version.clone());
                ready_exact.push_back(QueueNode {
                    name: dep.clone(),
                    registry_name: dep_registry_name,
                    version: entry.version.clone(),
                });
                continue;
            }
            let dep_meta = metadata.get(&dep_registry_name).ok_or_else(|| {
                InstallError::Registry(RegistryError::Metadata {
                    name: dep_registry_name.to_string(),
                    reason: "metadata missing after pipeline fetch".to_owned(),
                })
            })?;
            let dep_version = select_version(
                &dep_registry_name,
                dep_meta,
                effective_spec.selection_spec(),
            )?;
            dependencies.insert(dep.clone(), dep_version.clone());
            ready_exact.push_back(QueueNode {
                name: dep.clone(),
                registry_name: dep_registry_name,
                version: dep_version,
            });
        }
        for (dep, raw_req) in &node.version_meta.optional_dependencies {
            let dep_spec = DepSpec::parse(raw_req);
            let effective_spec = self.effective_spec(dep, &dep_spec);
            let dep_registry_name = effective_spec.registry_package(dep).clone();
            if !package_name_is_next_swc_for_host(&dep_registry_name, host) {
                continue;
            }
            if let Some(entry) = self.locked_entry_for_spec(dep, effective_spec.selection_spec()) {
                dependencies.insert(dep.clone(), entry.version.clone());
                ready_exact.push_back(QueueNode {
                    name: dep.clone(),
                    registry_name: dep_registry_name,
                    version: entry.version.clone(),
                });
                continue;
            }
            let dep_meta = metadata.get(&dep_registry_name).ok_or_else(|| {
                InstallError::Registry(RegistryError::Metadata {
                    name: dep_registry_name.to_string(),
                    reason: "metadata missing after pipeline fetch".to_owned(),
                })
            })?;
            let dep_version = select_version(
                &dep_registry_name,
                dep_meta,
                effective_spec.selection_spec(),
            )?;
            let dep_locked_meta = dep_meta.versions.get(&dep_version).ok_or_else(|| {
                InstallError::MissingVersion {
                    name: dep_registry_name.to_string(),
                    version: dep_version.to_string(),
                }
            })?;
            if !is_compatible_optional_dependency(&dep_registry_name, dep_locked_meta, host) {
                continue;
            }
            dependencies.insert(dep.clone(), dep_version.clone());
            ready_exact.push_back(QueueNode {
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
            let effective_spec = self.effective_spec(peer, &dep_spec);
            if let Some(existing) = selected
                .get(peer)
                .and_then(|set| max_selected_satisfying(set, effective_spec.selection_spec()))
            {
                dependencies.insert(peer.clone(), existing);
                continue;
            }
            let dep_registry_name = effective_spec.registry_package(peer).clone();
            if let Some(entry) = self.locked_entry_for_spec(peer, effective_spec.selection_spec()) {
                dependencies.insert(peer.clone(), entry.version.clone());
                selected
                    .entry(peer.clone())
                    .or_default()
                    .insert(entry.version.clone());
                ready_exact.push_back(QueueNode {
                    name: peer.clone(),
                    registry_name: dep_registry_name,
                    version: entry.version.clone(),
                });
                continue;
            }
            let Some(dep_meta) = metadata.get(&dep_registry_name) else {
                continue;
            };
            let Ok(dep_version) = select_version(
                &dep_registry_name,
                dep_meta,
                effective_spec.selection_spec(),
            ) else {
                continue;
            };
            dependencies.insert(peer.clone(), dep_version.clone());
            selected
                .entry(peer.clone())
                .or_default()
                .insert(dep_version.clone());
            ready_exact.push_back(QueueNode {
                name: peer.clone(),
                registry_name: dep_registry_name,
                version: dep_version,
            });
        }

        Ok(ResolvedNode {
            name: node.name,
            version: node.version,
            tarball: node.version_meta.dist.tarball,
            integrity: node.version_meta.dist.integrity,
            dependencies,
        })
    }
}

fn spawn_metadata_fetch(
    source: &Arc<dyn RegistrySource>,
    name: PackageName,
    metadata: &BTreeMap<PackageName, PackageMetadata>,
    metadata_inflight: &mut BTreeSet<PackageName>,
    set: &mut tokio::task::JoinSet<Result<(PackageName, PackageMetadata), RegistryError>>,
) {
    if metadata.contains_key(&name) || !metadata_inflight.insert(name.clone()) {
        return;
    }
    let source = Arc::clone(source);
    set.spawn(async move {
        source
            .fetch_metadata(&name)
            .await
            .map(|metadata| (name, metadata))
    });
}

fn pump_tarball_downloads(
    source: &Arc<dyn RegistrySource>,
    pending: &mut VecDeque<ResolvedNode>,
    set: &mut tokio::task::JoinSet<Result<DownloadedNode, InstallError>>,
) {
    while set.len() < INSTALL_HTTP_CONCURRENCY {
        let Some(node) = pending.pop_front() else {
            break;
        };
        let source = Arc::clone(source);
        set.spawn(async move {
            let bytes = source.fetch_tarball(&node.tarball).await?;
            let name = node.name.clone();
            let version = node.version.clone();
            let integrity = node.integrity.clone();
            let (bytes, result) = tokio::task::spawn_blocking(move || {
                let result = verify_npm_integrity(&name, &version, &integrity, &bytes);
                (bytes, result)
            })
            .await
            .map_err(|e| InstallError::Registry(RegistryError::Fetch {
                target: "integrity worker".to_owned(),
                reason: e.to_string(),
            }))?;
            result?;
            Ok::<DownloadedNode, InstallError>(DownloadedNode { node, bytes })
        });
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
    declared: &BTreeMap<PackageName, DepSpec>,
    lockfile: &Lockfile,
) -> Result<BTreeMap<PackageName, Version>, RootResolveError> {
    let mut roots = BTreeMap::new();

    for (name, spec) in declared {
        let Some(version) = max_locked_satisfying(name, spec.selection_spec(), lockfile)? else {
            return Err(RootResolveError::NotInLockfile {
                name: name.to_string(),
            });
        };
        roots.insert(name.clone(), version);
    }

    Ok(roots)
}

fn max_locked_satisfying(
    name: &PackageName,
    spec: &DepSpec,
    lockfile: &Lockfile,
) -> Result<Option<Version>, RootResolveError> {
    let req = match spec {
        DepSpec::Range(req) => req,
        DepSpec::Tag(tag) => {
            return Err(RootResolveError::UnsupportedRange {
                name: name.to_string(),
                req: tag.clone(),
            });
        }
        DepSpec::Alias { spec, .. } => return max_locked_satisfying(name, spec, lockfile),
    };
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

    Ok(selected.map(|(_, version)| version))
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
