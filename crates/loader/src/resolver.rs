//! THE single module resolver for the whole toolchain (I-1, I-5).
//!
//! The runtime's [`crate::MeowModuleLoader`] and (later, LSP-001) the language
//! server both consume this exact type. Resolution stays centralized here:
//!
//! - [`Resolver::locate`] decides identity (`specifier` + `referrer` → URL + locator).
//! - [`Resolver::resolve`] re-enters that identity and reads the bytes.
//!
//! No `node_modules` is ever created or consulted (I-5). Cached dependencies resolve
//! from lockfile entries into members of content-addressed package archives.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use deno_core::url::Url;
use meow_pkg::{Cache, CacheError, ContentHash, Lockfile, PackageName, UnpackedStore, Version};
use meow_runtime::native::NativeModuleSource;

use crate::package::{Exports, ExportsTarget, PackageFs, PackageJson};
use crate::url as virtual_url;

// === LOAD-004 ===
/// Active import-context conditions, applied in a FIXED meow priority order.
///
/// Honest divergence from Node (I-11): Node selects the first matching key in the
/// package.json conditions object in *author insertion order*; meow instead applies
/// this fixed priority and stores condition maps in a key-sorted `BTreeMap` (author
/// order is not preserved). For an object listing both `node` and `import`, meow
/// deterministically prefers `import` regardless of the authored order — a
/// deliberate, documented choice (LOAD-003 Operator note 2), not byte-for-byte Node
/// ESM.
const IMPORT_CONDITIONS: [&str; 4] = ["meow", "import", "node", "default"];
/// The same resolver in CommonJS `require()` context: only the condition set changes.
const REQUIRE_CONDITIONS: [&str; 4] = ["meow", "require", "node", "default"];
// === /LOAD-004 ===
const EXTENSIONS: [&str; 4] = [".js", ".mjs", ".cjs", ".json"];
const INDEX_FILES: [&str; 4] = ["index.js", "index.mjs", "index.cjs", "index.json"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// === LOAD-004 ===
enum ResolveContext {
    Import,
    Require,
}

impl ResolveContext {
    fn conditions(self) -> &'static [&'static str] {
        match self {
            Self::Import => &IMPORT_CONDITIONS,
            Self::Require => &REQUIRE_CONDITIONS,
        }
    }
}
// === /LOAD-004 ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Esm,
    Json,
    Cjs,
}

#[derive(Debug, Clone)]
pub enum ModuleLocator {
    LocalFile(PathBuf),
    Cached {
        package: ContentHash,
        member: String,
    },
    // === RT-005 ===
    Native {
        name: String,
    },
    // === /RT-005 ===
}

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub url: Url,
    // === LOAD-004 ===
    pub locator: ModuleLocator,
    // === /LOAD-004 ===
    pub source: Arc<str>,
    pub kind: ModuleKind,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("could not resolve {specifier:?} from {referrer}")]
    SpecifierNotFound { specifier: String, referrer: Url },
    #[error("bare specifier {name:?} is not resolvable — run `meow install`")]
    BareSpecifierNotInLockfile { name: String },
    #[error("unsupported module URL scheme {scheme:?} in {url}")]
    UnsupportedScheme { scheme: String, url: Url },
    #[error("reading {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("module source is not valid UTF-8: {url}")]
    NotUtf8 { url: Url },
    #[error("malformed virtual cache URL: {0}")]
    InvalidVirtualUrl(Url),
    #[error("{subpath:?} is not exported by {package:?} (check its package.json \"exports\")")]
    SubpathNotExported { package: String, subpath: String },
    #[error("{subpath:?} is explicitly blocked (null) by {package:?}'s exports/imports")]
    SubpathBlocked { package: String, subpath: String },
    #[error("no export of {package:?} matched conditions {tried:?} for {subpath:?}")]
    NoMatchingCondition {
        package: String,
        subpath: String,
        tried: Vec<&'static str>,
    },
    #[error("internal import {specifier:?} is not defined in {package:?}'s \"imports\"")]
    ImportNotDefined { package: String, specifier: String },
    #[error(
        "invalid package target {target:?} in {package:?} (must be a relative \"./\" path, no \"..\" escape)"
    )]
    InvalidPackageTarget { package: String, target: String },
    #[error("malformed package.json in {package:?}: {reason}")]
    InvalidManifest { package: String, reason: String },
    #[error("malformed package archive for {package:?}: {reason}")]
    InvalidArchive { package: String, reason: String },
    // === LOAD-004 ===
    #[error("cached package {package:?} could not be projected for CommonJS paths: {reason}")]
    UnpackedPath { package: String, reason: String },
    // === /LOAD-004 ===
    // === RT-005 ===
    #[error(
        "unknown meow:{name} — available meow:* modules: {available}; fix the import or run on a meow runtime that provides it"
    )]
    UnknownNativeModule { name: String, available: String },
    // === /RT-005 ===
    #[error(transparent)]
    Cache(#[from] CacheError),
}

