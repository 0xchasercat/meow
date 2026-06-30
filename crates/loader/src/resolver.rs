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

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use deno_core::url::Url;
use meow_pkg::{Cache, CacheError, ContentHash, Lockfile, PackageName, UnpackedStore, Version};
use meow_runtime::native::NativeModuleSource;
use node_resolver::IsBuiltInNodeModuleChecker;

use crate::package::{Exports, ExportsTarget, PackageJson};

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

#[derive(Clone)]
pub struct Resolver {
    cache: Arc<Cache>,
    lockfile: Arc<Lockfile>,
    root_deps: BTreeMap<PackageName, Version>,
    by_integrity: HashMap<ContentHash, (PackageName, Version)>,
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
    pub fn runtime_path_for(&self, locator: &ModuleLocator) -> Result<PathBuf, ResolveError> {
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
            ModuleLocator::Native { name } => {
                let (base, member) = if let Some(member) = name.strip_prefix("node:") {
                    ("node-native", member)
                } else {
                    ("meow-native", name.as_str())
                };
                let member = member.trim_start_matches('/');
                let member = if member.is_empty() { "index" } else { member };
                Ok(PathBuf::from(base).join(format!("{member}.ts")))
            }
        }
    }

    pub fn package_root_for_require(
        &self,
        specifier: &str,
        referrer: &Url,
    ) -> Result<PathBuf, ResolveError> {
        let (package_name_text, _) = parse_package_specifier(specifier);
        let owner = self.owner_for_referrer(referrer)?;
        if owner.package_name() == Some(package_name_text) {
            return Ok(self.owner_projected_root(&owner));
        }
        let dep_name = PackageName::new(package_name_text.to_owned());
        let Some(version) = self.dependency_version(&owner, &dep_name) else {
            return Err(ResolveError::BareSpecifierNotInLockfile {
                name: package_name_text.to_owned(),
            });
        };
        let dep_owner = self.cached_dependency_owner(&dep_name, &version)?;
        Ok(self.owner_projected_root(&dep_owner))
    }

    fn owner_projected_root(&self, owner: &OwnerPackage) -> PathBuf {
        let OwnerPackage::Cached { package, root, .. } = owner else {
            return self.project_root_path().unwrap_or_default();
        };
        let package_json_locator = ModuleLocator::Cached {
            package: package.clone(),
            member: "package.json".to_owned(),
        };
        self.projected_path_for(&package_json_locator)
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| root.clone())
    }

    pub fn projected_path_for(&self, locator: &ModuleLocator) -> Option<PathBuf> {
        let ModuleLocator::Cached { package, member } = locator else {
            return None;
        };
        let (name, version) = self.by_integrity.get(package)?;
        let root = self.project_root.to_file_path().ok()?;

        let key = format!("{}@{}", name.as_str().replace('/', "+"), version);
        let strict_path = root
            .join("node_modules")
            .join(".meow")
            .join(key)
            .join("node_modules")
            .join(name.as_str())
            .join(member);

        if strict_path.exists() {
            return Some(strict_path);
        }

        let hoisted_path = root
            .join("node_modules")
            .join(name.to_string())
            .join(member);
        hoisted_path.exists().then_some(hoisted_path)
    }

    /// The on-disk unpacked-store directory for a cached package, forcing an
    /// on-demand unpack. This is the canonical real root every cached member
    /// path and `file://` URL is built from.
    fn cached_root(&self, package: &ContentHash) -> Result<PathBuf, ResolveError> {
        let store = UnpackedStore::new(self.cache.root().join("unpacked"), self.cache.clone());
        store.ensure(package).map_err(|err| match err {
            meow_pkg::MaterializeError::CacheBlob { source, .. } => ResolveError::Cache(source),
            meow_pkg::MaterializeError::Cache { source, .. } => ResolveError::Cache(source),
            meow_pkg::MaterializeError::InvalidArchive { reason, .. } => {
                ResolveError::InvalidArchive {
                    package: self.package_label(package),
                    reason,
                }
            }
            other => ResolveError::UnpackedPath {
                package: self.package_label(package),
                reason: other.to_string(),
            },
        })
    }

    /// The single canonical `file://` identity for a resolved module. Cached
    /// members map to their REAL absolute path in the unpacked store (never the
    /// removed virtual cache schemes), so ESM `locate`/`resolve`, the CJS
    /// require path, and owner referrers all agree on one URL per member.
    pub fn cached_url(&self, locator: &ModuleLocator) -> Result<Url, ResolveError> {
        let path = match locator {
            ModuleLocator::Cached { .. } => self
                .projected_path_for(locator)
                .map_or_else(|| self.runtime_path_for(locator), Ok)?,
            _ => self.runtime_path_for(locator)?,
        };
        Url::from_file_path(&path).map_err(|()| ResolveError::SpecifierNotFound {
            specifier: path.display().to_string(),
            referrer: self.project_root.clone(),
        })
    }

    /// If `path` lives inside the unpacked store or the package manager's projected
    /// `node_modules` tree, recover its `(package, member)` so a real `file://` path
    /// re-enters cached resolution as a [`ModuleLocator::Cached`] and its owning
    /// package is recognized (transitive deps and package-private imports included).
    fn cached_locator_for_path(&self, path: &Path) -> Option<(ContentHash, String)> {
        self.unpacked_locator_for_path(path)
            .or_else(|| self.projected_locator_for_path(path))
    }

    fn unpacked_locator_for_path(&self, path: &Path) -> Option<(ContentHash, String)> {
        let unpacked_root = self.cache.root().join("unpacked");
        let rel = strip_store_prefix(path, &unpacked_root)?;
        let mut components = rel.components();
        let host = components.next()?.as_os_str().to_str()?;
        let package = ContentHash::from_url_host(host).ok()?;
        if !self.by_integrity.contains_key(&package) {
            return None;
        }
        let member = components.as_path().to_string_lossy().replace('\\', "/");
        Some((package, member))
    }

    fn projected_locator_for_path(&self, path: &Path) -> Option<(ContentHash, String)> {
        let root = self.project_root.to_file_path().ok()?;
        let rel = path.strip_prefix(root.join("node_modules")).ok()?;
        let components: Vec<_> = rel.components().collect();
        let store_index = components
            .iter()
            .position(|component| component.as_os_str() == ".meow")?;
        let key = components.get(store_index + 1)?.as_os_str().to_str()?;
        if components.get(store_index + 2)?.as_os_str() != "node_modules" {
            return None;
        }
        let package_index = store_index + 3;
        let member_start = package_member_start(&components, package_index)?;
        let member = components[member_start..]
            .iter()
            .collect::<PathBuf>()
            .to_string_lossy()
            .replace('\\', "/");
        let (encoded_name, version_text) = key.rsplit_once('@')?;
        let package_name = PackageName::new(encoded_name.replace('+', "/"));
        let version = Version::parse(version_text).ok()?;
        let entry = self.lockfile.get(&package_name, &version)?;
        Some((entry.integrity.clone(), member))
    }
    // === /LOAD-004 ===
    // === RT-005 ===
    // === RT-007 ===
    /// Expose the runtime's canonical `node:*` built-in name set to the rest of
    /// the loader (ops registration, etc.). Keeping this on `Resolver` avoids having
    /// the runtime bridge reach into the `NativeModuleSource` trait directly.
    pub fn node_builtins(&self) -> &'static [&'static str] {
        self.native.node_builtins()
    }
    // === /RT-007 ===

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
        if !node_resolver::DenoIsBuiltInNodeModuleChecker.is_builtin_node_module(name) {
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
            if url.scheme() == "ext" {
                return Ok((
                    url.clone(),
                    ModuleLocator::Native {
                        name: specifier.to_owned(),
                    },
                ));
            }
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
        if specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/')
            || specifier.starts_with("\\\\?\\")
            || (specifier.len() >= 3
                && specifier.as_bytes()[0].is_ascii_alphabetic()
                && specifier.as_bytes()[1] == b':'
                && (specifier.as_bytes()[2] == b'/' || specifier.as_bytes()[2] == b'\\'))
        {
            // Convert Windows paths to file:// URLs for proper resolution
            let file_url = if let Some(stripped) = specifier.strip_prefix("\\\\?\\") {
                // Strip extended-length prefix and convert to file URL
                let normalized = stripped.replace('\\', "/");
                Url::parse(&format!("file:///{normalized}")).map_err(|_| {
                    ResolveError::SpecifierNotFound {
                        specifier: specifier.to_owned(),
                        referrer: referrer.clone(),
                    }
                })?
            } else if specifier.len() >= 3
                && specifier.as_bytes()[0].is_ascii_alphabetic()
                && specifier.as_bytes()[1] == b':'
            {
                // Windows drive letter path - convert to file URL
                let normalized = specifier.replace('\\', "/");
                Url::parse(&format!("file:///{normalized}")).map_err(|_| {
                    ResolveError::SpecifierNotFound {
                        specifier: specifier.to_owned(),
                        referrer: referrer.clone(),
                    }
                })?
            } else {
                referrer
                    .join(specifier)
                    .map_err(|_| ResolveError::SpecifierNotFound {
                        specifier: specifier.to_owned(),
                        referrer: referrer.clone(),
                    })?
            };
            return self.finalize_joined(file_url, specifier, referrer);
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
            ModuleLocator::Cached { .. } => {
                let path = self
                    .projected_path_for(&locator)
                    .unwrap_or_else(|| self.runtime_path_for(&locator).unwrap());
                std::fs::read(&path).map_err(|source| ResolveError::Io { path, source })?
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
        let is_napi = match &locator {
            ModuleLocator::LocalFile(path) => {
                path.extension().and_then(|ext| ext.to_str()) == Some("node")
            }
            ModuleLocator::Cached { member, .. } => {
                Path::new(member).extension().and_then(|ext| ext.to_str()) == Some("node")
            }
            ModuleLocator::Native { .. } => false,
        };
        let source = if is_napi {
            Arc::<str>::from("")
        } else {
            let source =
                String::from_utf8(bytes).map_err(|_| ResolveError::NotUtf8 { url: url.clone() })?;
            Arc::from(source)
        };
        Ok(ResolvedModule {
            url,
            locator,
            source,
            kind,
        })
    }

    fn module_kind(&self, locator: &ModuleLocator) -> Result<ModuleKind, ResolveError> {
        match locator {
            ModuleLocator::LocalFile(path) => local_file_kind(path),
            ModuleLocator::Cached { package, member } => {
                let root = self.cached_root(package)?;
                Ok(cached_file_kind(&root, member))
            }
            ModuleLocator::Native { .. } => Ok(ModuleKind::Esm),
        }
    }

    fn locate_url(&self, url: Url) -> Result<(Url, ModuleLocator), ResolveError> {
        match url.scheme() {
            "ext" => Ok((
                url.clone(),
                ModuleLocator::Native {
                    name: url.to_string(),
                },
            )),
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| ResolveError::SpecifierNotFound {
                        specifier: url.to_string(),
                        referrer: self.project_root.clone(),
                    })?;
                Ok((url, self.file_locator(path)))
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
            "ext" => Ok((
                joined.clone(),
                ModuleLocator::Native {
                    name: joined.to_string(),
                },
            )),
            "file" => self.finalize_local_url(joined, specifier, referrer),
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
        let url = Url::from_file_path(&path).map_err(|()| ResolveError::SpecifierNotFound {
            specifier: specifier.to_owned(),
            referrer: referrer.clone(),
        })?;
        Ok((url, self.file_locator(path)))
    }

    /// Classify a real filesystem path into the right locator: a path inside the
    /// unpacked store becomes [`ModuleLocator::Cached`] (so its owning package is
    /// recognized for bare/`#imports` resolution), everything else is a
    /// [`ModuleLocator::LocalFile`]. The URL stays the same `file://` path either way.
    fn file_locator(&self, path: PathBuf) -> ModuleLocator {
        match self.cached_locator_for_path(&path) {
            Some((package, member)) if !member.is_empty() => {
                ModuleLocator::Cached { package, member }
            }
            _ => ModuleLocator::LocalFile(path),
        }
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
        let (package_name_text, subpath) = parse_package_specifier(specifier);
        let canonical_node_builtin =
            if node_resolver::DenoIsBuiltInNodeModuleChecker.is_builtin_node_module(specifier) {
                Some(specifier)
            } else if subpath.is_none()
                && node_resolver::DenoIsBuiltInNodeModuleChecker
                    .is_builtin_node_module(package_name_text)
            {
                Some(package_name_text)
            } else {
                None
            };
        if let Some(name) = canonical_node_builtin {
            return self.locate_node_builtin(name);
        }

        let owner = self.owner_for_referrer(referrer)?;
        if owner.package_name() == Some(package_name_text) {
            if context == ResolveContext::Require {
                if let Ok(resolution) = self.legacy_package_resolve(&owner, subpath, context) {
                    return self.target_resolution_to_locator(resolution);
                }
            }

            let subpath = subpath_for_exports(subpath);
            let Some(manifest) = owner.manifest() else {
                return Err(ResolveError::SubpathNotExported {
                    package: package_name_text.to_owned(),
                    subpath: subpath.clone(),
                });
            };
            let Some(exports) = manifest.exports.as_ref() else {
                return Err(ResolveError::SubpathNotExported {
                    package: package_name_text.to_owned(),
                    subpath,
                });
            };
            let resolution = self.package_exports_resolve(&owner, exports, &subpath, context)?;
            return self.target_resolution_to_locator(resolution);
        }

        let dep_name = PackageName::new(package_name_text.to_owned());
        let version = match self.dependency_version(&owner, &dep_name) {
            Some(version) => version,
            None => {
                return Err(ResolveError::BareSpecifierNotInLockfile {
                    name: package_name_text.to_owned(),
                });
            }
        };
        let dep_owner = self.cached_dependency_owner(&dep_name, &version)?;
        let resolution = match self.package_owner_resolve(&dep_owner, subpath, context) {
            Ok(resolution) => resolution,
            Err(err @ ResolveError::SubpathNotExported { .. })
            | Err(err @ ResolveError::NoMatchingCondition { .. }) => self
                .phantom_export_resolution(&dep_name, &version, subpath, context)?
                .ok_or(err)?,
            Err(err) => return Err(err),
        };
        self.target_resolution_to_locator(resolution)
    }

    fn owner_for_referrer(&self, referrer: &Url) -> Result<OwnerPackage, ResolveError> {
        match referrer.scheme() {
            "file" => {
                if let Some(owner) = self.cached_owner_for_file_referrer(referrer)? {
                    return Ok(owner);
                }
                let root = self.project_root_path().unwrap_or_default();
                let manifest = self.project_manifest()?;
                Ok(OwnerPackage::Root { root, manifest })
            }
            "meow" | "node" => {
                let root = self.project_root_path().unwrap_or_default();
                let manifest = self.project_manifest()?;
                Ok(OwnerPackage::Root { root, manifest })
            }
            other => Err(ResolveError::UnsupportedScheme {
                scheme: other.to_owned(),
                url: referrer.clone(),
            }),
        }
    }

    fn cached_owner_for_file_referrer(
        &self,
        referrer: &Url,
    ) -> Result<Option<OwnerPackage>, ResolveError> {
        let Ok(path) = referrer.to_file_path() else {
            return Ok(None);
        };
        let Some((package, _member)) = self.cached_locator_for_path(&path) else {
            return Ok(None);
        };
        let Some((name, version)) = self.by_integrity.get(&package).cloned() else {
            return Ok(None);
        };
        Ok(Some(self.cached_owner(package, name, version)?))
    }

    /// Build a cached [`OwnerPackage`] from disk: unpack the package and read its
    /// top-level `package.json`. Replaces the old in-memory `PackageFs` owner.
    fn cached_owner(
        &self,
        package: ContentHash,
        name: PackageName,
        version: Version,
    ) -> Result<OwnerPackage, ResolveError> {
        let root = self.cached_root(&package)?;
        let manifest = read_cached_manifest(&root, &root)?.unwrap_or_default();
        Ok(OwnerPackage::Cached {
            package,
            name,
            version,
            root,
            manifest,
        })
    }

    fn cached_dependency_owner(
        &self,
        dep_name: &PackageName,
        version: &Version,
    ) -> Result<OwnerPackage, ResolveError> {
        let Some(entry) = self.lockfile.get(dep_name, version) else {
            return Err(ResolveError::BareSpecifierNotInLockfile {
                name: dep_name.to_string(),
            });
        };
        self.cached_owner(entry.integrity.clone(), dep_name.clone(), version.clone())
    }

    fn package_owner_resolve(
        &self,
        owner: &OwnerPackage,
        subpath: Option<&str>,
        context: ResolveContext,
    ) -> Result<TargetResolution, ResolveError> {
        if let Some(exports) = owner
            .manifest()
            .and_then(|manifest| manifest.exports.as_ref())
        {
            self.package_exports_resolve(owner, exports, &subpath_for_exports(subpath), context)
        } else {
            self.legacy_package_resolve(owner, subpath, context)
        }
    }

    fn phantom_export_resolution(
        &self,
        dep_name: &PackageName,
        current_version: &Version,
        subpath: Option<&str>,
        context: ResolveContext,
    ) -> Result<Option<TargetResolution>, ResolveError> {
        let mut versions: Vec<_> = self
            .lockfile
            .iter()
            .filter(|entry| &entry.name == dep_name)
            .map(|entry| entry.version.clone())
            .collect();
        versions.sort_by(|left, right| right.cmp(left));

        for version in versions {
            if &version == current_version {
                continue;
            }
            let owner = self.cached_dependency_owner(dep_name, &version)?;
            match self.package_owner_resolve(&owner, subpath, context) {
                Ok(resolution) => return Ok(Some(resolution)),
                Err(ResolveError::SubpathNotExported { .. })
                | Err(ResolveError::NoMatchingCondition { .. }) => continue,
                Err(err) => return Err(err),
            }
        }
        Ok(None)
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
        let explicit = match owner {
            OwnerPackage::Root { .. } => self.root_deps.get(dep_name).cloned(),
            OwnerPackage::Cached {
                name,
                version,
                manifest,
                ..
            } => {
                let dependencies = self.lockfile.get(name, version)?;
                if let Some(version) = dependencies.dependencies.get(dep_name) {
                    return Some(version.clone());
                }
                if manifest.peer_dependencies.contains_key(dep_name.as_str()) {
                    return self.root_deps.get(dep_name).cloned();
                }
                None
            }
        };
        explicit.or_else(|| self.phantom_dependency_version(dep_name))
    }

    fn phantom_dependency_version(&self, dep_name: &PackageName) -> Option<Version> {
        self.lockfile
            .iter()
            .filter(|entry| &entry.name == dep_name)
            .map(|entry| entry.version.clone())
            .max()
    }

    fn target_resolution_to_locator(
        &self,
        resolution: TargetResolution,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        match resolution {
            TargetResolution::LocalFile(path) => self.local_target(path),
            TargetResolution::Cached { package, member } => {
                let locator = ModuleLocator::Cached { package, member };
                let url = self.cached_url(&locator)?;
                Ok((url, locator))
            }
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
                    OwnerPackage::Cached { package, root, .. } => {
                        if !root.join(&member).is_file() {
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
        context: ResolveContext,
    ) -> Result<TargetResolution, ResolveError> {
        let OwnerPackage::Cached { package, root, .. } = owner else {
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
            let member = finalize_cached_member(root, &member).ok_or_else(|| {
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

        let legacy_entry = owner.manifest().and_then(|manifest| match context {
            ResolveContext::Import => manifest.module.as_deref().or(manifest.main.as_deref()),
            ResolveContext::Require => manifest.main.as_deref(),
        });
        if let Some(entry) = legacy_entry
            .and_then(normalize_legacy_member)
            .and_then(|member| finalize_cached_member(root, &member))
        {
            return Ok(TargetResolution::Cached {
                package: package.clone(),
                member: entry,
            });
        }

        let member =
            cached_directory_index(root, "").ok_or_else(|| ResolveError::SpecifierNotFound {
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
        root: PathBuf,
        manifest: PackageJson,
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
            OwnerPackage::Cached { manifest, .. } => Some(manifest),
        }
    }

    fn referrer_url(&self, project_root: &Url) -> Url {
        match self {
            OwnerPackage::Root { root, .. } => Url::from_file_path(root.join("package.json"))
                .ok()
                .unwrap_or_else(|| project_root.clone()),
            OwnerPackage::Cached { root, .. } => Url::from_file_path(root.join("package.json"))
                .ok()
                .unwrap_or_else(|| project_root.clone()),
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

/// Return the path of `path` relative to the unpacked-store `root`, tolerating the
/// macOS `/var` -> `/private/var` symlink divergence: try a direct prefix match
/// first (our minted URLs are un-canonicalized store paths), then fall back to
/// canonicalizing both sides so a referrer Deno may have canonicalized still maps
/// back to its package.
fn strip_store_prefix(path: &Path, root: &Path) -> Option<PathBuf> {
    if let Ok(rel) = path.strip_prefix(root) {
        return Some(rel.to_path_buf());
    }

    // The cached file may not exist yet: resolving a file:// URL into the
    // unpacked store is what triggers `UnpackedStore::ensure`. On Windows,
    // URL conversion/canonicalization can also introduce or remove the `\\?\`
    // extended-length prefix. Do a purely lexical normalized-prefix comparison
    // before attempting canonicalization so missing cached members still classify
    // as `ModuleLocator::Cached` and get unpacked instead of read as local files.
    if let Some(rel) = strip_normalized_prefix(path, root) {
        return Some(rel);
    }

    let canonical_path = strip_unc_prefix(std::fs::canonicalize(path).ok()?);
    let stripped_root = strip_unc_prefix(root.to_path_buf());
    if let Ok(rel) = canonical_path.strip_prefix(&stripped_root) {
        return Some(rel.to_path_buf());
    }
    let canonical_root = strip_unc_prefix(std::fs::canonicalize(root).ok()?);
    canonical_path
        .strip_prefix(&canonical_root)
        .ok()
        .map(|rel| rel.to_path_buf())
}

fn strip_normalized_prefix(path: &Path, root: &Path) -> Option<PathBuf> {
    let path_text = normalize_path_text(path);
    let mut root_text = normalize_path_text(root);
    while root_text.ends_with('/') && root_text.len() > 1 {
        root_text.pop();
    }
    let rel = path_text.strip_prefix(&root_text)?;
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel.is_empty() {
        return Some(PathBuf::new());
    }
    Some(rel.split('/').collect())
}

fn normalize_path_text(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_owned();
    }
    text
}

/// Strip Windows UNC extended-length prefix (\\?\) for path comparisons.
fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix("\\\\?\\") {
        PathBuf::from(stripped)
    } else {
        path
    }
}

fn finalize_local_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if let Some(candidate) = typescript_source_fallback(path) {
        return Some(candidate);
    }
    for suffix in EXTENSIONS {
        let candidate = append_path_suffix(path, suffix);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    local_directory_index(path)
}

fn typescript_source_fallback(path: &Path) -> Option<PathBuf> {
    let requested_ext = path.extension().and_then(|ext| ext.to_str())?;
    let fallbacks: &[&str] = match requested_ext {
        "js" => &["ts", "tsx", "mts"],
        "mjs" => &["mts", "ts", "tsx"],
        "cjs" => &["cts", "ts", "tsx"],
        _ => return None,
    };
    for ext in fallbacks {
        let candidate = path.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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

fn cached_member_is_file(root: &Path, member: &str) -> bool {
    !member.is_empty() && root.join(member).is_file()
}

fn finalize_cached_member(root: &Path, member: &str) -> Option<String> {
    let member = member.trim_start_matches('/');
    if member.is_empty() {
        return cached_package_entry(root, "");
    }
    if cached_member_is_file(root, member) {
        return Some(member.to_owned());
    }
    for suffix in EXTENSIONS {
        let candidate = format!("{member}{suffix}");
        if cached_member_is_file(root, &candidate) {
            return Some(candidate);
        }
    }
    cached_package_entry(root, member)
}

fn cached_package_entry(root: &Path, member: &str) -> Option<String> {
    if let Some(main) = cached_manifest_for_dir(root, member)
        .and_then(|manifest| manifest.main)
        .as_deref()
        .and_then(normalize_legacy_member)
    {
        let candidate = if member.is_empty() {
            main
        } else {
            format!("{member}/{main}")
        };
        if let Some(found) = finalize_cached_member(root, &candidate) {
            return Some(found);
        }
    }
    cached_directory_index(root, member)
}

fn cached_directory_index(root: &Path, member: &str) -> Option<String> {
    for index in INDEX_FILES {
        let candidate = if member.is_empty() {
            index.to_owned()
        } else {
            format!("{member}/{index}")
        };
        if cached_member_is_file(root, &candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Read the `package.json` directly under `dir` (relative to the unpacked package
/// `root`), if present. Mirrors the old `PackageFs::manifest_for_dir` against disk.
fn cached_manifest_for_dir(root: &Path, dir: &str) -> Option<PackageJson> {
    let manifest_dir = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    read_cached_manifest(root, &manifest_dir).ok().flatten()
}

/// Walk up from a cached `member` to the nearest enclosing `package.json` inside the
/// unpacked package `root`, returning its parsed manifest (the top-level one if none
/// nested matches). Mirrors `PackageFs::nearest_manifest` against disk.
fn nearest_cached_manifest(root: &Path, member: &str) -> PackageJson {
    let mut current = member.rsplit_once('/').map(|(dir, _)| dir);
    while let Some(dir) = current {
        if let Ok(Some(manifest)) = read_cached_manifest(root, &root.join(dir)) {
            return manifest;
        }
        current = dir.rsplit_once('/').map(|(parent, _)| parent);
    }
    read_cached_manifest(root, root)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn read_cached_manifest(root: &Path, dir: &Path) -> Result<Option<PackageJson>, ResolveError> {
    let path = dir.join("package.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|err| ResolveError::InvalidManifest {
                    package: root.display().to_string(),
                    reason: err.to_string(),
                })
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ResolveError::Io { path, source }),
    }
}

fn package_member_start(components: &[Component<'_>], index: usize) -> Option<usize> {
    let first = components.get(index)?.as_os_str().to_str()?;
    if first == ".pnpm" {
        let node_modules_index = components[index + 1..]
            .iter()
            .position(|component| component.as_os_str() == "node_modules")
            .map(|offset| index + 1 + offset)?;
        return package_member_start(components, node_modules_index + 1);
    }
    if first == "node_modules" {
        return package_member_start(components, index + 1);
    }
    if first.starts_with('@') {
        components.get(index + 1)?;
        Some(index + 2)
    } else {
        Some(index + 1)
    }
}

fn append_path_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

fn cached_file_kind(root: &Path, member: &str) -> ModuleKind {
    match Path::new(member).extension().and_then(|ext| ext.to_str()) {
        Some("mjs") => ModuleKind::Esm,
        Some("cjs") => ModuleKind::Cjs,
        Some("json") => ModuleKind::Json,
        Some("js") => {
            let manifest = nearest_cached_manifest(root, member);
            if manifest.package_type.as_deref() == Some("module") {
                return ModuleKind::Esm;
            }
            if let Some(main) = manifest.main.as_deref().and_then(normalize_legacy_member) {
                if member == main {
                    return ModuleKind::Cjs;
                }
            }
            if let Some(module_path) = manifest.module.as_deref().and_then(normalize_legacy_member)
            {
                let module_dir = Path::new(&module_path).parent().unwrap_or(Path::new(""));
                if Path::new(member).starts_with(module_dir) {
                    return ModuleKind::Esm;
                }
            }
            ModuleKind::Cjs
        }
        _ => {
            let manifest = nearest_cached_manifest(root, member);
            if manifest.package_type.as_deref() == Some("module") {
                return ModuleKind::Esm;
            }
            if let Some(module_path) = manifest.module.as_deref().and_then(normalize_legacy_member)
            {
                let module_dir = Path::new(&module_path).parent().unwrap_or(Path::new(""));
                if Path::new(member).starts_with(module_dir) {
                    return ModuleKind::Esm;
                }
            }
            ModuleKind::Cjs
        }
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
    fn deno_node_builtins_resolve_without_native_shim_registration() {
        let dir = unique_dir("deno-builtins");
        let resolver = resolver(&dir);
        let referrer = Url::from_file_path(dir.join("main.mjs")).expect("referrer URL");

        for specifier in ["node:http", "http"] {
            let (url, locator) = resolver
                .locate(specifier, &referrer)
                .expect("Deno-owned builtin resolves");
            assert_eq!(url.as_str(), "node:http");
            assert!(matches!(locator, ModuleLocator::Native { name } if name == "node:http"));
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    fn cached_package_archive(package_json: &str) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        let bytes = package_json.as_bytes();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, "package/package.json", bytes)
            .expect("append manifest");
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
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
    fn resolves_typescript_source_for_javascript_esm_specifier() {
        let dir = unique_dir("ts-js-fallback");
        std::fs::write(dir.join("benchmarks.ts"), "export const ok = true;\n")
            .expect("write module");
        let resolver = resolver(&dir);
        let referrer = Url::from_file_path(dir.join("index.ts")).expect("referrer URL");
        let (url, locator) = resolver
            .locate("./benchmarks.js", &referrer)
            .expect("js specifier resolves to ts source");
        assert_eq!(
            url.as_str(),
            Url::from_file_path(dir.join("benchmarks.ts"))
                .unwrap()
                .as_str()
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
    fn package_root_for_require_resolves_cached_self_reference_to_projected_root() {
        let dir = unique_dir("require-self-root");
        let archive =
            cached_package_archive(r#"{"name":"esbuild","version":"1.0.0","main":"lib/main.js"}"#);
        let integrity = ContentHash::of(&archive);
        let package_name = PackageName::new("esbuild");
        let version = Version::parse("1.0.0").unwrap();

        let cache = Arc::new(Cache::with_root(dir.join("cache")));
        cache.store(&archive).expect("store cached blob");
        let mut lockfile = Lockfile::new();
        lockfile.upsert(meow_pkg::LockEntry {
            name: package_name.clone(),
            version: version.clone(),
            integrity: integrity.clone(),
            dependencies: BTreeMap::new(),
            registry: meow_pkg::RegistryProvenance::new("https://registry.npmjs.org"),
            capabilities: Vec::new(),
            wasm: Vec::new(),
            meow: meow_pkg::VersionReq::parse("*").unwrap(),
        });
        let resolver = Resolver::new(
            cache,
            Arc::new(lockfile),
            BTreeMap::from([(package_name, version)]),
            Url::from_directory_path(&dir).expect("project root URL"),
            meow_runtime::native::native_module_registry(),
        );

        let projected_root = dir
            .join("node_modules")
            .join(".meow")
            .join("esbuild@1.0.0")
            .join("node_modules")
            .join("esbuild");
        std::fs::create_dir_all(&projected_root).expect("create projected package root");
        std::fs::write(projected_root.join("package.json"), "{}").expect("projected manifest");

        let unpacked_root = resolver
            .cached_root(&integrity)
            .expect("unpack cached package");
        let referrer =
            Url::from_file_path(unpacked_root.join("package.json")).expect("referrer URL");
        let root = resolver
            .package_root_for_require("esbuild", &referrer)
            .expect("self package root resolves");
        assert_eq!(root, projected_root);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cached_package_uses_root_for_declared_peer_dependency() {
        let dir = unique_dir("peer-deps");
        let manifest = r#"{"peerDependencies":{"react":"^19.0.0"}}"#;
        let archive = cached_package_archive(manifest);
        let integrity = ContentHash::of(&archive);

        let next_name = PackageName::new("next");
        let next_version = Version::parse("19.0.0").unwrap();
        let react_version = Version::parse("18.3.1").unwrap();

        let mut lockfile = Lockfile::new();
        lockfile.upsert(meow_pkg::LockEntry {
            name: next_name.clone(),
            version: next_version.clone(),
            integrity: integrity.clone(),
            dependencies: BTreeMap::new(),
            registry: meow_pkg::RegistryProvenance::new("https://registry.npmjs.org"),
            capabilities: Vec::new(),
            wasm: Vec::new(),
            meow: meow_pkg::VersionReq::parse("*").unwrap(),
        });

        let mut root_deps = BTreeMap::new();
        root_deps.insert(PackageName::new("react"), react_version.clone());

        let resolver = Resolver::new(
            Arc::new(Cache::with_root(dir.join("cache"))),
            Arc::new(lockfile),
            root_deps,
            Url::from_directory_path(&dir).expect("project root URL"),
            meow_runtime::native::native_module_registry(),
        );

        let cache = Arc::new(Cache::with_root(dir.join("cache")));
        cache.store(&archive).expect("store cached blob");
        let store = UnpackedStore::new(dir.join("cache").join("unpacked"), cache);
        let root = store.ensure(&integrity).expect("unpack cached package");
        let manifest = read_cached_manifest(&root, &root)
            .expect("read manifest")
            .unwrap_or_default();
        let dep_owner = OwnerPackage::Cached {
            package: integrity,
            name: next_name,
            version: next_version,
            root,
            manifest,
        };

        assert_eq!(
            resolver.dependency_version(&dep_owner, &PackageName::new("react")),
            Some(react_version.clone())
        );
        assert_eq!(
            resolver.dependency_version(&dep_owner, &PackageName::new("react-dom")),
            None
        );
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
