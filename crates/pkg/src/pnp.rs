use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use crate::{
    resolve_roots, Cache, ContentHash, LockError, Lockfile, PackageName, RootResolveError, Version,
    VersionReq,
};

/// The install-mode-independent, fully-pinned resolved dependency tree.
#[derive(Clone)]
pub struct ResolutionGraph {
    root_deps: BTreeMap<PackageName, Version>,
    lockfile: Arc<Lockfile>,
    reachable: BTreeSet<(PackageName, Version)>,
}

/// A borrowed view of one reachable package.
#[derive(Clone, Copy)]
pub struct ResolvedPackage<'a> {
    pub name: &'a PackageName,
    pub version: &'a Version,
    pub integrity: &'a ContentHash,
    pub dependencies: &'a BTreeMap<PackageName, Version>,
}

#[derive(thiserror::Error, Debug)]
pub enum PnpError {
    #[error(transparent)]
    Lock(#[from] LockError),
    #[error(
        "direct dependency {name} pinned to {version}, but no such entry is in meow.lock.jsonl — run `meow install`"
    )]
    RootNotLocked { name: PackageName, version: Version },
    #[error(
        "dependency {name}@{version} requires {dep}@{dep_version}, which is not in meow.lock.jsonl (dangling edge) — run `meow install`"
    )]
    DanglingEdge {
        name: PackageName,
        version: Version,
        dep: PackageName,
        dep_version: Version,
    },
    #[error("package {name}@{version} is not in the cache — run `meow install`")]
    NotCached { name: PackageName, version: Version },
    #[error(transparent)]
    Roots(#[from] RootResolveError),
}

struct PendingPackage {
    name: PackageName,
    version: Version,
    from: ReachableFrom,
}

enum ReachableFrom {
    Root,
    Edge { name: PackageName, version: Version },
}

impl ResolutionGraph {
    pub fn assemble(
        lockfile: Arc<Lockfile>,
        root_deps: BTreeMap<PackageName, Version>,
    ) -> Result<ResolutionGraph, PnpError> {
        let mut reachable = BTreeSet::new();
        let mut pending = VecDeque::new();

        for (name, version) in &root_deps {
            pending.push_back(PendingPackage {
                name: name.clone(),
                version: version.clone(),
                from: ReachableFrom::Root,
            });
        }

        while let Some(next) = pending.pop_front() {
            let key = (next.name.clone(), next.version.clone());
            if reachable.contains(&key) {
                continue;
            }

            let Some(entry) = lockfile.get(&next.name, &next.version) else {
                return Err(match next.from {
                    ReachableFrom::Root => PnpError::RootNotLocked {
                        name: next.name,
                        version: next.version,
                    },
                    ReachableFrom::Edge { name, version } => PnpError::DanglingEdge {
                        name,
                        version,
                        dep: next.name,
                        dep_version: next.version,
                    },
                });
            };

            reachable.insert(key);

            for (dep, dep_version) in &entry.dependencies {
                if reachable.contains(&(dep.clone(), dep_version.clone())) {
                    continue;
                }
                pending.push_back(PendingPackage {
                    name: dep.clone(),
                    version: dep_version.clone(),
                    from: ReachableFrom::Edge {
                        name: entry.name.clone(),
                        version: entry.version.clone(),
                    },
                });
            }
        }

        Ok(ResolutionGraph {
            root_deps,
            lockfile,
            reachable,
        })
    }

    pub fn from_project(
        project_root: &Path,
        direct: &BTreeMap<PackageName, VersionReq>,
    ) -> Result<ResolutionGraph, PnpError> {
        let lock_path = project_root.join("meow.lock.jsonl");
        let lockfile = match Lockfile::read(&lock_path) {
            Ok(lockfile) => lockfile,
            Err(LockError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                Lockfile::new()
            }
            Err(err) => return Err(err.into()),
        };
        let root_deps = resolve_roots(direct, &lockfile)?;
        ResolutionGraph::assemble(Arc::new(lockfile), root_deps)
    }

    pub fn root_deps(&self) -> &BTreeMap<PackageName, Version> {
        &self.root_deps
    }

    pub fn lockfile(&self) -> &Arc<Lockfile> {
        &self.lockfile
    }

    pub fn packages(&self) -> impl Iterator<Item = ResolvedPackage<'_>> + '_ {
        self.reachable.iter().filter_map(move |(name, version)| {
            self.lockfile
                .get(name, version)
                .map(|entry| ResolvedPackage {
                    name: &entry.name,
                    version: &entry.version,
                    integrity: &entry.integrity,
                    dependencies: &entry.dependencies,
                })
        })
    }

    pub fn verify_cached(&self, cache: &Cache) -> Result<(), PnpError> {
        for package in self.packages() {
            if !cache.contains(package.integrity) {
                return Err(PnpError::NotCached {
                    name: package.name.clone(),
                    version: package.version.clone(),
                });
            }
        }
        Ok(())
    }
}