pub struct Resolver {
    cache: Arc<Cache>,
    lockfile: Arc<Lockfile>,
    root_deps: BTreeMap<PackageName, Version>,
    by_integrity: HashMap<ContentHash, (PackageName, Version)>,
    packages: RefCell<HashMap<ContentHash, Arc<PackageFs>>>,
    project_root: Url,
    // === RT-005 ===
    native: Arc<dyn NativeModuleSource>,
    // === /RT-005 ===
}

impl Resolver {
    pub fn new(
        cache: Arc<Cache>,
        lockfile: Arc<Lockfile>,
        root_deps: BTreeMap<PackageName, Version>,
        project_root: Url,
        // === RT-005 ===
        native: Arc<dyn NativeModuleSource>,
        // === /RT-005 ===
    ) -> Resolver {
        let by_integrity = lockfile
            .iter()
            .map(|entry| {
                (
                    entry.integrity.clone(),
                    (entry.name.clone(), entry.version.clone()),
                )
            })
            .collect();
        Resolver {
            cache,
            lockfile,
            root_deps,
            by_integrity,
            packages: RefCell::new(HashMap::new()),
            project_root,
            native,
        }
    }
    // === PKG-003 ===
    pub fn from_resolution(
        graph: &meow_pkg::ResolutionGraph,
        cache: Arc<Cache>,
        project_root: Url,
        native: Arc<dyn NativeModuleSource>,
    ) -> Resolver {
        Resolver::new(
            cache,
            graph.lockfile().clone(),
            graph.root_deps().clone(),
            project_root,
            native,
        )
    }
    // === /PKG-003 ===

    pub fn project_root(&self) -> &Url {
        &self.project_root
    }

