//! Loader behavior tests. Exercise the real resolver + loader + runtime without any
//! `node_modules` tree on disk.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deno_core::url::Url;
use deno_core::{
    ModuleLoadOptions, ModuleLoadResponse, ModuleLoader, ModuleSourceCode, ModuleSpecifier,
    RequestedModuleType,
};
use meow_graph::GraphDb;
use meow_loader::{
    encode_cache_url, MeowModuleLoader, ModuleKind, ModuleLocator, ResolveError, Resolver,
};
use meow_pkg::{
    Cache, CacheError, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
};
use meow_runtime::node::NpmPackageFolderResolver;
use meow_runtime::{hermetic, node, print_sink_extension, PrintSink, Runtime, RuntimeOptions};

fn unique_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("meow-loader-test-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn cache_arc(root: &Path) -> Arc<Cache> {
    Arc::new(Cache::with_root(root))
}

fn dir_url(dir: &Path) -> Url {
    Url::from_directory_path(dir).expect("dir → file URL")
}
fn parsed_version(text: &str) -> Version {
    Version::parse(text).expect("valid version")
}

fn root_deps(entries: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
    entries
        .iter()
        .map(|(name, ver)| (PackageName::new((*name).to_owned()), parsed_version(ver)))
        .collect()
}

fn lock_entry(
    name: &str,
    version: &str,
    integrity: meow_pkg::ContentHash,
    deps: &[(&str, &str)],
) -> LockEntry {
    LockEntry {
        name: PackageName::new(name.to_owned()),
        version: Version::parse(version).expect("valid version"),
        integrity,
        dependencies: deps
            .iter()
            .map(|(name, version)| {
                (
                    PackageName::new((*name).to_owned()),
                    Version::parse(version).expect("valid version"),
                )
            })
            .collect(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: Vec::new(),
        wasm: Vec::new(),
        meow: VersionReq::parse("*").expect("valid requirement"),
    }
}

fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("package/{path}"), *bytes)
            .expect("append tar member");
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

fn resolver_with(
    cache_root: &Path,
    lockfile: Lockfile,
    root_deps: BTreeMap<PackageName, Version>,
    project_root: &Path,
) -> Resolver {
    Resolver::new(
        cache_arc(cache_root),
        Arc::new(lockfile),
        root_deps,
        dir_url(project_root),
        meow_runtime::native::native_module_registry(),
    )
}

fn capture() -> (Rc<RefCell<String>>, deno_core::Extension) {
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    let sink = PrintSink(Rc::new(move |msg: &str, is_err: bool| {
        if !is_err {
            o.borrow_mut().push_str(msg);
        }
    }));
    (out, print_sink_extension(sink))
}
struct TestDenoNodeBridge {
    resolver: Resolver,
    store: meow_pkg::UnpackedStore,
}

impl TestDenoNodeBridge {
    fn new(resolver: Resolver, cache_root: &Path) -> Self {
        let cache = cache_arc(cache_root);
        Self {
            resolver,
            store: meow_pkg::UnpackedStore::new(cache_root.join("unpacked"), cache),
        }
    }

    fn referrer_url(&self, referrer: &node::UrlOrPathRef) -> Url {
        if let Ok(path) = referrer.path() {
            if let Some(url) = self.unpacked_path_to_cache_url(path) {
                return url;
            }
        }
        referrer.url().expect("test bridge referrer URL").clone()
    }

    fn unpacked_path_to_cache_url(&self, path: &Path) -> Option<Url> {
        let rel = path.strip_prefix(self.store.root()).ok()?;
        let mut components = rel.components();
        let package_host = components.next()?.as_os_str().to_str()?;
        let package = meow_pkg::ContentHash::from_url_host(package_host).ok()?;
        let member = components.as_path().to_string_lossy().replace('\\', "/");
        if member.is_empty() {
            return None;
        }
        Some(encode_cache_url(&package, &member))
    }

    fn module_kind(&self, specifier: &Url) -> Option<ModuleKind> {
        let resolved = self
            .resolver
            .resolve_require(specifier.as_str(), specifier)
            .ok()?;
        Some(resolved.kind)
    }
}

