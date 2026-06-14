use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::archive;
use crate::{tmp_path, Cache, ContentHash, PackageName, ResolutionGraph, UnpackedStore, Version};

const STORE_DIR: &str = ".meow";
const SIDECAR_NAME: &str = ".materialized";
pub(crate) const SIDECAR_VERSION: u32 = 1;
pub(crate) const DIR_MODE: u32 = 0o755;
pub(crate) const FILE_MODE: u32 = 0o644;
pub(crate) const EXEC_MODE: u32 = 0o755;
const ARCHIVE_LABEL: &str = "<archive>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    NodeModules,
    Vendor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStrategy {
    Symlink,
    Copy,
    Auto,
}

#[derive(Debug, Clone)]
pub struct MaterializeOptions {
    pub projection: Projection,
    pub link: LinkStrategy,
    pub clean: bool,
    pub vendor_dir: PathBuf,
}

impl MaterializeOptions {
    pub fn node_modules() -> Self {
        MaterializeOptions {
            projection: Projection::NodeModules,
            link: LinkStrategy::Symlink,
            clean: false,
            vendor_dir: PathBuf::from("vendor"),
        }
    }

    pub fn vendor() -> Self {
        MaterializeOptions {
            projection: Projection::Vendor,
            link: LinkStrategy::Copy,
            clean: false,
            vendor_dir: PathBuf::from("vendor"),
        }
    }
}