    pub(crate) fn package_label(&self, hash: &ContentHash) -> String {
        self.by_integrity
            .get(hash)
            .map(|(name, _version)| name.to_string())
            .unwrap_or_else(|| hash.to_sri())
    }
    // === LOAD-004 ===
    pub(crate) fn runtime_path_for(
        &self,
        locator: &ModuleLocator,
    ) -> Result<PathBuf, ResolveError> {
        match locator {
            ModuleLocator::LocalFile(path) => Ok(path.clone()),
            ModuleLocator::Cached { package, member } => {
                let store =
                    UnpackedStore::new(self.cache.root().join("unpacked"), self.cache.clone());
                let root = store.ensure(package).map_err(|err| match err {
                    meow_pkg::MaterializeError::CacheBlob { source, .. } => {
                        ResolveError::Cache(source)
                    }
                    other => ResolveError::UnpackedPath {
                        package: self.package_label(package),
                        reason: other.to_string(),
                    },
                })?;
                Ok(root.join(member))
            }
            ModuleLocator::Native { name } => Ok(PathBuf::from("meow-native").join(name)),
        }
    }
    // === /LOAD-004 ===
    // === RT-005 ===
    fn native_available_list(&self, node_builtin: bool) -> String {
        if node_builtin {
            return self
                .native
                .node_builtins()
                .iter()
                .map(|module| format!("node:{module}"))
                .collect::<Vec<_>>()
                .join(", ");
        }
        self.native
            .modules()
            .iter()
            .map(|module| format!("meow:{module}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn locate_native(&self, name: &str) -> Result<(Url, ModuleLocator), ResolveError> {
        if self.native.source(name).is_none() {
            return Err(ResolveError::UnknownNativeModule {
                name: name.to_owned(),
                available: self.native_available_list(false),
            });
        }
        let url =
            Url::parse(&format!("meow:{name}")).map_err(|_| ResolveError::SpecifierNotFound {
                specifier: format!("meow:{name}"),
                referrer: self.project_root.clone(),
            })?;
        Ok((
            url,
            ModuleLocator::Native {
                name: name.to_owned(),
            },
        ))
    }
    // === /RT-005 ===
    // === RT-007 ===
    fn locate_node_builtin(&self, name: &str) -> Result<(Url, ModuleLocator), ResolveError> {
        let canonical = format!("node:{name}");
        if self.native.source(&canonical).is_none() {
            return Err(ResolveError::UnknownNativeModule {
                name: canonical,
                available: self.native_available_list(true),
            });
        }
        let url =
            Url::parse(&format!("node:{name}")).map_err(|_| ResolveError::SpecifierNotFound {
                specifier: format!("node:{name}"),
                referrer: self.project_root.clone(),
            })?;
        Ok((
            url,
            ModuleLocator::Native {
                name: format!("node:{name}"),
            },
        ))
    }
    // === /RT-007 ===

    pub fn locate(
        &self,
        specifier: &str,
        referrer: &Url,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        self.locate_with_context(specifier, referrer, ResolveContext::Import)
    }

    // === LOAD-004 ===
    pub fn locate_require(
        &self,
        specifier: &str,
        referrer: &Url,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        self.locate_with_context(specifier, referrer, ResolveContext::Require)
    }
    // === /LOAD-004 ===

    fn locate_with_context(
        &self,
        specifier: &str,
        referrer: &Url,
        context: ResolveContext,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        if let Ok(url) = Url::parse(specifier) {
            // === RT-005 ===
            if url.scheme() == "meow" {
                return self.locate_native(url.path());
            }
            // === /RT-005 ===
            // === RT-007 ===
            if url.scheme() == "node" {
                return self.locate_node_builtin(url.path());
            }
            // === /RT-007 ===
            return self.locate_url(url);
        }
        if specifier.starts_with("./") || specifier.starts_with("../") || specifier.starts_with('/')
        {
            let joined = referrer
                .join(specifier)
                .map_err(|_| ResolveError::SpecifierNotFound {
                    specifier: specifier.to_owned(),
                    referrer: referrer.clone(),
                })?;
            return self.finalize_joined(joined, specifier, referrer);
        }
        if specifier.starts_with('#') {
            return self.locate_package_import(specifier, referrer, context);
        }
        // === RT-005 ===
        if let Some(name) = specifier.strip_prefix("meow:") {
            return self.locate_native(name);
        }
        // === /RT-005 ===
        self.locate_bare(specifier, referrer, context)
    }

    pub fn resolve(&self, specifier: &str, referrer: &Url) -> Result<ResolvedModule, ResolveError> {
        self.resolve_with_context(specifier, referrer, ResolveContext::Import)
    }

    // === LOAD-004 ===
    pub fn resolve_require(
        &self,
        specifier: &str,
        referrer: &Url,
    ) -> Result<ResolvedModule, ResolveError> {
        self.resolve_with_context(specifier, referrer, ResolveContext::Require)
    }
    // === /LOAD-004 ===

    fn resolve_with_context(
        &self,
        specifier: &str,
        referrer: &Url,
        context: ResolveContext,
    ) -> Result<ResolvedModule, ResolveError> {
        let (url, locator) = self.locate_with_context(specifier, referrer, context)?;
        let kind = self.module_kind(&locator)?;
        let bytes = match &locator {
            ModuleLocator::LocalFile(path) => {
                std::fs::read(path).map_err(|source| ResolveError::Io {
                    path: path.clone(),
                    source,
                })?
            }
            ModuleLocator::Cached { package, member } => {
                let fs = self.package_fs(package)?;
                let bytes = fs
                    .read(member)
                    .ok_or_else(|| ResolveError::SpecifierNotFound {
                        specifier: url.to_string(),
                        referrer: referrer.clone(),
                    })?;
                bytes.as_ref().to_vec()
            }
            // === RT-005 ===
            ModuleLocator::Native { name } => self
                .native
                .source(name)
                .ok_or_else(|| ResolveError::UnknownNativeModule {
                    name: name.clone(),
                    available: self.native_available_list(name.starts_with("node:")),
                })?
                .as_bytes()
                .to_vec(), // === /RT-005 ===
        };
        let source =
            String::from_utf8(bytes).map_err(|_| ResolveError::NotUtf8 { url: url.clone() })?;
        Ok(ResolvedModule {
            url,
            locator,
            source: Arc::from(source),
            kind,
        })
    }

    fn module_kind(&self, locator: &ModuleLocator) -> Result<ModuleKind, ResolveError> {
        match locator {
            ModuleLocator::LocalFile(path) => local_file_kind(path),
            ModuleLocator::Cached { package, member } => {
                let fs = self.package_fs(package)?;
                Ok(cached_file_kind(member, &fs))
            }
            ModuleLocator::Native { .. } => Ok(ModuleKind::Esm),
        }
    }

    fn locate_url(&self, url: Url) -> Result<(Url, ModuleLocator), ResolveError> {
        match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| ResolveError::SpecifierNotFound {
                        specifier: url.to_string(),
                        referrer: self.project_root.clone(),
                    })?;
                Ok((url, ModuleLocator::LocalFile(path)))
            }
            virtual_url::SCHEME => {
                let (package, member) = virtual_url::decode(&url)?;
                let fs = self.package_fs(&package)?;
                if !fs.contains(&member) {
                    return Err(ResolveError::SpecifierNotFound {
                        specifier: url.to_string(),
                        referrer: self.project_root.clone(),
                    });
                }
                Ok((url, ModuleLocator::Cached { package, member }))
            }
            other => Err(ResolveError::UnsupportedScheme {
                scheme: other.to_owned(),
                url,
            }),
        }
    }

    fn finalize_joined(
        &self,
        joined: Url,
        specifier: &str,
        referrer: &Url,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        match joined.scheme() {
            "file" => self.finalize_local_url(joined, specifier, referrer),
            virtual_url::SCHEME => self.finalize_cached_url(joined, specifier, referrer),
            other => Err(ResolveError::UnsupportedScheme {
                scheme: other.to_owned(),
                url: joined,
            }),
        }
    }

    fn finalize_local_url(
        &self,
        joined: Url,
        specifier: &str,
        referrer: &Url,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        let path = joined
            .to_file_path()
            .map_err(|()| ResolveError::SpecifierNotFound {
                specifier: specifier.to_owned(),
                referrer: referrer.clone(),
            })?;
        let path = finalize_local_path(&path).ok_or_else(|| ResolveError::SpecifierNotFound {
            specifier: specifier.to_owned(),
            referrer: referrer.clone(),
        })?;
        self.local_target(path)
    }

    fn finalize_cached_url(
        &self,
        joined: Url,
        specifier: &str,
        referrer: &Url,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        let (package, member) = virtual_url::decode(&joined)?;
        let fs = self.package_fs(&package)?;
        let member = finalize_cached_member(&fs, &member).ok_or_else(|| {
            ResolveError::SpecifierNotFound {
                specifier: specifier.to_owned(),
                referrer: referrer.clone(),
            }
        })?;
        Ok((
            virtual_url::encode(&package, &member),
            ModuleLocator::Cached { package, member },
        ))
    }

    fn locate_package_import(
        &self,
        specifier: &str,
        referrer: &Url,
        context: ResolveContext,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        let owner = self.owner_for_referrer(referrer)?;
        let Some(imports) = owner
            .manifest()
            .and_then(|manifest| manifest.imports.as_ref())
        else {
            return Err(ResolveError::ImportNotDefined {
                package: owner.label(),
                specifier: specifier.to_owned(),
            });
        };
        if let Some(target) = imports.get(specifier) {
            return self.target_resolution_to_locator(
                self.package_target_resolve(&owner, target, None, specifier, context)?,
            );
        }
        if let Some((target, capture)) = longest_pattern_match(imports, specifier) {
            return self.target_resolution_to_locator(self.package_target_resolve(
                &owner,
                target,
                Some(capture),
                specifier,
                context,
            )?);
        }
        Err(ResolveError::ImportNotDefined {
            package: owner.label(),
            specifier: specifier.to_owned(),
        })
    }

    fn locate_bare(
        &self,
        specifier: &str,
        referrer: &Url,
        context: ResolveContext,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        let owner = self.owner_for_referrer(referrer)?;
        let (package_name_text, subpath) = parse_package_specifier(specifier);

        if owner.package_name() == Some(package_name_text) {
            let Some(manifest) = owner.manifest() else {
                return Err(ResolveError::SubpathNotExported {
                    package: package_name_text.to_owned(),
                    subpath: subpath_for_exports(subpath),
                });
            };
            let Some(exports) = manifest.exports.as_ref() else {
                return Err(ResolveError::SubpathNotExported {
                    package: package_name_text.to_owned(),
                    subpath: subpath_for_exports(subpath),
                });
            };
            return self.target_resolution_to_locator(self.package_exports_resolve(
                &owner,
                exports,
                &subpath_for_exports(subpath),
                context,
            )?);
        }

        let dep_name = PackageName::new(package_name_text.to_owned());
        let version = match self.dependency_version(&owner, &dep_name) {
            Some(version) => version,
            None => {
                // === RT-007 ===
                if self.native.node_builtins().contains(&specifier) {
                    return self.locate_node_builtin(specifier);
                }
                // === /RT-007 ===
                return Err(ResolveError::BareSpecifierNotInLockfile {
                    name: package_name_text.to_owned(),
                });
            }
        };
        let entry = self.lockfile.get(&dep_name, &version).ok_or_else(|| {
            ResolveError::BareSpecifierNotInLockfile {
                name: package_name_text.to_owned(),
            }
        })?;
        let package = entry.integrity.clone();
        let fs = self.package_fs(&package)?;
        let dep_owner = OwnerPackage::Cached {
            package: package.clone(),
            name: dep_name.clone(),
            version,
            fs,
        };

        let resolution =
            if let Some(exports) = dep_owner.manifest().and_then(|m| m.exports.as_ref()) {
                self.package_exports_resolve(
                    &dep_owner,
                    exports,
                    &subpath_for_exports(subpath),
                    context,
                )?
            } else {
                self.legacy_package_resolve(&dep_owner, subpath)?
            };
        self.target_resolution_to_locator(resolution)
    }

    fn owner_for_referrer(&self, referrer: &Url) -> Result<OwnerPackage, ResolveError> {
        match referrer.scheme() {
            "file" => {
                let root = self.project_root_path().unwrap_or_default();
                let manifest = self.project_manifest()?;
                Ok(OwnerPackage::Root { root, manifest })
            }
            virtual_url::SCHEME => {
                let (package, _member) = virtual_url::decode(referrer)?;
                let (name, version) =
                    self.by_integrity.get(&package).cloned().ok_or_else(|| {
                        ResolveError::SpecifierNotFound {
                            specifier: referrer.to_string(),
                            referrer: referrer.clone(),
                        }
                    })?;
                let fs = self.package_fs(&package)?;
                Ok(OwnerPackage::Cached {
                    package,
                    name,
                    version,
                    fs,
                })
            }
            other => Err(ResolveError::UnsupportedScheme {
                scheme: other.to_owned(),
                url: referrer.clone(),
            }),
        }
    }

    fn project_root_path(&self) -> Option<PathBuf> {
        self.project_root.to_file_path().ok()
    }

    fn project_manifest(&self) -> Result<Option<PackageJson>, ResolveError> {
        let Some(root) = self.project_root_path() else {
            return Ok(None);
        };
        let path = root.join("package.json");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ResolveError::Io { path, source }),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| ResolveError::InvalidManifest {
                package: root.display().to_string(),
                reason: err.to_string(),
            })
    }

    fn dependency_version(&self, owner: &OwnerPackage, dep_name: &PackageName) -> Option<Version> {
        match owner {
            OwnerPackage::Root { .. } => self.root_deps.get(dep_name).cloned(),
            OwnerPackage::Cached { name, version, .. } => self
                .lockfile
                .get(name, version)
                .and_then(|entry| entry.dependencies.get(dep_name))
                .cloned(),
        }
    }

    fn package_fs(&self, integrity: &ContentHash) -> Result<Arc<PackageFs>, ResolveError> {
        if let Some(existing) = self.packages.borrow().get(integrity) {
            return Ok(existing.clone());
        }
        let bytes = self.cache.read(integrity)?;
        let fs = PackageFs::from_archive(&bytes)
            .map_err(|err| self.relabel_package_error(integrity, err))?;
        let fs = Arc::new(fs);
        self.packages
            .borrow_mut()
            .insert(integrity.clone(), fs.clone());
        Ok(fs)
    }

    fn relabel_package_error(&self, integrity: &ContentHash, err: ResolveError) -> ResolveError {
        let label = self.package_label(integrity);
        match err {
            ResolveError::InvalidManifest { reason, .. } => ResolveError::InvalidManifest {
                package: label,
                reason,
            },
            ResolveError::InvalidArchive { reason, .. } => ResolveError::InvalidArchive {
                package: label,
                reason,
            },
            other => other,
        }
    }

    fn target_resolution_to_locator(
        &self,
        resolution: TargetResolution,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        match resolution {
            TargetResolution::LocalFile(path) => self.local_target(path),
            TargetResolution::Cached { package, member } => Ok((
                virtual_url::encode(&package, &member),
                ModuleLocator::Cached { package, member },
            )),
        }
    }

    fn local_target(&self, path: PathBuf) -> Result<(Url, ModuleLocator), ResolveError> {
        let url = Url::from_file_path(&path).map_err(|()| ResolveError::SpecifierNotFound {
            specifier: path.display().to_string(),
            referrer: self.project_root.clone(),
        })?;
        Ok((url, ModuleLocator::LocalFile(path)))
    }

    fn package_exports_resolve(
        &self,
        owner: &OwnerPackage,
        exports: &Exports,
        subpath: &str,
        context: ResolveContext,
    ) -> Result<TargetResolution, ResolveError> {
        match exports {
            Exports::Single(ExportsTarget::Conditions(map)) if is_conditions_map(map) => {
                if subpath != "." {
                    return Err(ResolveError::SubpathNotExported {
                        package: owner.label(),
                        subpath: subpath.to_owned(),
                    });
                }
                self.condition_target_resolve(owner, map, None, subpath, context)
            }
            Exports::Single(ExportsTarget::Conditions(map)) => {
                if let Some(target) = map.get(subpath) {
                    return self.package_target_resolve(owner, target, None, subpath, context);
                }
                if let Some((target, capture)) = longest_pattern_match(map, subpath) {
                    return self.package_target_resolve(
                        owner,
                        target,
                        Some(capture),
                        subpath,
                        context,
                    );
                }
                Err(ResolveError::SubpathNotExported {
                    package: owner.label(),
                    subpath: subpath.to_owned(),
                })
            }
            Exports::Single(target) => {
                if subpath != "." {
                    return Err(ResolveError::SubpathNotExported {
                        package: owner.label(),
                        subpath: subpath.to_owned(),
                    });
                }
                self.package_target_resolve(owner, target, None, subpath, context)
            }
            Exports::Map(map) if is_conditions_map(map) => {
                if subpath != "." {
                    return Err(ResolveError::SubpathNotExported {
                        package: owner.label(),
                        subpath: subpath.to_owned(),
                    });
                }
                self.condition_target_resolve(owner, map, None, subpath, context)
            }
            Exports::Map(map) => {
                if let Some(target) = map.get(subpath) {
                    return self.package_target_resolve(owner, target, None, subpath, context);
                }
                if let Some((target, capture)) = longest_pattern_match(map, subpath) {
                    return self.package_target_resolve(
                        owner,
                        target,
                        Some(capture),
                        subpath,
                        context,
                    );
                }
                Err(ResolveError::SubpathNotExported {
                    package: owner.label(),
                    subpath: subpath.to_owned(),
                })
            }
        }
    }

    fn condition_target_resolve(
        &self,
        owner: &OwnerPackage,
        map: &BTreeMap<String, ExportsTarget>,
        capture: Option<&str>,
        subpath: &str,
        context: ResolveContext,
    ) -> Result<TargetResolution, ResolveError> {
        for &condition in context.conditions() {
            if let Some(target) = map.get(condition) {
                return self.package_target_resolve(owner, target, capture, subpath, context);
            }
        }
        Err(ResolveError::NoMatchingCondition {
            package: owner.label(),
            subpath: subpath.to_owned(),
            tried: context.conditions().to_vec(),
        })
    }

    fn package_target_resolve(
        &self,
        owner: &OwnerPackage,
        target: &ExportsTarget,
        capture: Option<&str>,
        subpath: &str,
        context: ResolveContext,
    ) -> Result<TargetResolution, ResolveError> {
        match target {
            ExportsTarget::Path(path) => {
                let member = normalize_target_member(&owner.label(), path, capture)?;
                match owner {
                    OwnerPackage::Root { root, .. } => {
                        let path = root.join(&member);
                        if !path.is_file() {
                            return Err(ResolveError::SpecifierNotFound {
                                specifier: member,
                                referrer: owner.referrer_url(&self.project_root),
                            });
                        }
                        Ok(TargetResolution::LocalFile(path))
                    }
                    OwnerPackage::Cached { package, fs, .. } => {
                        if !fs.contains(&member) {
                            return Err(ResolveError::SpecifierNotFound {
                                specifier: member,
                                referrer: owner.referrer_url(&self.project_root),
                            });
                        }
                        Ok(TargetResolution::Cached {
                            package: package.clone(),
                            member,
                        })
                    }
                }
            }
            ExportsTarget::Conditions(map) => {
                self.condition_target_resolve(owner, map, capture, subpath, context)
            }
            ExportsTarget::Fallback(list) => {
                let mut last = None;
                for item in list {
                    match self.package_target_resolve(owner, item, capture, subpath, context) {
                        Ok(resolution) => return Ok(resolution),
                        Err(err) => last = Some(err),
                    }
                }
                Err(last.unwrap_or_else(|| ResolveError::NoMatchingCondition {
                    package: owner.label(),
                    subpath: subpath.to_owned(),
                    tried: context.conditions().to_vec(),
                }))
            }
            ExportsTarget::Blocked => Err(ResolveError::SubpathBlocked {
                package: owner.label(),
                subpath: subpath.to_owned(),
            }),
        }
    }

    fn legacy_package_resolve(
        &self,
        owner: &OwnerPackage,
        subpath: Option<&str>,
    ) -> Result<TargetResolution, ResolveError> {
        let OwnerPackage::Cached { package, fs, .. } = owner else {
            return Err(ResolveError::SubpathNotExported {
                package: owner.label(),
                subpath: subpath_for_exports(subpath),
            });
        };

        if let Some(subpath) = subpath {
            let member = normalize_legacy_member(subpath).ok_or_else(|| {
                ResolveError::SpecifierNotFound {
                    specifier: subpath.to_owned(),
                    referrer: owner.referrer_url(&self.project_root),
                }
            })?;
            let member = finalize_cached_member(fs, &member).ok_or_else(|| {
                ResolveError::SpecifierNotFound {
                    specifier: subpath.to_owned(),
                    referrer: owner.referrer_url(&self.project_root),
                }
            })?;
            return Ok(TargetResolution::Cached {
                package: package.clone(),
                member,
            });
        }

        if let Some(main) = owner
            .manifest()
            .and_then(|manifest| manifest.main.as_deref())
            .and_then(normalize_legacy_member)
            .and_then(|member| finalize_cached_member(fs, &member))
        {
            return Ok(TargetResolution::Cached {
                package: package.clone(),
                member: main,
            });
        }

        let member =
            cached_directory_index(fs, "").ok_or_else(|| ResolveError::SpecifierNotFound {
                specifier: owner.label(),
                referrer: owner.referrer_url(&self.project_root),
            })?;
        Ok(TargetResolution::Cached {
            package: package.clone(),
            member,
        })
    }
}