impl node::NpmPackageFolderResolver for TestDenoNodeBridge {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        referrer: &node::UrlOrPathRef,
    ) -> Result<PathBuf, node::PackageFolderResolveError> {
        let referrer_url = self.referrer_url(referrer);
        let resolved = self
            .resolver
            .resolve_require(specifier, &referrer_url)
            .unwrap_or_else(|err| {
                panic!(
                    "test Deno Node bridge failed to resolve package {specifier:?} from {}: {err}",
                    referrer.display()
                )
            });

        let package_root = match resolved.locator {
            ModuleLocator::Cached { package, .. } => {
                let package_label = package.to_sri();
                self.store.ensure(&package).unwrap_or_else(|err| {
                    panic!("test Deno Node bridge failed to unpack package {package_label}: {err}")
                })
            }
            ModuleLocator::LocalFile(ref path) => {
                path.parent().unwrap_or(path.as_path()).to_path_buf()
            }
            ModuleLocator::Native { .. } => {
                panic!(
                    "test Deno Node bridge cannot expose native module {specifier:?} as a package"
                )
            }
        };
        Ok(package_root)
    }

    fn resolve_types_package_folder(
        &self,
        types_package_name: &str,
        _maybe_package_version: Option<&node::Version>,
        maybe_referrer: Option<&node::UrlOrPathRef>,
    ) -> Option<PathBuf> {
        let types_package_name = if types_package_name.starts_with("@types/") {
            types_package_name.to_owned()
        } else {
            format!("@types/{types_package_name}")
        };
        let referrer = maybe_referrer
            .and_then(|referrer| referrer.url().ok())
            .unwrap_or_else(|| self.resolver.project_root());
        let referrer = node::UrlOrPathRef::from_url(referrer);
        self.resolve_package_folder_from_package(&types_package_name, &referrer)
            .ok()
    }
}

impl node::InNpmPackageChecker for TestDenoNodeBridge {
    fn in_npm_package(&self, specifier: &Url) -> bool {
        if specifier.scheme() == meow_loader::CACHE_SCHEME {
            return true;
        }
        let Ok(path) = specifier.to_file_path() else {
            return false;
        };
        path.starts_with(self.store.root())
    }
}

impl node::NodeRequireLoader for TestDenoNodeBridge {
    fn ensure_read_permission<'a>(
        &self,
        _permissions: &mut node::PermissionsContainer,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, node::JsErrorBox> {
        Ok(path)
    }

    fn load_text_file_lossy(&self, path: &Path) -> Result<node::FastString, node::JsErrorBox> {
        let source = std::fs::read(path).map_err(|err| {
            node::JsErrorBox::generic(format!("failed reading {}: {err}", path.display()))
        })?;
        Ok(String::from_utf8_lossy(&source).into_owned().into())
    }

    fn is_maybe_cjs(&self, specifier: &Url) -> Result<bool, node::PackageJsonLoadError> {
        if let Ok(path) = specifier.to_file_path() {
            if let Some(cache_url) = self.unpacked_path_to_cache_url(&path) {
                return Ok(matches!(
                    self.module_kind(&cache_url),
                    Some(ModuleKind::Cjs)
                ));
            }
            match path.extension().and_then(|ext| ext.to_str()) {
                None | Some("cjs") | Some("cts") => return Ok(true),
                Some("json") | Some("mjs") | Some("mts") => return Ok(false),
                _ => {}
            }
        }
        Ok(matches!(self.module_kind(specifier), Some(ModuleKind::Cjs)))
    }

    fn is_maybe_cjs_from_require(
        &self,
        specifier: &Url,
    ) -> Result<bool, node::PackageJsonLoadError> {
        self.is_maybe_cjs(specifier)
    }

    fn resolve_package_folder_from_name(&self, package_name: &str) -> Option<PathBuf> {
        let referrer = node::UrlOrPathRef::from_url(self.resolver.project_root());
        self.resolve_package_folder_from_package(package_name, &referrer)
            .ok()
    }
}

// === RT-007 ===
fn node_extensions_without_bridge(cwd: &Path) -> Vec<deno_core::Extension> {
    node_extensions(cwd, None)
}

fn node_extensions_with_bridge(
    cwd: &Path,
    resolver: Resolver,
    cache_root: &Path,
) -> Vec<deno_core::Extension> {
    node_extensions(cwd, Some(TestDenoNodeBridge::new(resolver, cache_root)))
}

fn node_extensions(cwd: &Path, bridge: Option<TestDenoNodeBridge>) -> Vec<deno_core::Extension> {
    let mut opts = node::NodeOptions::enabled(Vec::new(), cwd.to_path_buf());
    if let Some(bridge) = bridge {
        let bridge: Rc<dyn node::DenoNodeBridge> = Rc::new(bridge);
        opts.deno_node_services = Some(node::DenoNodeServicesBuilder::new(bridge).build());
    }
    let mut exts = node::extensions(opts);
    exts.extend(hermetic::extensions(hermetic::HermeticConfig::default()));
    exts
}
// === /RT-007 ===