pub struct Materializer<'a> {
    cache: &'a Cache,
    manifest: &'a ResolutionGraph,
    project_root: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializePlan {
    root: PathBuf,
    nodes: Vec<PlanNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanNode {
    pub path: PathBuf,
    pub entry: PlanEntry,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanEntry {
    Package {
        integrity: ContentHash,
        name: PackageName,
        version: Version,
    },
    Edge {
        target: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeReport {
    pub root: PathBuf,
    pub tree_hash: ContentHash,
    pub packages: usize,
    pub edges: usize,
    pub bytes_written: u64,
    pub skipped: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum MaterializeError {
    #[error("materialize I/O at {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("reading {name}@{version} from cache")]
    Cache {
        name: String,
        version: String,
        #[source]
        source: crate::CacheError,
    },
    #[error("reading cached blob {hash} for the unpacked store")]
    CacheBlob {
        hash: String,
        #[source]
        source: crate::CacheError,
    },
    #[error("malformed package archive for {name}@{version}: {reason}")]
    InvalidArchive {
        name: String,
        version: String,
        reason: String,
    },
    #[error("unsafe archive member {member:?} in {name}@{version} (path escape / absolute / link entry)")]
    UnsafeMember {
        name: String,
        version: String,
        member: String,
    },
    #[error("symlinks unsupported at {} on this filesystem; use --vendor for a copy-based projection", .path.display())]
    SymlinkUnsupported { path: PathBuf },
    #[error("dependency edge {dep}@{ver} of {name}@{version} is not in the resolved closure")]
    DanglingEdge {
        name: String,
        version: String,
        dep: String,
        ver: String,
    },
}

impl MaterializeError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> MaterializeError {
        MaterializeError::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn invalid_archive(reason: impl Into<String>) -> MaterializeError {
        MaterializeError::InvalidArchive {
            name: ARCHIVE_LABEL.to_owned(),
            version: ARCHIVE_LABEL.to_owned(),
            reason: reason.into(),
        }
    }

    pub(crate) fn unsafe_member(member: impl Into<String>) -> MaterializeError {
        MaterializeError::UnsafeMember {
            name: ARCHIVE_LABEL.to_owned(),
            version: ARCHIVE_LABEL.to_owned(),
            member: member.into(),
        }
    }

    fn with_package(self, name: &PackageName, version: &Version) -> MaterializeError {
        match self {
            MaterializeError::InvalidArchive { reason, .. } => MaterializeError::InvalidArchive {
                name: name.to_string(),
                version: version.to_string(),
                reason,
            },
            MaterializeError::UnsafeMember { member, .. } => MaterializeError::UnsafeMember {
                name: name.to_string(),
                version: version.to_string(),
                member,
            },
            other => other,
        }
    }
}

impl MaterializePlan {
    pub fn tree_hash(&self) -> ContentHash {
        let mut bytes = Vec::new();
        for node in &self.nodes {
            bytes.extend_from_slice(path_key(&node.path).as_bytes());
            bytes.push(0);
            match &node.entry {
                PlanEntry::Package { integrity, .. } => {
                    bytes.extend_from_slice(b"package");
                    bytes.push(0);
                    bytes.extend_from_slice(integrity.to_sri().as_bytes());
                }
                PlanEntry::Edge { target } => {
                    bytes.extend_from_slice(b"edge");
                    bytes.push(0);
                    bytes.extend_from_slice(path_key(target).as_bytes());
                }
            }
            bytes.push(0);
            bytes.extend_from_slice(format!("{:o}", node.mode).as_bytes());
            bytes.push(b'\n');
        }
        ContentHash::of(&bytes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn packages(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| matches!(node.entry, PlanEntry::Package { .. }))
            .count()
    }

    pub fn edges(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| matches!(node.entry, PlanEntry::Edge { .. }))
            .count()
    }
}

impl<'a> Materializer<'a> {
    pub fn new(cache: &'a Cache, manifest: &'a ResolutionGraph, project_root: &'a Path) -> Self {
        Materializer {
            cache,
            manifest,
            project_root,
        }
    }

    pub fn plan(&self, opts: &MaterializeOptions) -> Result<MaterializePlan, MaterializeError> {
        let root_rel = projection_root_relative(opts, self.project_root)?;
        let root = self.project_root.join(&root_rel);
        let mut packages = BTreeSet::new();
        let mut store_paths = BTreeMap::new();
        let mut nodes = Vec::new();

        for pkg in self.manifest.packages() {
            let key = (pkg.name.clone(), pkg.version.clone());
            packages.insert(key.clone());
            let store_path = store_package_path(&root_rel, pkg.name, pkg.version);
            store_paths.insert(key, store_path.clone());
            nodes.push(PlanNode {
                path: store_path,
                entry: PlanEntry::Package {
                    integrity: pkg.integrity.clone(),
                    name: pkg.name.clone(),
                    version: pkg.version.clone(),
                },
                mode: DIR_MODE,
            });
        }

        for pkg in self.manifest.packages() {
            for (dep, dep_version) in pkg.dependencies {
                let dep_key = (dep.clone(), dep_version.clone());
                let Some(dep_store_path) = store_paths.get(&dep_key) else {
                    return Err(MaterializeError::DanglingEdge {
                        name: pkg.name.to_string(),
                        version: pkg.version.to_string(),
                        dep: dep.to_string(),
                        ver: dep_version.to_string(),
                    });
                };
                let edge_path = store_edge_path(&root_rel, pkg.name, pkg.version, dep);
                let edge_parent = edge_path.parent().map(PathBuf::from).unwrap_or_default();
                let target = relative_path(&edge_parent, dep_store_path);
                nodes.push(PlanNode {
                    path: edge_path,
                    entry: PlanEntry::Edge { target },
                    mode: DIR_MODE,
                });
            }
        }

        for (name, version) in self.manifest.root_deps() {
            let dep_key = (name.clone(), version.clone());
            let Some(dep_store_path) = store_paths.get(&dep_key) else {
                return Err(MaterializeError::DanglingEdge {
                    name: "<root>".to_owned(),
                    version: "<root>".to_owned(),
                    dep: name.to_string(),
                    ver: version.to_string(),
                });
            };
            let edge_path = root_edge_path(&root_rel, name);
            let edge_parent = edge_path.parent().map(PathBuf::from).unwrap_or_default();
            let target = relative_path(&edge_parent, dep_store_path);
            nodes.push(PlanNode {
                path: edge_path,
                entry: PlanEntry::Edge { target },
                mode: DIR_MODE,
            });
        }

        nodes.sort_by_cached_key(|node| path_key(&node.path));
        Ok(MaterializePlan { root, nodes })
    }

    pub fn materialize(
        &self,
        opts: &MaterializeOptions,
    ) -> Result<MaterializeReport, MaterializeError> {
        let plan = self.plan(opts)?;
        let tree_hash = plan.tree_hash();
        if tree_is_current(&plan, opts, &tree_hash, self.cache)? {
            return Ok(MaterializeReport {
                root: plan.root.clone(),
                tree_hash,
                packages: plan.packages(),
                edges: plan.edges(),
                bytes_written: 0,
                skipped: true,
            });
        }

        let root_rel = projection_root_relative(opts, self.project_root)?;
        let tmp_root = tmp_path(plan.root());
        cleanup_best_effort(&tmp_root);
        ensure_projection_parent(&tmp_root)?;
        if opts.clean && plan.root.exists() {
            remove_path(plan.root())?;
        }

        let catalog = package_catalog(self.manifest, &root_rel);
        let path_index = path_index(&catalog);
        let mut bytes_written = 0;

        let build_result = (|| -> Result<(), MaterializeError> {
            ensure_dir(&tmp_root)?;
            ensure_dir(&tmp_root.join(STORE_DIR))?;
            let mut built: BTreeMap<(PackageName, Version), PathBuf> = BTreeMap::new();

            let link = effective_link(opts);
            let unpacked_store = if matches!(link, LinkStrategy::Symlink) {
                let cache_root = self.cache.root().to_path_buf();
                Some(UnpackedStore::new(
                    cache_root.join("unpacked"),
                    Arc::new(Cache::with_root(cache_root)),
                ))
            } else {
                None
            };

            for node in &plan.nodes {
                let PlanEntry::Package {
                    integrity,
                    name,
                    version,
                } = &node.entry
                else {
                    continue;
                };
                let rel = path_inside_root(&node.path, &root_rel)?;
                let abs = tmp_root.join(&rel);
                match link {
                    LinkStrategy::Copy => {
                        let bytes = self.cache.read(integrity).map_err(|source| {
                            MaterializeError::Cache {
                                name: name.to_string(),
                                version: version.to_string(),
                                source,
                            }
                        })?;
                        let stats = archive::unpack_to(&bytes, &abs)
                            .map_err(|err| err.with_package(name, version))?;
                        bytes_written += stats.bytes;
                    }
                    LinkStrategy::Symlink => {
                        if needs_real_tree_for_native_walkers(&rel) {
                            let bytes = self.cache.read(integrity).map_err(|source| {
                                MaterializeError::Cache {
                                    name: name.to_string(),
                                    version: version.to_string(),
                                    source,
                                }
                            })?;
                            let stats = archive::unpack_to(&bytes, &abs)
                                .map_err(|err| err.with_package(name, version))?;
                            bytes_written += stats.bytes;
                        } else {
                            let store = unpacked_store.as_ref().expect("unpacked store");
                            let source = store.ensure(integrity)?;
                            ensure_dir(abs.parent().unwrap_or(&tmp_root))?;
                            create_symlink(&source, &abs)?;
                        }
                    }
                    LinkStrategy::Auto => unreachable!("effective link policy resolves auto"),
                }
            }

            for node in &plan.nodes {
                let PlanEntry::Edge { target } = &node.entry else {
                    continue;
                };
                let rel = path_inside_root(&node.path, &root_rel)?;
                let abs = tmp_root.join(&rel);
                let parent_rel = rel.parent().unwrap_or(Path::new(""));
                let parent_abs = abs.parent().unwrap_or(&tmp_root).to_path_buf();
                ensure_dir(&parent_abs)?;
                let target_rel = normalize_relative_join(parent_rel, target)?;
                let Some(key) = path_index.get(&target_rel) else {
                    return Err(MaterializeError::DanglingEdge {
                        name: rel.display().to_string(),
                        version: rel.display().to_string(),
                        dep: target.display().to_string(),
                        ver: target_rel.display().to_string(),
                    });
                };
                match link {
                    LinkStrategy::Symlink => {
                        if needs_real_tree_for_native_walkers(&rel) {
                            bytes_written += copy_edge_tree(
                                &abs,
                                key,
                                &catalog,
                                &tmp_root,
                                &mut Vec::new(),
                                &mut built,
                            )?;
                        } else {
                            create_symlink(target, &abs)?;
                        }
                    }
                    LinkStrategy::Copy => {
                        bytes_written += copy_edge_tree(
                            &abs,
                            key,
                            &catalog,
                            &tmp_root,
                            &mut Vec::new(),
                            &mut built,
                        )?;
                    }
                    LinkStrategy::Auto => unreachable!("effective link policy resolves auto"),
                }
            }

            write_sidecar(
                &tmp_root,
                &Sidecar {
                    version: SIDECAR_VERSION,
                    projection: projection_name(opts.projection).to_owned(),
                    tree_hash: tree_hash.to_sri(),
                },
            )?;
            Ok(())
        })();

        if let Err(err) = build_result {
            cleanup_best_effort(&tmp_root);
            return Err(err);
        }

        if !opts.clean && plan.root.exists() {
            let backup = tmp_path(&tmp_root);
            cleanup_best_effort(&backup);
            fs::rename(plan.root(), &backup)
                .map_err(|source| MaterializeError::io(plan.root(), source))?;
            if let Err(source) = fs::rename(&tmp_root, plan.root()) {
                let restore = fs::rename(&backup, plan.root());
                cleanup_best_effort(&tmp_root);
                if restore.is_err() {
                    cleanup_best_effort(&backup);
                }
                return Err(MaterializeError::io(plan.root(), source));
            }
            remove_path(&backup)?;
        } else {
            ensure_projection_parent(plan.root())?;
            fs::rename(&tmp_root, plan.root())
                .map_err(|source| MaterializeError::io(plan.root(), source))?;
        }

        Ok(MaterializeReport {
            root: plan.root.clone(),
            tree_hash,
            packages: plan.packages(),
            edges: plan.edges(),
            bytes_written,
            skipped: false,
        })
    }
}

#[derive(Debug, Clone)]
struct PackageRecord {
    name: PackageName,
    version: Version,
    dependencies: BTreeMap<PackageName, Version>,
    store_path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    version: u32,
    projection: String,
    tree_hash: String,
}

fn package_catalog(
    manifest: &ResolutionGraph,
    root_rel: &Path,
) -> BTreeMap<(PackageName, Version), PackageRecord> {
    let mut records = BTreeMap::new();
    for pkg in manifest.packages() {
        let key = (pkg.name.clone(), pkg.version.clone());
        records.insert(
            key,
            PackageRecord {
                name: pkg.name.clone(),
                version: pkg.version.clone(),
                dependencies: pkg.dependencies.clone(),
                store_path: strip_root_prefix(
                    &store_package_path(root_rel, pkg.name, pkg.version),
                    root_rel,
                ),
            },
        );
    }
    records
}

fn needs_real_tree_for_native_walkers(rel: &Path) -> bool {
    let mut parts = rel.components();
    let first = parts.next().map(|part| part.as_os_str().to_string_lossy());
    let second = parts.next().map(|part| part.as_os_str().to_string_lossy());
    match (first.as_deref(), second.as_deref(), parts.next()) {
        (Some(scope), Some(_name), None) if scope.starts_with('@') => true,
        (Some(_name), None, None) => true,
        _ => false,
    }
}

fn path_index(
    catalog: &BTreeMap<(PackageName, Version), PackageRecord>,
) -> BTreeMap<PathBuf, (PackageName, Version)> {
    let mut index = BTreeMap::new();
    for (key, record) in catalog {
        index.insert(record.store_path.clone(), key.clone());
    }
    index
}

fn tree_is_current(
    plan: &MaterializePlan,
    opts: &MaterializeOptions,
    tree_hash: &ContentHash,
    cache: &Cache,
) -> Result<bool, MaterializeError> {
    let sidecar = read_sidecar(plan.root())?;
    let Some(sidecar) = sidecar else {
        return Ok(false);
    };
    if sidecar.version != SIDECAR_VERSION
        || sidecar.projection != projection_name(opts.projection)
        || sidecar.tree_hash != tree_hash.to_sri()
    {
        return Ok(false);
    }
    if !plan.root.is_dir() {
        return Ok(false);
    }
    let root_rel = projection_root_relative(opts, plan.root.parent().unwrap_or(Path::new("")))?;
    let link = effective_link(opts);
    for node in &plan.nodes {
        let rel = path_inside_root(&node.path, &root_rel)?;
        let abs = plan.root.join(&rel);
        match &node.entry {
            PlanEntry::Package { integrity, .. } => match link {
                LinkStrategy::Copy => {
                    let meta = match fs::symlink_metadata(&abs) {
                        Ok(meta) => meta,
                        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
                        Err(err) => return Err(MaterializeError::io(&abs, err)),
                    };
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        return Ok(false);
                    }
                }
                LinkStrategy::Symlink => {
                    let meta = match fs::symlink_metadata(&abs) {
                        Ok(meta) => meta,
                        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
                        Err(err) => return Err(MaterializeError::io(&abs, err)),
                    };
                    if !meta.file_type().is_symlink() {
                        return Ok(false);
                    }
                    let target =
                        fs::read_link(&abs).map_err(|source| MaterializeError::io(&abs, source))?;
                    let expected = cache.root().join("unpacked").join(integrity.to_url_host());
                    if target != expected {
                        return Ok(false);
                    }
                    if !abs.is_dir() {
                        return Ok(false);
                    }
                }
                LinkStrategy::Auto => unreachable!("effective link policy resolves auto"),
            },
            PlanEntry::Edge { target } => match link {
                LinkStrategy::Symlink => {
                    let meta = match fs::symlink_metadata(&abs) {
                        Ok(meta) => meta,
                        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
                        Err(err) => return Err(MaterializeError::io(&abs, err)),
                    };
                    if needs_real_tree_for_native_walkers(&rel) {
                        if !meta.is_dir() || meta.file_type().is_symlink() {
                            return Ok(false);
                        }
                    } else {
                        if !meta.file_type().is_symlink() {
                            return Ok(false);
                        }
                        let actual = fs::read_link(&abs)
                            .map_err(|source| MaterializeError::io(&abs, source))?;
                        if actual != *target {
                            return Ok(false);
                        }
                    }
                }
                LinkStrategy::Copy => {
                    let meta = match fs::symlink_metadata(&abs) {
                        Ok(meta) => meta,
                        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
                        Err(err) => return Err(MaterializeError::io(&abs, err)),
                    };
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        return Ok(false);
                    }
                }
                LinkStrategy::Auto => unreachable!("effective link policy resolves auto"),
            },
        }
    }
    Ok(true)
}

fn copy_edge_tree(
    dest: &Path,
    key: &(PackageName, Version),
    catalog: &BTreeMap<(PackageName, Version), PackageRecord>,
    root: &Path,
    stack: &mut Vec<(PackageName, Version)>,
    built: &mut BTreeMap<(PackageName, Version), PathBuf>,
) -> Result<u64, MaterializeError> {
    // Dedup the dependency DAG: materialize each (name, version) subtree as a real tree
    // exactly once. Repeat occurrences become a relative symlink to the first copy.
    // Without this, a shared/diamond dependency is re-copied once per path through the
    // graph -> exponential work that never terminates on real npm trees (the install hang).
    if let Some(canonical) = built.get(key) {
        if canonical.as_path() == dest {
            return Ok(0);
        }
        if let Some(parent) = dest.parent() {
            ensure_dir(parent)?;
        }
        let from_dir = dest.parent().unwrap_or(root);
        let rel_target = relative_path(from_dir, canonical);
        create_symlink(&rel_target, dest)?;
        return Ok(0);
    }
    let Some(record) = catalog.get(key) else {
        return Err(MaterializeError::DanglingEdge {
            name: "<copy>".to_owned(),
            version: "<copy>".to_owned(),
            dep: key.0.to_string(),
            ver: key.1.to_string(),
        });
    };
    let source = root.join(&record.store_path);
    let bytes = copy_dir_recursive(&source, dest)?;
    built.insert(key.clone(), dest.to_path_buf());
    stack.push((record.name.clone(), record.version.clone()));
    let mut total = bytes;
    for (dep, version) in &record.dependencies {
        let dep_key = (dep.clone(), version.clone());
        if stack.contains(&dep_key) {
            continue;
        }
        let child = dest.join("node_modules").join(package_rel_path(dep));
        total += copy_edge_tree(&child, &dep_key, catalog, root, stack, built)?;
    }
    let _ = stack.pop();
    Ok(total)
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<u64, MaterializeError> {
    ensure_dir(dest)?;
    let mut total = 0;
    let entries =
        fs::read_dir(source).map_err(|source_err| MaterializeError::io(source, source_err))?;
    for entry in entries {
        let entry = entry.map_err(|source_err| MaterializeError::io(source, source_err))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source_err| MaterializeError::io(&entry_path, source_err))?;
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            total += copy_dir_recursive(&entry_path, &dest_path)?;
            continue;
        }
        if file_type.is_file() {
            total += copy_file(&entry_path, &dest_path)?;
            continue;
        }
        return Err(MaterializeError::invalid_archive(format!(
            "unsupported copied member {:?}",
            entry_path
        )));
    }
    Ok(total)
}

fn copy_file(source: &Path, dest: &Path) -> Result<u64, MaterializeError> {
    if let Some(parent) = dest.parent() {
        ensure_dir(parent)?;
    }
    let mut src =
        fs::File::open(source).map_err(|source_err| MaterializeError::io(source, source_err))?;
    let mut dst =
        fs::File::create(dest).map_err(|source_err| MaterializeError::io(dest, source_err))?;
    let mut buf = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let read = src
            .read(&mut buf)
            .map_err(|source_err| MaterializeError::io(source, source_err))?;
        if read == 0 {
            break;
        }
        dst.write_all(&buf[..read])
            .map_err(|source_err| MaterializeError::io(dest, source_err))?;
        total += read as u64;
    }
    let mode = file_mode_from_metadata(source)?;
    normalize_path(dest, mode)?;
    Ok(total)
}

fn write_sidecar(root: &Path, sidecar: &Sidecar) -> Result<(), MaterializeError> {
    let path = sidecar_path(root);
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let bytes = serde_json::to_vec(sidecar)
        .map_err(|source| MaterializeError::invalid_archive(source.to_string()))?;
    fs::write(&path, &bytes).map_err(|source| MaterializeError::io(&path, source))?;
    normalize_path(&path, FILE_MODE)?;
    Ok(())
}

fn read_sidecar(root: &Path) -> Result<Option<Sidecar>, MaterializeError> {
    let path = sidecar_path(root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(MaterializeError::io(&path, err)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(sidecar) => Ok(Some(sidecar)),
        Err(_) => Ok(None),
    }
}

fn sidecar_path(root: &Path) -> PathBuf {
    root.join(STORE_DIR).join(SIDECAR_NAME)
}

fn projection_root_relative(
    opts: &MaterializeOptions,
    project_root: &Path,
) -> Result<PathBuf, MaterializeError> {
    match opts.projection {
        Projection::NodeModules => Ok(PathBuf::from("node_modules")),
        Projection::Vendor => {
            sanitize_relative_path(&opts.vendor_dir).map_err(|source| MaterializeError::Io {
                path: project_root.join(&opts.vendor_dir),
                source,
            })
        }
    }
}

fn sanitize_relative_path(path: &Path) -> Result<PathBuf, io::Error> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "projection path must stay within the project root",
                ));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "projection path must not be empty",
        ));
    }
    Ok(out)
}

