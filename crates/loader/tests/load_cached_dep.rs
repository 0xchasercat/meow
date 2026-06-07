//! Loader behavior tests. Exercise the real resolver + loader + runtime without any
//! `node_modules` tree on disk.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deno_core::url::Url;
use deno_core::{
    ModuleLoadOptions, ModuleLoadResponse, ModuleLoader, ModuleSourceCode, ModuleSpecifier,
    RequestedModuleType,
};
use meow_graph::GraphDb;
use meow_loader::{MeowModuleLoader, ModuleLocator, ResolveError, Resolver};
use meow_pkg::{
    Cache, CacheError, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
};
use meow_runtime::{print_sink_extension, PrintSink, Runtime, RuntimeOptions};

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
    let loader = Rc::new(MeowModuleLoader::new(resolver, graph));

    let (out, sink_ext) = capture();
    let mut rt = Runtime::new(RuntimeOptions {
        module_loader: loader,
        extensions: vec![sink_ext],
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
fn ts_source_is_stripped_on_load() {
    let proj = unique_dir("strip");
    let entry = proj.join("typed.ts");
    std::fs::write(&entry, "export const n: number = 41 + 1;\n").expect("write");

    let resolver = resolver_with(&proj.join("cache"), Lockfile::new(), BTreeMap::new(), &proj);
    let loader = MeowModuleLoader::new(resolver, Rc::new(RefCell::new(GraphDb::new())));

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
    let loader = MeowModuleLoader::new(resolver, Rc::new(RefCell::new(GraphDb::new())));

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