enum OwnerPackage {
    Root {
        root: PathBuf,
        manifest: Option<PackageJson>,
    },
    Cached {
        package: ContentHash,
        name: PackageName,
        version: Version,
        fs: Arc<PackageFs>,
    },
}

impl OwnerPackage {
    fn label(&self) -> String {
        match self {
            OwnerPackage::Root { manifest, root } => manifest
                .as_ref()
                .and_then(|pkg| pkg.name.clone())
                .unwrap_or_else(|| root.display().to_string()),
            OwnerPackage::Cached { name, .. } => name.to_string(),
        }
    }

    fn package_name(&self) -> Option<&str> {
        match self {
            OwnerPackage::Root { manifest, .. } => {
                manifest.as_ref().and_then(|pkg| pkg.name.as_deref())
            }
            OwnerPackage::Cached { name, .. } => Some(name.as_str()),
        }
    }

    fn manifest(&self) -> Option<&PackageJson> {
        match self {
            OwnerPackage::Root { manifest, .. } => manifest.as_ref(),
            OwnerPackage::Cached { fs, .. } => Some(fs.manifest()),
        }
    }

    fn referrer_url(&self, project_root: &Url) -> Url {
        match self {
            OwnerPackage::Root { root, .. } => Url::from_file_path(root.join("package.json"))
                .ok()
                .unwrap_or_else(|| project_root.clone()),
            OwnerPackage::Cached { package, .. } => virtual_url::encode(package, "package.json"),
        }
    }
}