fn store_package_path(root_rel: &Path, name: &PackageName, version: &Version) -> PathBuf {
    let mut path = root_rel
        .join(STORE_DIR)
        .join(store_key(name, version))
        .join("node_modules");
    path.push(package_rel_path(name));
    path
}

fn store_edge_path(
    root_rel: &Path,
    importer: &PackageName,
    importer_version: &Version,
    dep: &PackageName,
) -> PathBuf {
    let mut path = root_rel
        .join(STORE_DIR)
        .join(store_key(importer, importer_version))
        .join("node_modules");
    path.push(package_rel_path(dep));
    path
}

fn root_edge_path(root_rel: &Path, name: &PackageName) -> PathBuf {
    let mut path = root_rel.to_path_buf();
    path.push(package_rel_path(name));
    path
}

fn store_key(name: &PackageName, version: &Version) -> String {
    format!("{}@{}", name.as_str().replace('/', "+"), version)
}

fn package_rel_path(name: &PackageName) -> PathBuf {
    let mut path = PathBuf::new();
    for segment in name.as_str().split('/') {
        path.push(segment);
    }
    path
}

fn relative_path(from_dir: &Path, to_path: &Path) -> PathBuf {
    let from = path_parts(from_dir);
    let to = path_parts(to_path);
    let mut common = 0;
    while common < from.len() && common < to.len() && from[common] == to[common] {
        common += 1;
    }
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &to[common..] {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn normalize_relative_join(base: &Path, rel: &Path) -> Result<PathBuf, MaterializeError> {
    let mut parts = path_parts(base);
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(MaterializeError::invalid_archive(format!(
                        "relative target escapes root: {:?}",
                        rel
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(MaterializeError::invalid_archive(format!(
                    "absolute edge target {:?}",
                    rel
                )));
            }
        }
    }
    Ok(parts_to_path(&parts))
}

fn path_inside_root(path: &Path, root_rel: &Path) -> Result<PathBuf, MaterializeError> {
    path.strip_prefix(root_rel).map(PathBuf::from).map_err(|_| {
        MaterializeError::io(
            path,
            io::Error::new(
                ErrorKind::InvalidInput,
                "planned path escaped projection root",
            ),
        )
    })
}

fn strip_root_prefix(path: &Path, root_rel: &Path) -> PathBuf {
    path.strip_prefix(root_rel)
        .map(PathBuf::from)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn ensure_projection_parent(path: &Path) -> Result<(), MaterializeError> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    Ok(())
}

fn ensure_dir(path: &Path) -> Result<(), MaterializeError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|source| MaterializeError::io(path, source))?;
    normalize_path(path, DIR_MODE)
}

