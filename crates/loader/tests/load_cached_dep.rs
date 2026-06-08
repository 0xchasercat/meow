//! Loader behavior tests. Exercise the real resolver + loader + runtime without any
//! `node_modules` tree on disk.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
struct TestCjsResolver(Mutex<Resolver>);

impl node::CjsResolver for TestCjsResolver {
    fn resolve_and_load_cjs(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Result<node::CjsLoadedModule, String> {
        let referrer_url =
            Url::parse(referrer).map_err(|_| format!("invalid CommonJS referrer {referrer}"))?;
        let resolver = self
            .0
            .lock()
            .map_err(|_| "CommonJS resolver lock poisoned".to_owned())?;
        let resolved = resolver
            .resolve_require(specifier, &referrer_url)
            .map_err(|err| err.to_string())?;
        if resolved.kind == meow_loader::ModuleKind::Esm {
            return Err(format!("cannot require ES module {}", resolved.url));
        }
        let filename = resolver
            .runtime_path_for(&resolved.locator)
            .map_err(|err| err.to_string())?
            .to_string_lossy()
            .into_owned();
        let dirname = PathBuf::from(&filename)
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kind = match resolved.kind {
            meow_loader::ModuleKind::Json => "json",
            meow_loader::ModuleKind::Cjs => "cjs",
            meow_loader::ModuleKind::Esm => unreachable!("ESM was rejected above"),
        }
        .to_owned();
        Ok(node::CjsLoadedModule {
            url: resolved.url.to_string(),
            filename,
            dirname,
            source: resolved.source.as_ref().to_owned(),
            kind,
        })
    }
}

// === RT-007 ===
fn node_extensions(resolver: Option<Resolver>) -> Vec<deno_core::Extension> {
    let mut opts = node::NodeOptions::enabled(Vec::new(), PathBuf::from("."));
    opts.cjs_resolver = resolver.map(|resolver| {
        Arc::new(TestCjsResolver(Mutex::new(resolver))) as Arc<dyn node::CjsResolver>
    });
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
    entry: &Path,
    mut extensions: Vec<deno_core::Extension>,
) -> Result<(), meow_runtime::RuntimeError> {
    // === RT-007 ===
    // Every CJS executor pre-injects the full `node:*` built-in set, so the
    // `meow_node` ops must be registered before we evaluate the entry module.
    extensions.extend(node_extensions(Some(resolver)));
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
        // === RT-007 === CJS executors pre-import the `node:*` set, so the
        // meow_node ops have to be live before the entry module evaluates.
        extensions: {
            let mut exts = node_extensions(Some(resolver.clone()));
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
                br#"{"name":"next","version":"1.0.0","type":"commonjs","bin":{"next":"dist/bin/next"}}"#,
            ),
            (
                "dist/bin/next",
                b"Object.defineProperty(exports, '__esModule', { value: true });\nexports.run = 'ok';\nconsole.log(exports.run);\n",
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
        // === RT-007 === see note on the entry above
        extensions: {
            let mut exts = node_extensions(Some(resolver.clone()));
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

    assert_eq!(*out.borrow(), "ok\n");
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
        .await
        .expect("require branch runs");

    assert_eq!(*out.borrow(), "cjs\n");
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
    run_entry(loader, resolver.clone(), &entry, vec![sink_ext])
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