enum TargetResolution {
    LocalFile(PathBuf),
    Cached {
        package: ContentHash,
        member: String,
    },
}

fn parse_package_specifier(specifier: &str) -> (&str, Option<&str>) {
    let name = package_name(specifier);
    if specifier.len() == name.len() {
        (name, None)
    } else {
        (name, Some(&specifier[name.len() + 1..]))
    }
}

fn subpath_for_exports(subpath: Option<&str>) -> String {
    match subpath {
        Some(subpath) => format!("./{subpath}"),
        None => ".".to_owned(),
    }
}

fn longest_pattern_match<'a>(
    map: &'a BTreeMap<String, ExportsTarget>,
    specifier: &'a str,
) -> Option<(&'a ExportsTarget, &'a str)> {
    let mut best: Option<(&ExportsTarget, &str, usize)> = None;
    for (key, target) in map {
        let Some(capture) = match_subpath_pattern(key, specifier) else {
            continue;
        };
        let score = key.len();
        if best
            .map(|(_, _, best_score)| score > best_score)
            .unwrap_or(true)
        {
            best = Some((target, capture, score));
        }
    }
    best.map(|(target, capture, _)| (target, capture))
}

fn match_subpath_pattern<'a>(pattern: &str, specifier: &'a str) -> Option<&'a str> {
    let star = pattern.find('*')?;
    let prefix = &pattern[..star];
    let suffix = &pattern[star + 1..];
    if !specifier.starts_with(prefix) || !specifier.ends_with(suffix) {
        return None;
    }
    let capture_end = specifier.len().checked_sub(suffix.len())?;
    if capture_end < prefix.len() {
        return None;
    }
    Some(&specifier[prefix.len()..capture_end])
}