fn create_symlink(target: &Path, path: &Path) -> Result<(), MaterializeError> {
    #[cfg(unix)]
    {
        symlink(target, path).map_err(|_| MaterializeError::SymlinkUnsupported {
            path: path.to_path_buf(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        let _ = path;
        Err(MaterializeError::SymlinkUnsupported {
            path: path.to_path_buf(),
        })
    }
}

fn remove_path(path: &Path) -> Result<(), MaterializeError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(MaterializeError::io(path, err)),
    };
    let result = if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|source| MaterializeError::io(path, source))
}

fn cleanup_best_effort(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        let _ = if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
    }
}

#[cfg(unix)]
fn file_mode_from_metadata(path: &Path) -> Result<u32, MaterializeError> {
    let meta = fs::metadata(path).map_err(|source| MaterializeError::io(path, source))?;
    Ok(if meta.permissions().mode() & 0o111 != 0 {
        EXEC_MODE
    } else {
        FILE_MODE
    })
}

#[cfg(not(unix))]
fn file_mode_from_metadata(path: &Path) -> Result<u32, MaterializeError> {
    let _ = fs::metadata(path).map_err(|source| MaterializeError::io(path, source))?;
    Ok(FILE_MODE)
}

fn normalize_path(path: &Path, mode: u32) -> Result<(), MaterializeError> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|source| MaterializeError::io(path, source))?;
    }
    set_fixed_times(path)
}