fn sync_options() -> ModuleLoadOptions {
    ModuleLoadOptions {
        is_dynamic_import: false,
        is_synchronous: true,
        requested_module_type: RequestedModuleType::None,
    }
}

fn load_code(loader: &MeowModuleLoader, spec: &ModuleSpecifier) -> Result<String, String> {
    match loader.load(spec, None, sync_options()) {
        ModuleLoadResponse::Sync(Ok(source)) => match source.code {
            ModuleSourceCode::String(s) => Ok(s.as_str().to_owned()),
            ModuleSourceCode::Bytes(_) => panic!("expected string module source"),
        },
        ModuleLoadResponse::Sync(Err(e)) => Err(e.to_string()),
        _ => panic!("expected a synchronous load response"),
    }
}

async fn run_entry(
    loader: Rc<dyn ModuleLoader>,
    resolver: Resolver,
    cache_root: Option<&Path>,
    entry: &Path,
    mut extensions: Vec<deno_core::Extension>,
) -> Result<(), meow_runtime::RuntimeError> {
    // === RT-007 ===
    // Deno's CommonJS executor owns `require()` and `node:*`; cached npm
    // package paths are supplied only for tests that actually cross that seam.
    let cwd = entry.parent().unwrap_or_else(|| Path::new("."));
    extensions.push(meow_loader::cjs_resolve_extension(resolver.clone()));
    extensions.extend(match cache_root {
        Some(cache_root) => node_extensions_with_bridge(cwd, resolver, cache_root),
        None => node_extensions_without_bridge(cwd),
    });
    // === /RT-007 ===
    let mut rt = Runtime::new(RuntimeOptions {
        module_loader: loader,
        extensions,
    })
    .expect("runtime initializes");
    let spec = ModuleSpecifier::from_file_path(entry).expect("entry → file URL");
    rt.run_main_module(&spec).await
}