fn is_conditions_map(map: &BTreeMap<String, ExportsTarget>) -> bool {
    !map.is_empty() && map.keys().all(|key| !key.starts_with('.'))
}

fn normalize_target_member(
    package: &str,
    target: &str,
    capture: Option<&str>,
) -> Result<String, ResolveError> {
    let expanded = match capture {
        Some(capture) => {
            if target.contains('*') {
                target.replace('*', capture)
            } else {
                target.to_owned()
            }
        }
        None if target.contains('*') => {
            return Err(ResolveError::InvalidPackageTarget {
                package: package.to_owned(),
                target: target.to_owned(),
            });
        }
        None => target.to_owned(),
    };
    let stripped =
        expanded
            .strip_prefix("./")
            .ok_or_else(|| ResolveError::InvalidPackageTarget {
                package: package.to_owned(),
                target: target.to_owned(),
            })?;
    normalize_member_path(package, target, stripped)
}

fn normalize_legacy_member(member: &str) -> Option<String> {
    let stripped = member
        .strip_prefix("./")
        .unwrap_or(member)
        .trim_start_matches('/');
    if stripped.is_empty() {
        return None;
    }
    normalize_member_path("<legacy>", member, stripped).ok()
}

fn normalize_member_path(package: &str, target: &str, raw: &str) -> Result<String, ResolveError> {
    let mut member = String::new();
    for component in Path::new(raw).components() {
        let Component::Normal(segment) = component else {
            return Err(ResolveError::InvalidPackageTarget {
                package: package.to_owned(),
                target: target.to_owned(),
            });
        };
        let segment = segment
            .to_str()
            .ok_or_else(|| ResolveError::InvalidPackageTarget {
                package: package.to_owned(),
                target: target.to_owned(),
            })?;
        if !member.is_empty() {
            member.push('/');
        }
        member.push_str(segment);
    }
    if member.is_empty() {
        return Err(ResolveError::InvalidPackageTarget {
            package: package.to_owned(),
            target: target.to_owned(),
        });
    }
    Ok(member)
}