fn set_fixed_times(path: &Path) -> Result<(), MaterializeError> {
    let file = fs::File::open(path).map_err(|source| MaterializeError::io(path, source))?;
    let fixed = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
    file.set_times(fs::FileTimes::new().set_accessed(fixed).set_modified(fixed))
        .map_err(|source| MaterializeError::io(path, source))
}

fn path_key(path: &Path) -> String {
    let mut key = String::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !key.is_empty() {
                    key.push('/');
                }
                key.push_str("..");
            }
            Component::Normal(part) => {
                if !key.is_empty() {
                    key.push('/');
                }
                key.push_str(&part.to_string_lossy());
            }
            Component::RootDir => key.push('/'),
            Component::Prefix(prefix) => key.push_str(&prefix.as_os_str().to_string_lossy()),
        }
    }
    key
}

fn path_parts(path: &Path) -> Vec<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => parts.push("..".to_owned()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    parts
}

fn parts_to_path(parts: &[String]) -> PathBuf {
    let mut path = PathBuf::new();
    for part in parts {
        path.push(part);
    }
    path
}

fn effective_link(opts: &MaterializeOptions) -> LinkStrategy {
    match opts.projection {
        Projection::NodeModules => LinkStrategy::Symlink,
        Projection::Vendor => LinkStrategy::Copy,
    }
}

