//! Deterministic dependency resolution + cache population (PKG-002).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use base64::Engine as _;
use sha2::{Digest, Sha512};

use crate::registry::{sha512_sri, DepSpec, PackageMetadata, RegistryError, RegistrySource};
use crate::{
    Cache, CacheError, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
};

/// Resolve a project's declared dependencies into a complete pinned graph.
pub struct Installer<'a> {
    source: &'a dyn RegistrySource,
    cache: &'a Cache,
    registry_url: String,
    meow_req: VersionReq,
}

impl<'a> Installer<'a> {
    pub fn new(
        source: &'a dyn RegistrySource,
        cache: &'a Cache,
        registry_url: impl Into<String>,
        meow_req: VersionReq,
    ) -> Installer<'a> {
        Installer {
            source,
            cache,
            registry_url: registry_url.into(),
            meow_req,
        }
    }

    /// Resolve, verify, cache, and pin the complete dependency graph.
    pub fn resolve(
        &self,
        direct: &BTreeMap<PackageName, DepSpec>,
    ) -> Result<Lockfile, InstallError> {
        let mut metadata = BTreeMap::new();
        let mut queue = VecDeque::new();

        for (name, spec) in direct {
            let meta = metadata_for(self.source, &mut metadata, name)?;
            let version = select_version(name, &meta, spec)?;
            queue.push_back((name.clone(), version));
        }

        let mut done = BTreeSet::new();
        let mut lockfile = Lockfile::new();

        while let Some((name, version)) = queue.pop_front() {
            if done.contains(&(name.clone(), version.clone())) {
                continue;
            }

            let meta = metadata_for(self.source, &mut metadata, &name)?;
            let version_meta =
                meta.versions
                    .get(&version)
                    .ok_or_else(|| InstallError::MissingVersion {
                        name: name.to_string(),
                        version: version.to_string(),
                    })?;

            let mut dependencies = BTreeMap::new();
            for (dep, raw_req) in &version_meta.dependencies {
                let dep_meta = metadata_for(self.source, &mut metadata, dep)?;
                let dep_version = select_version(dep, &dep_meta, &DepSpec::parse(raw_req))?;
                dependencies.insert(dep.clone(), dep_version.clone());
                queue.push_back((dep.clone(), dep_version));
            }

            let bytes = self.source.fetch_tarball(&version_meta.dist.tarball)?;
            verify_npm_integrity(&name, &version, &version_meta.dist.integrity, &bytes)?;
            let integrity = self.cache.store(&bytes)?;
            lockfile.upsert(LockEntry {
                name: name.clone(),
                version: version.clone(),
                integrity,
                dependencies,
                registry: RegistryProvenance::new(&self.registry_url),
                capabilities: vec![],
                wasm: vec![],
                meow: self.meow_req.clone(),
            });
            done.insert((name, version));
        }

        Ok(lockfile)
    }
}

/// Typed install failures. No network / metadata / integrity path panics.
#[derive(thiserror::Error, Debug)]
pub enum PkgInstallError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("no published version of {name} satisfies {req}")]
    NoMatchingVersion { name: String, req: String },
    #[error(
        "unsupported requirement {req:?} for {name}: meow resolves the `semver` crate's range grammar + dist-tags; node-semver `||`/hyphen/x-range forms are not yet supported"
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

pub type InstallError = PkgInstallError;

/// Re-derive root dependency pins for the runtime from the declared config + lockfile.
pub fn resolve_roots(
    declared: &BTreeMap<PackageName, VersionReq>,
    lockfile: &Lockfile,
) -> Result<BTreeMap<PackageName, Version>, RootResolveError> {
    let mut roots = BTreeMap::new();

    for (name, req) in declared {
        let semver_req = semver::VersionReq::parse(req.as_str()).map_err(|_| {
            RootResolveError::UnsupportedRange {
                name: name.to_string(),
                req: req.to_string(),
            }
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
            if !semver_req.matches(&version) {
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

fn metadata_for(
    source: &dyn RegistrySource,
    memo: &mut BTreeMap<PackageName, PackageMetadata>,
    name: &PackageName,
) -> Result<PackageMetadata, InstallError> {
    if let Some(existing) = memo.get(name) {
        return Ok(existing.clone());
    }
    let fetched = source.fetch_metadata(name)?;
    memo.insert(name.clone(), fetched.clone());
    Ok(fetched)
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
    }
}

fn select_version_for_range(
    name: &PackageName,
    meta: &PackageMetadata,
    req: &VersionReq,
) -> Result<Version, InstallError> {
    let semver_req =
        semver::VersionReq::parse(req.as_str()).map_err(|_| InstallError::UnsupportedRange {
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
        if !semver_req.matches(&candidate) {
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