fn finalize_local_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    for suffix in EXTENSIONS {
        let candidate = append_path_suffix(path, suffix);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    local_directory_index(path)
}

fn local_directory_index(path: &Path) -> Option<PathBuf> {
    for index in INDEX_FILES {
        let candidate = path.join(index);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn finalize_cached_member(fs: &PackageFs, member: &str) -> Option<String> {
    let member = member.trim_start_matches('/');
    if member.is_empty() {
        return cached_directory_index(fs, "");
    }
    if fs.contains(member) {
        return Some(member.to_owned());
    }
    for suffix in EXTENSIONS {
        let candidate = format!("{member}{suffix}");
        if fs.contains(&candidate) {
            return Some(candidate);
        }
    }
    cached_directory_index(fs, member)
}

fn cached_directory_index(fs: &PackageFs, member: &str) -> Option<String> {
    for index in INDEX_FILES {
        let candidate = if member.is_empty() {
            index.to_owned()
        } else {
            format!("{member}/{index}")
        };
        if fs.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn append_path_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

fn cached_file_kind(member: &str, fs: &PackageFs) -> ModuleKind {
    match Path::new(member).extension().and_then(|ext| ext.to_str()) {
        Some("mjs") => ModuleKind::Esm,
        Some("cjs") => ModuleKind::Cjs,
        Some("json") => ModuleKind::Json,
        Some("js") => {
            if fs.nearest_manifest(member).package_type.as_deref() == Some("module") {
                ModuleKind::Esm
            } else {
                ModuleKind::Cjs
            }
        }
        _ => ModuleKind::Esm,
    }
}

// === LOAD-004 ===
fn local_file_kind(path: &Path) -> Result<ModuleKind, ResolveError> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => Ok(ModuleKind::Json),
        Some("mjs") | Some("mts") => Ok(ModuleKind::Esm),
        Some("cjs") | Some("cts") => Ok(ModuleKind::Cjs),
        Some("js") => {
            if nearest_local_package_type(path)?.as_deref() == Some("module") {
                Ok(ModuleKind::Esm)
            } else {
                Ok(ModuleKind::Cjs)
            }
        }
        _ => Ok(ModuleKind::Esm),
    }
}

fn nearest_local_package_type(path: &Path) -> Result<Option<String>, ResolveError> {
    let mut current = path.parent();
    while let Some(dir) = current {
        let manifest_path = dir.join("package.json");
        match std::fs::read(&manifest_path) {
            Ok(bytes) => {
                let manifest: PackageJson = serde_json::from_slice(&bytes).map_err(|err| {
                    ResolveError::InvalidManifest {
                        package: manifest_path.display().to_string(),
                        reason: err.to_string(),
                    }
                })?;
                return Ok(manifest.package_type);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                current = dir.parent();
            }
            Err(source) => {
                return Err(ResolveError::Io {
                    path: manifest_path,
                    source,
                });
            }
        }
    }
    Ok(None)
}
// === /LOAD-004 ===

fn package_name(specifier: &str) -> &str {
    if let Some(rest) = specifier.strip_prefix('@') {
        let mut slashes = rest.match_indices('/');
        slashes.next();
        match slashes.next() {
            Some((idx, _)) => &specifier[..idx + 1],
            None => specifier,
        }
    } else {
        match specifier.find('/') {
            Some(idx) => &specifier[..idx],
            None => specifier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "meow-loader-resolver-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn resolver(project_root: &Path) -> Resolver {
        Resolver::new(
            Arc::new(Cache::with_root(project_root.join("cache"))),
            Arc::new(Lockfile::new()),
            BTreeMap::new(),
            Url::from_directory_path(project_root).expect("project root URL"),
            meow_runtime::native::native_module_registry(),
        )
    }

    #[test]
    fn resolves_relative_local_file_with_extension_probe() {
        let dir = unique_dir("relative");
        std::fs::write(dir.join("a.js"), "export const a = 1;\n").expect("write module");
        let resolver = resolver(&dir);
        let referrer = Url::from_file_path(dir.join("main.ts")).expect("referrer URL");
        let (url, locator) = resolver
            .locate("./a", &referrer)
            .expect("relative resolves");
        assert_eq!(
            url.as_str(),
            Url::from_file_path(dir.join("a.js")).unwrap().as_str()
        );
        assert!(matches!(locator, ModuleLocator::LocalFile(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn first_party_cjs_locates_and_resolves_as_cjs() {
        let dir = unique_dir("cjs");
        std::fs::write(dir.join("legacy.cjs"), "module.exports = 1;\n").expect("write module");
        let resolver = resolver(&dir);
        let referrer = Url::from_file_path(dir.join("main.mjs")).expect("referrer URL");
        let (url, locator) = resolver
            .locate("./legacy.cjs", &referrer)
            .expect(".cjs resolves");
        assert_eq!(url, Url::from_file_path(dir.join("legacy.cjs")).unwrap());
        assert!(matches!(locator, ModuleLocator::LocalFile(_)));
        let resolved = resolver
            .resolve("./legacy.cjs", &referrer)
            .expect(".cjs resolves");
        assert_eq!(resolved.kind, ModuleKind::Cjs);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn local_js_defaults_to_commonjs_without_type_module() {
        let dir = unique_dir("commonjs-js");
        std::fs::write(dir.join("legacy.js"), "module.exports = 1;\n").expect("write module");
        let resolver = resolver(&dir);
        let referrer = Url::from_file_path(dir.join("main.mjs")).expect("referrer URL");
        let resolved = resolver
            .resolve("./legacy.js", &referrer)
            .expect(".js resolves");
        assert_eq!(resolved.kind, ModuleKind::Cjs);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scoped_package_name_extraction() {
        assert_eq!(package_name("@scope/pkg"), "@scope/pkg");
        assert_eq!(package_name("@scope/pkg/sub"), "@scope/pkg");
        assert_eq!(package_name("lodash"), "lodash");
        assert_eq!(package_name("lodash/fp"), "lodash");
    }
}