fn projection_name(projection: Projection) -> &'static str {
    match projection {
        Projection::NodeModules => "node_modules",
        Projection::Vendor => "vendor",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use crate::{
        ContentHash, LockEntry, Lockfile, PackageName, RegistryProvenance, ResolutionGraph,
        Version, VersionReq,
    };

    use super::*;

    fn lock_entry(
        name: &str,
        version: &str,
        integrity: ContentHash,
        deps: &[(&str, &str)],
    ) -> LockEntry {
        LockEntry {
            name: PackageName::new(name),
            version: Version::parse(version).expect("valid version"),
            integrity,
            dependencies: deps
                .iter()
                .map(|(dep, dep_version)| {
                    (
                        PackageName::new(*dep),
                        Version::parse(dep_version).expect("valid version"),
                    )
                })
                .collect(),
            registry: RegistryProvenance::new("https://registry.npmjs.org"),
            capabilities: vec![],
            wasm: vec![],
            meow: VersionReq::parse("*").expect("valid req"),
        }
    }

    fn roots(entries: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
        entries
            .iter()
            .map(|(name, version)| {
                (
                    PackageName::new(*name),
                    Version::parse(version).expect("valid version"),
                )
            })
            .collect()
    }

    #[test]
    fn scoped_store_keys_and_relative_targets_are_stable() {
        let root_rel = PathBuf::from("node_modules");
        let pkg = PackageName::new("@scope/pkg");
        let dep = PackageName::new("left-pad");
        let version = Version::parse("1.2.3").expect("valid version");
        let dep_version = Version::parse("4.5.6").expect("valid version");

        assert_eq!(store_key(&pkg, &version), "@scope+pkg@1.2.3");
        assert_eq!(
            path_key(&store_package_path(&root_rel, &pkg, &version)),
            "node_modules/.meow/@scope+pkg@1.2.3/node_modules/@scope/pkg"
        );

        let edge = store_edge_path(&root_rel, &pkg, &version, &dep);
        let target = store_package_path(&root_rel, &dep, &dep_version);
        assert_eq!(
            path_key(&relative_path(
                edge.parent().unwrap_or(Path::new("")),
                &target
            )),
            "../../left-pad@4.5.6/node_modules/left-pad"
        );
    }

    #[test]
    fn projection_link_policy_is_strict() {
        let mut node_modules = MaterializeOptions::node_modules();
        assert_eq!(effective_link(&node_modules), LinkStrategy::Symlink);

        node_modules.link = LinkStrategy::Copy;
        assert_eq!(effective_link(&node_modules), LinkStrategy::Symlink);

        node_modules.link = LinkStrategy::Auto;
        assert_eq!(effective_link(&node_modules), LinkStrategy::Symlink);

        let mut vendor = MaterializeOptions::vendor();
        vendor.link = LinkStrategy::Symlink;
        assert_eq!(effective_link(&vendor), LinkStrategy::Copy);
    }

    #[test]
    fn plan_tree_hash_is_stable_across_insertion_orders() {
        let mut forward = Lockfile::new();
        let mut reverse = Lockfile::new();
        let a = lock_entry("a", "1.0.0", ContentHash::of(b"a"), &[("b", "1.0.0")]);
        let b = lock_entry("b", "1.0.0", ContentHash::of(b"b"), &[]);
        forward.upsert(a.clone());
        forward.upsert(b.clone());
        reverse.upsert(b);
        reverse.upsert(a);

        let roots = roots(&[("a", "1.0.0")]);
        let graph_a = ResolutionGraph::assemble(Arc::new(forward), roots.clone()).expect("graph");
        let graph_b = ResolutionGraph::assemble(Arc::new(reverse), roots).expect("graph");
        let cache = Cache::with_root(crate::test_support::unique_tmp_dir("plan-hash-cache"));
        let root = crate::test_support::unique_tmp_dir("plan-hash-root");
        let plan_a = Materializer::new(&cache, &graph_a, &root)
            .plan(&MaterializeOptions::node_modules())
            .expect("plan a");
        let plan_b = Materializer::new(&cache, &graph_b, &root)
            .plan(&MaterializeOptions::node_modules())
            .expect("plan b");
        assert_eq!(plan_a.tree_hash(), plan_b.tree_hash());
        std::fs::remove_dir_all(cache.root()).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