#[tokio::test]
async fn cached_dep_runs_end_to_end_with_no_node_modules() {
    let proj = unique_dir("e2e");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);

    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","type":"module","exports":{".":{"import":"./esm/index.js","require":"./cjs/index.cjs","default":"./fallback.js"}}}"#,
            ),
            (
                "esm/index.js",
                b"import { value } from \"nested\";\nexport const greet = () => `from ${value}`;\n",
            ),
            ("cjs/index.cjs", b"module.exports = { greet() { return 'wrong'; } };\n"),
            ("fallback.js", b"export const greet = () => 'fallback';\n"),
        ]))
        .expect("store dep blob");
    let nested_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"nested","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const value = 'cache';\n"),
        ]))
        .expect("store nested blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "dep",
        "1.0.0",
        dep_hash.clone(),
        &[("nested", "1.0.0")],
    ));
    lockfile.upsert(lock_entry("nested", "1.0.0", nested_hash, &[]));

    let entry = proj.join("main.ts");
    std::fs::write(
        &entry,
        "import { greet } from \"dep\";\nconst msg: string = greet();\nconsole.log(msg);\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("dep", "1.0.0")]), &proj);
    let graph = Rc::new(RefCell::new(GraphDb::new()));
    let loader = Rc::new(MeowModuleLoader::new(resolver.clone(), graph));

    let (out, sink_ext) = capture();
    let mut rt = Runtime::new(RuntimeOptions {
        module_loader: loader,
        // === RT-007 === Pure ESM loading only needs Deno's Node built-ins
        // registered; no package-folder bridge is involved here.
        extensions: {
            let mut exts = node_extensions_without_bridge(&proj);
            exts.push(sink_ext);
            exts
        },
        // === /RT-007 ===
    })
    .expect("runtime initializes");

    let spec = ModuleSpecifier::from_file_path(&entry).expect("entry → file URL");
    rt.run_main_module(&spec)
        .await
        .expect("module runs to completion");

    assert_eq!(*out.borrow(), "from cache\n");
    assert!(
        !proj.join("node_modules").exists(),
        "no node_modules is created"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn one_resolver_handles_relative_and_bare() {
    let proj = unique_dir("single");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const v = 1;\n"),
        ]))
        .expect("store dep blob");
    std::fs::write(proj.join("a.ts"), "export const a = 1;\n").expect("write local file");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash.clone(), &[]));
    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("dep", "1.0.0")]), &proj);

    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");
    let (rel_url, rel_loc) = resolver
        .locate("./a.ts", &referrer)
        .expect("relative resolves");
    assert_eq!(rel_url.scheme(), "file");
    assert!(matches!(rel_loc, ModuleLocator::LocalFile(_)));

    let (bare_url, bare_loc) = resolver.locate("dep", &referrer).expect("bare resolves");
    assert_eq!(
        bare_url,
        meow_loader::encode_cache_url(&dep_hash, "index.js")
    );
    match bare_loc {
        ModuleLocator::Cached { package, member } => {
            assert_eq!(package, dep_hash);
            assert_eq!(member, "index.js");
        }
        other => panic!("expected Cached, got {other:?}"),
    }
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn meow_import_resolves() {
    let proj = unique_dir("meow-native");
    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (url, locator) = resolver
        .locate("meow:http", &referrer)
        .expect("meow:http resolves");
    assert_eq!(url.as_str(), "meow:http");
    match locator {
        ModuleLocator::Native { name } => assert_eq!(name, "http"),
        other => panic!("expected Native locator, got {other:?}"),
    }

    let resolved = resolver
        .resolve("meow:http", &referrer)
        .expect("native module source resolves");
    assert_eq!(resolved.kind, meow_loader::ModuleKind::Esm);
    assert!(
        resolved.source.contains("export function serve"),
        "native source served from the runtime registry"
    );

    let err = resolver
        .locate("meow:nope", &referrer)
        .expect_err("unknown meow module is refused");
    assert!(
        matches!(err, ResolveError::UnknownNativeModule { .. }),
        "typed native-module error, got: {err:?}"
    );
    assert!(
        err.to_string().contains("meow:http"),
        "diagnostic points at the fix, got: {err}"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn node_builtin_resolves_from_node_and_bare_specifiers() {
    let proj = unique_dir("node-native");
    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (node_url, node_locator) = resolver
        .locate("node:fs", &referrer)
        .expect("node:fs resolves");
    assert_eq!(node_url.as_str(), "node:fs");
    match node_locator {
        ModuleLocator::Native { name } => assert_eq!(name, "node:fs"),
        other => panic!("expected Native locator, got {other:?}"),
    }

    let (bare_url, bare_locator) = resolver.locate("fs", &referrer).expect("fs resolves");
    assert_eq!(bare_url, node_url, "bare builtin canonicalizes to node:");
    match bare_locator {
        ModuleLocator::Native { name } => assert_eq!(name, "node:fs"),
        other => panic!("expected Native locator, got {other:?}"),
    }
    let (dns_url, dns_locator) = resolver.locate("dns", &referrer).expect("dns resolves");
    assert_eq!(dns_url.as_str(), "node:dns");
    match dns_locator {
        ModuleLocator::Native { name } => assert_eq!(name, "node:dns"),
        other => panic!("expected Native locator, got {other:?}"),
    }

    let resolved = resolver
        .resolve("fs/promises", &referrer)
        .expect("fs/promises resolves");
    assert_eq!(resolved.url.as_str(), "node:fs/promises");
    assert!(
        resolved.source.contains("writeFile"),
        "node builtin source served from the runtime registry"
    );

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn lockfile_package_does_not_shadow_builtin_name() {
    let proj = unique_dir("node-shadow");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"path","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export default 'shadow-package';\n"),
        ]))
        .expect("store dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("path", "1.0.0", dep_hash.clone(), &[]));
    let resolver = resolver_with(
        &cache_root,
        lockfile,
        root_deps(&[("path", "1.0.0")]),
        &proj,
    );
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (bare_url, bare_locator) = resolver
        .locate("path", &referrer)
        .expect("builtin path resolves for bare specifier");
    assert_eq!(bare_url.as_str(), "node:path");
    assert!(matches!(bare_locator, ModuleLocator::Native { name } if name == "node:path"));

    let (node_url, node_locator) = resolver
        .locate("node:path", &referrer)
        .expect("explicit node:path still resolves");
    assert_eq!(node_url.as_str(), "node:path");
    assert!(matches!(node_locator, ModuleLocator::Native { name } if name == "node:path"));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn missing_lockfile_entry_falls_back_to_node_builtin() {
    let proj = unique_dir("node-builtin-missing-lock");
    let cache_root = proj.join("cache");
    let resolver = resolver_with(
        &cache_root,
        Lockfile::new(),
        root_deps(&[("dns", "0.0.0")]),
        &proj,
    );
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (url, locator) = resolver
        .locate("dns", &referrer)
        .expect("dns falls back to node builtin when the lock has no package entry");
    assert_eq!(url.as_str(), "node:dns");
    match locator {
        ModuleLocator::Native { name } => assert_eq!(name, "node:dns"),
        other => panic!("expected Native locator, got {other:?}"),
    }

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn cached_package_dependency_dns_entry_prefers_builtin() {
    let proj = unique_dir("node-builtin-locked-entry");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let next_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"next","version":"1.0.0","type":"module"}"#,
            ),
            ("dist/bin/next", b"import dns from 'dns';\n"),
        ]))
        .expect("store next-like dep blob");
    let dns_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dns","version":"1.0.0","type":"module","exports":"./index.js"}"#,
            ),
            ("index.js", b"export default 'shadowed package';\n"),
        ]))
        .expect("store dns-like dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "next",
        "1.0.0",
        next_hash.clone(),
        &[("dns", "1.0.0")],
    ));
    lockfile.upsert(lock_entry("dns", "1.0.0", dns_hash.clone(), &[]));
    let resolver = resolver_with(
        &cache_root,
        lockfile,
        root_deps(&[("next", "1.0.0")]),
        &proj,
    );
    let referrer = encode_cache_url(&next_hash, "dist/bin/next");

    let (url, locator) = resolver
        .locate("dns", &referrer)
        .expect("cached package dns import prefers node builtin");
    assert_eq!(url.as_str(), "node:dns");
    assert!(matches!(locator, ModuleLocator::Native { name } if name == "node:dns"));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn cached_package_missing_lockfile_entry_falls_back_to_node_builtin() {
    let proj = unique_dir("node-builtin-cached-missing-lock");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let next_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"next","version":"1.0.0","type":"module"}"#,
            ),
            ("dist/bin/next", b"import dns from 'dns';\n"),
        ]))
        .expect("store next-like dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "next",
        "1.0.0",
        next_hash.clone(),
        &[("dns", "0.0.0")],
    ));
    let resolver = resolver_with(
        &cache_root,
        lockfile,
        root_deps(&[("next", "1.0.0")]),
        &proj,
    );
    let referrer = encode_cache_url(&next_hash, "dist/bin/next");

    let (url, locator) = resolver
        .locate("dns", &referrer)
        .expect("cached package dns import falls back to node builtin");
    assert_eq!(url.as_str(), "node:dns");
    match locator {
        ModuleLocator::Native { name } => assert_eq!(name, "node:dns"),
        other => panic!("expected Native locator, got {other:?}"),
    }

    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn cached_package_uses_root_peer_dependency() {
    let proj = unique_dir("peer-dep-fallback");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);

    let react_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"react","version":"1.0.0","type":"commonjs","main":"./index.js"}"#,
            ),
            (
                "index.js",
                b"module.exports = { value: 'peer root react' };\n",
            ),
        ]))
        .expect("store react peer provider");

    let next_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"next","version":"1.0.0","type":"commonjs","main":"./index.js","peerDependencies":{"react":"1.0.0"}}"#,
            ),
            ("index.js", b"module.exports = require('react');\n"),
        ]))
        .expect("store next peer consumer");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("react", "1.0.0", react_hash.clone(), &[]));
    lockfile.upsert(lock_entry("next", "1.0.0", next_hash, &[]));

    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const react = require('next');\nconsole.log(react.value);\n",
    )
    .expect("write entry");

    let resolver = resolver_with(
        &cache_root,
        lockfile,
        root_deps(&[("next", "1.0.0"), ("react", "1.0.0")]),
        &proj,
    );
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(
        loader,
        resolver.clone(),
        Some(&cache_root),
        &entry,
        vec![sink_ext],
    )
    .await
    .expect("cached package resolves peer dependency from root");

    assert_eq!(*out.borrow(), "peer root react\n");
    assert!(!proj.join("node_modules").exists());
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn ts_source_is_stripped_on_load() {
    let proj = unique_dir("strip");
    let entry = proj.join("typed.ts");
    std::fs::write(&entry, "export const n: number = 41 + 1;\n").expect("write");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader = MeowModuleLoader::new(resolver.clone(), Rc::new(RefCell::new(GraphDb::new())));

    let spec = ModuleSpecifier::from_file_path(&entry).expect("file URL");
    let code = load_code(&loader, &spec).expect("typed module loads");
    assert!(!code.contains("number"), "type annotation erased: {code:?}");
    assert!(code.contains("41 + 1"), "runtime expression kept: {code:?}");
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn non_erasable_ts_enum_is_a_load_error() {
    let proj = unique_dir("enum");
    let entry = proj.join("bad.ts");
    std::fs::write(&entry, "enum E { A }\nexport const e = E.A;\n").expect("write");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader = MeowModuleLoader::new(resolver.clone(), Rc::new(RefCell::new(GraphDb::new())));

    let spec = ModuleSpecifier::from_file_path(&entry).expect("file URL");
    let err = load_code(&loader, &spec).expect_err("enum module fails to load");
    assert!(
        err.contains("cannot load"),
        "carries an honest load diagnostic: {err:?}"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn tampered_cache_blob_surfaces_integrity_error() {
    let proj = unique_dir("tamper");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const v = 1;\n"),
        ]))
        .expect("store dep blob");

    std::fs::write(cache.path_for(&dep_hash), b"not a valid tarball anymore").expect("tamper blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash.clone(), &[]));
    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("dep", "1.0.0")]), &proj);

    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer");
    let err = resolver
        .resolve("dep", &referrer)
        .expect_err("tampered blob is refused");
    assert!(
        matches!(
            err,
            ResolveError::Cache(CacheError::IntegrityMismatch { .. })
        ),
        "integrity failure surfaces, got: {err:?}"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn first_party_js_commonjs_runs_end_to_end() {
    let proj = unique_dir("first-party-js-cjs");
    let entry = proj.join("app.js");
    std::fs::write(proj.join("dep.cjs"), "module.exports = { answer: 42 };\n").expect("write dep");
    std::fs::write(
        &entry,
        "const dep = require('./dep.cjs');\nconsole.log(dep.answer);\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("first-party CommonJS runs");

    assert_eq!(*out.borrow(), "42\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn esm_imports_cached_cjs_dependency() {
    let proj = unique_dir("cached-cjs-esm");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","type":"commonjs","exports":"./index.js"}"#,
            ),
            ("index.js", b"module.exports = { kind: 'cache-cjs' };\n"),
        ]))
        .expect("store dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash, &[]));
    let entry = proj.join("main.mjs");
    std::fs::write(&entry, "import dep from 'dep';\nconsole.log(dep.kind);\n")
        .expect("write entry");

    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("dep", "1.0.0")]), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(
        loader,
        resolver.clone(),
        Some(&cache_root),
        &entry,
        vec![sink_ext],
    )
    .await
    .expect("cached CJS dependency imports");

    assert_eq!(*out.borrow(), "cache-cjs\n");
    assert!(!proj.join("node_modules").exists());
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn extensionless_cached_commonjs_bin_runs_via_native_cjs_runtime() {
    let proj = unique_dir("extensionless-cjs-bin");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"next","version":"1.0.0","type":"commonjs","main":"dist/server/root-main.js","bin":{"next":"dist/bin/next"}}"#,
            ),
            (
                "dist/bin/next",
                b"const hook = require('../server/require-hook');\nconsole.log(hook.value);\n",
            ),
            (
                "dist/server/require-hook.js",
                b"process.env.MEOW_TEST_NEXT = 'yes';\nconst os = require('os');\nconst target = require(require.resolve(__dirname + '/require-target'));\nconst packageRoot = __dirname.slice(0, __dirname.indexOf('/dist/server'));\nconst rootTarget = require(require.resolve(packageRoot));\nexports.value = typeof process + ':' + typeof process.env + ':' + process.env.MEOW_TEST_NEXT + ':' + String(typeof os.tmpdir() === 'string') + ':' + String(global === globalThis) + ':' + typeof performance.now + ':' + typeof TextEncoderStream + ':' + typeof atob + ':' + typeof setInterval + ':' + typeof Event + ':' + target.value + ':' + rootTarget.value;\n",
            ),
            (
                "dist/server/require-target.js",
                b"exports.value = 'hooked';\n",
            ),
            (
                "dist/server/root-main.js",
                b"exports.value = 'root';\n",
            ),
        ]))
        .expect("store dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("next", "1.0.0", dep_hash.clone(), &[]));
    let resolver = resolver_with(
        &cache_root,
        lockfile,
        root_deps(&[("next", "1.0.0")]),
        &proj,
    );
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    let mut rt = Runtime::new(RuntimeOptions {
        module_loader: loader,
        // === RT-007 === cached CommonJS execution needs the Deno package bridge.
        extensions: {
            let mut exts = vec![meow_loader::cjs_resolve_extension(resolver.clone())];
            exts.extend(node_extensions_with_bridge(
                &proj,
                resolver.clone(),
                &cache_root,
            ));
            exts.push(sink_ext);
            exts
        },
        // === /RT-007 ===
    })
    .expect("runtime initializes");
    let spec = encode_cache_url(&dep_hash, "dist/bin/next");
    let ext_resolved = resolver
        .resolve(spec.as_str(), &spec)
        .expect("extensionless cache member resolves");
    assert_eq!(
        ext_resolved.kind,
        ModuleKind::Cjs,
        "extensionless cached member follows package type commonjs"
    );

    rt.run_main_module(&spec)
        .await
        .expect("extensionless CJS bin runs");

    assert_eq!(
        *out.borrow(),
        "object:object:yes:true:true:function:function:function:function:function:hooked:root\n"
    );
    assert!(!proj.join("node_modules").exists());
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_uses_require_export_condition_for_cached_dep() {
    let proj = unique_dir("require-conditions");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","type":"module","exports":{".":{"import":"./esm/index.js","require":"./cjs/index.cjs","default":"./fallback.js"}}}"#,
            ),
            ("esm/index.js", b"export default { kind: 'esm' };\n"),
            ("cjs/index.cjs", b"module.exports = { kind: 'cjs' };\n"),
            ("fallback.js", b"export default { kind: 'fallback' };\n"),
        ]))
        .expect("store dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash, &[]));
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const dep = require('dep');\nconsole.log(dep.kind);\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("dep", "1.0.0")]), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(
        loader,
        resolver.clone(),
        Some(&cache_root),
        &entry,
        vec![sink_ext],
    )
    .await
    .expect("require branch runs");

    assert_eq!(*out.borrow(), "cjs\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_cached_package_self_reference_uses_legacy_resolution_when_exports_miss() {
    let proj = unique_dir("self-reference-legacy-fallback");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let self_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"pkg","version":"1.0.0","type":"commonjs","exports":{"." : "./index.js"}}"#,
            ),
            ("index.js", b"module.exports = require('pkg/internal-or-subpath');\n"),
            ("internal-or-subpath", b"module.exports = 'legacy';\n"),
        ]))
        .expect("store self package");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("pkg", "1.0.0", self_hash, &[]));
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const value = require('pkg');\nconsole.log(value);\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("pkg", "1.0.0")]), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(
        loader,
        resolver.clone(),
        Some(&cache_root),
        &entry,
        vec![sink_ext],
    )
    .await
    .expect("self-ref require reaches legacy fallback");

    assert_eq!(*out.borrow(), "legacy\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn esm_imports_cjs_default_named_and_reassignment() {
    let proj = unique_dir("cjs-named");
    std::fs::write(
        proj.join("legacy.cjs"),
        "exports.foo = 1;\nObject.defineProperty(exports, 'bar', { enumerable: true, value: 2 });\nmodule.exports = { foo: 3, bar: 4, answer: 42 };\n",
    )
    .expect("write cjs");
    let entry = proj.join("main.mjs");
    std::fs::write(
        &entry,
        "import legacy from './legacy.cjs';\nconsole.log(JSON.stringify({ answer: legacy.answer, foo: legacy.foo, bar: legacy.bar }));\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("ESM imports CJS");

    assert_eq!(*out.borrow(), "{\"answer\":42,\"foo\":3,\"bar\":4}\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn circular_commonjs_sees_partial_exports_object() {
    let proj = unique_dir("cjs-cycle");
    std::fs::write(
        proj.join("a.cjs"),
        "exports.done = false;\nconst b = require('./b.cjs');\nexports.seen = b.done;\nexports.done = true;\n",
    )
    .expect("write a");
    std::fs::write(
        proj.join("b.cjs"),
        "exports.done = false;\nconst a = require('./a.cjs');\nexports.seen = a.done;\nexports.done = true;\n",
    )
    .expect("write b");
    let entry = proj.join("main.mjs");
    std::fs::write(
        &entry,
        "import a from './a.cjs';\nimport b from './b.cjs';\nconsole.log(JSON.stringify({ aSeen: a.seen, aDone: a.done, bSeen: b.seen, bDone: b.done }));\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("cycle runs");

    assert_eq!(
        *out.borrow(),
        "{\"aSeen\":true,\"aDone\":true,\"bSeen\":false,\"bDone\":true}\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn repeated_require_returns_the_same_object() {
    let proj = unique_dir("repeat-require");
    std::fs::write(
        proj.join("dep.cjs"),
        "let calls = 0;\nmodule.exports = { next() { calls += 1; return calls; } };\n",
    )
    .expect("write dep");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const a = require('./dep.cjs');\nconst b = require('./dep.cjs');\nconsole.log(JSON.stringify([a === b, a.next(), b.next()]));\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("repeat require runs");

    assert_eq!(*out.borrow(), "[true,1,2]\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn dirname_and_filename_use_real_local_and_unpacked_cached_paths() {
    let proj = unique_dir("dirname-filename");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","type":"commonjs","exports":"./lib/index.cjs"}"#,
            ),
            (
                "lib/index.cjs",
                b"module.exports = { dirname: __dirname, filename: __filename };\n",
            ),
        ]))
        .expect("store dep blob");
    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash.clone(), &[]));

    let local = proj.join("local.cjs");
    std::fs::write(
        &local,
        "module.exports = { dirname: __dirname, filename: __filename };\n",
    )
    .expect("write local");
    let entry = proj.join("main.mjs");
    std::fs::write(
        &entry,
        "import local from './local.cjs';\nimport cached from 'dep';\nconsole.log(JSON.stringify({ local, cached }));\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&cache_root, lockfile, root_deps(&[("dep", "1.0.0")]), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(
        loader,
        resolver.clone(),
        Some(&cache_root),
        &entry,
        vec![sink_ext],
    )
    .await
    .expect("path metadata runs");

    let value: serde_json::Value = serde_json::from_str(out.borrow().trim()).expect("json");
    let cached_root = cache_root.join("unpacked").join(dep_hash.to_url_host());
    assert_eq!(value["local"]["dirname"], proj.to_string_lossy().as_ref());
    assert_eq!(value["local"]["filename"], local.to_string_lossy().as_ref());
    assert_eq!(
        value["cached"]["dirname"],
        cached_root.join("lib").to_string_lossy().as_ref()
    );
    assert_eq!(
        value["cached"]["filename"],
        cached_root
            .join("lib")
            .join("index.cjs")
            .to_string_lossy()
            .as_ref()
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn dynamic_require_resolves_at_runtime() {
    let proj = unique_dir("dynamic-require");
    std::fs::write(proj.join("dep.cjs"), "module.exports = 42;\n").expect("write dep");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const name = './dep.cjs';\nconsole.log(require(name));\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("dynamic require resolves through native CJS");
    assert_eq!(*out.borrow(), "42\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn unresolvable_static_require_falls_through_to_catchable_runtime_error() {
    // LOAD-003: CJS bundles (Next.js, Webpack) routinely wrap optional
    // `require(...)` calls in `try { ... } catch {}` to feature-detect
    // environments. Native CJS resolution keeps that error at the original
    // JavaScript call site, so userland `try/catch` can still handle it.
    let proj = unique_dir("optional-require");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        // `webpack` is never installed; the call is inside try/catch.
        "let mod; try { mod = require('webpack'); } catch (e) { mod = null; }\nconsole.log(mod === null ? 'caught' : 'resolved');\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("optional require wrapped in try/catch must not abort build");

    assert_eq!(
        *out.borrow(),
        "caught\n",
        "try/catch around an unresolvable require runs the catch branch"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn erasable_typescript_commonjs_runs() {
    let proj = unique_dir("ts-cjs");
    std::fs::write(proj.join("dep.cjs"), "module.exports = { value: 42 };\n").expect("write dep");
    let entry = proj.join("main.ts");
    std::fs::write(
        &entry,
        "const dep: { value: number } = require('./dep.cjs');\nconsole.log(dep.value);\nmodule.exports = dep;\n",
    )
    .expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader: Rc<dyn ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    run_entry(loader, resolver.clone(), None, &entry, vec![sink_ext])
        .await
        .expect("TS CommonJS runs");

    assert_eq!(*out.borrow(), "42\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn ts_export_equals_is_an_honest_commonjs_load_error() {
    let proj = unique_dir("ts-export-equals");
    let entry = proj.join("main.ts");
    std::fs::write(&entry, "const value = 1;\nexport = value;\n").expect("write entry");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader = MeowModuleLoader::new(resolver.clone(), Rc::new(RefCell::new(GraphDb::new())));
    let spec = ModuleSpecifier::from_file_path(&entry).expect("file URL");
    let err = load_code(&loader, &spec).expect_err("export = must fail honestly");
    assert!(
        err.contains("export ="),
        "typed strip diagnostic kept: {err}"
    );
    std::fs::remove_dir_all(&proj).ok();
}
