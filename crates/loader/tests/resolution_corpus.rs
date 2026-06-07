use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deno_core::url::Url;
use deno_core::{
    ModuleLoadOptions, ModuleLoadResponse, ModuleLoader, ModuleSourceCode, ModuleSpecifier,
    ModuleType, RequestedModuleType,
};
use meow_graph::GraphDb;
use meow_loader::{
    encode_cache_url, MeowModuleLoader, ModuleKind, ModuleLocator, ResolveError, Resolver,
};
use meow_pkg::{Cache, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq};

fn unique_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "meow-loader-corpus-{tag}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
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
        version: parsed_version(version),
        integrity,
        dependencies: deps
            .iter()
            .map(|(dep, ver)| (PackageName::new((*dep).to_owned()), parsed_version(ver)))
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
            .expect("append member");
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

fn resolver_with(
    project_root: &Path,
    cache: Arc<Cache>,
    lockfile: Lockfile,
    root_deps: BTreeMap<PackageName, Version>,
) -> Resolver {
    Resolver::new(
        cache,
        Arc::new(lockfile),
        root_deps,
        Url::from_directory_path(project_root).expect("project root URL"),
    )
}

fn sync_options() -> ModuleLoadOptions {
    ModuleLoadOptions {
        is_dynamic_import: false,
        is_synchronous: true,
        requested_module_type: RequestedModuleType::None,
    }
}

fn load_result(loader: &MeowModuleLoader, spec: &ModuleSpecifier) -> Result<String, String> {
    match loader.load(spec, None, sync_options()) {
        ModuleLoadResponse::Sync(Ok(source)) => match source.code {
            ModuleSourceCode::String(code) => Ok(code.as_str().to_owned()),
            ModuleSourceCode::Bytes(_) => panic!("expected string code"),
        },
        ModuleLoadResponse::Sync(Err(err)) => Err(err.to_string()),
        _ => panic!("expected synchronous load"),
    }
}

#[test]
fn conditional_exports_choose_import_and_require_only_errors() {
    let proj = unique_dir("conditions");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let dep_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","type":"module","exports":{".":{"import":"./esm/index.js","require":"./cjs/index.cjs","default":"./fallback.js"}}}"#,
            ),
            ("esm/index.js", b"export const value = 'esm';\n"),
            ("cjs/index.cjs", b"module.exports = { value: 'cjs' };\n"),
            ("fallback.js", b"export const value = 'fallback';\n"),
        ]))
        .expect("store dep");
    let require_only_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"reqonly","version":"1.0.0","exports":{".":{"require":"./index.cjs"}}}"#,
            ),
            ("index.cjs", b"module.exports = 1;\n"),
        ]))
        .expect("store reqonly");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash.clone(), &[]));
    lockfile.upsert(lock_entry("reqonly", "1.0.0", require_only_hash, &[]));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[("dep", "1.0.0"), ("reqonly", "1.0.0")]),
    );
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (url, locator) = resolver.locate("dep", &referrer).expect("dep resolves");
    assert_eq!(url, encode_cache_url(&dep_hash, "esm/index.js"));
    assert!(matches!(
        locator,
        ModuleLocator::Cached { package, member } if package == dep_hash && member == "esm/index.js"
    ));

    let err = resolver
        .locate("reqonly", &referrer)
        .expect_err("require-only package errors in import context");
    assert!(matches!(
        err,
        ResolveError::NoMatchingCondition { tried, .. } if tried == vec!["meow", "import", "node", "default"]
    ));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn subpath_patterns_exact_wins_and_invalid_targets_are_typed() {
    let proj = unique_dir("patterns");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let pattern_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"pattern","version":"1.0.0","exports":{"./feat/special":"./src/special.js","./feat/*":"./src/features/*.js"},"type":"module"}"#,
            ),
            ("src/special.js", b"export const value = 'special';\n"),
            ("src/features/a.js", b"export const value = 'a';\n"),
        ]))
        .expect("store pattern");
    let invalid_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"invalid","version":"1.0.0","exports":{"./feat/*":"../escape/*.js"}}"#,
            ),
            ("escape/a.js", b"export const value = 'nope';\n"),
        ]))
        .expect("store invalid");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("invalid", "1.0.0", invalid_hash, &[]));
    lockfile.upsert(lock_entry("pattern", "1.0.0", pattern_hash.clone(), &[]));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[("pattern", "1.0.0"), ("invalid", "1.0.0")]),
    );
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (exact_url, _) = resolver
        .locate("pattern/feat/special", &referrer)
        .expect("exact subpath resolves");
    assert_eq!(exact_url, encode_cache_url(&pattern_hash, "src/special.js"));

    let (pattern_url, _) = resolver
        .locate("pattern/feat/a", &referrer)
        .expect("pattern subpath resolves");
    assert_eq!(
        pattern_url,
        encode_cache_url(&pattern_hash, "src/features/a.js")
    );

    let err = resolver
        .locate("invalid/feat/a", &referrer)
        .expect_err("escaping target errors");
    assert!(matches!(err, ResolveError::InvalidPackageTarget { .. }));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn self_reference_imports_and_encapsulation_follow_package_maps() {
    let proj = unique_dir("selfref");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let hash = cache
        .store(&archive(&[
            (
                "package.json",
                br##"{"name":"selfpkg","version":"1.0.0","exports":{".":"./index.js","./util":"./src/util.js"},"imports":{"#internal":"./src/internal.js"},"type":"module"}"##,
            ),
            ("index.js", b"export const root = 1;\n"),
            ("src/util.js", b"export const util = 1;\n"),
            ("src/internal.js", b"export const internal = 1;\n"),
            ("src/hidden.js", b"export const hidden = 1;\n"),
        ]))
        .expect("store self package");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("selfpkg", "1.0.0", hash.clone(), &[]));
    let resolver = resolver_with(&proj, cache, lockfile, root_deps(&[("selfpkg", "1.0.0")]));
    let referrer = encode_cache_url(&hash, "index.js");

    let (self_url, _) = resolver
        .locate("selfpkg/util", &referrer)
        .expect("self-reference resolves");
    assert_eq!(self_url, encode_cache_url(&hash, "src/util.js"));

    let (imports_url, _) = resolver
        .locate("#internal", &referrer)
        .expect("imports map resolves");
    assert_eq!(imports_url, encode_cache_url(&hash, "src/internal.js"));

    let missing_import = resolver
        .locate("#missing", &referrer)
        .expect_err("missing imports key errors");
    assert!(matches!(
        missing_import,
        ResolveError::ImportNotDefined { .. }
    ));

    let missing_self = resolver
        .locate("selfpkg/missing", &referrer)
        .expect_err("unexported self subpath errors");
    assert!(matches!(
        missing_self,
        ResolveError::SubpathNotExported { .. }
    ));

    let encapsulated = resolver
        .locate("selfpkg/src/hidden.js", &referrer)
        .expect_err("exports map is authoritative");
    assert!(matches!(
        encapsulated,
        ResolveError::SubpathNotExported { .. }
    ));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn nested_dependencies_and_multi_version_follow_owner_lock_entries() {
    let proj = unique_dir("nested");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let a_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"a","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const fromA = true;\n"),
        ]))
        .expect("store a");
    let c_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"c","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const fromC = true;\n"),
        ]))
        .expect("store c");
    let b1_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const version = '1';\n"),
        ]))
        .expect("store b1");
    let b2_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"2.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const version = '2';\n"),
        ]))
        .expect("store b2");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[("a", "1.0.0"), ("c", "1.0.0")]),
    );

    let a_referrer = encode_cache_url(&a_hash, "index.js");
    let c_referrer = encode_cache_url(&c_hash, "index.js");
    let (from_a, _) = resolver.locate("b", &a_referrer).expect("a resolves b@1");
    let (from_c, _) = resolver.locate("b", &c_referrer).expect("c resolves b@2");
    assert_eq!(from_a, encode_cache_url(&b1_hash, "index.js"));
    assert_eq!(from_c, encode_cache_url(&b2_hash, "index.js"));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn legacy_main_extensionless_json_and_cjs_boundary_work_end_to_end() {
    let proj = unique_dir("formats");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let legacy_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"legacy","version":"1.0.0","main":"lib/index.js","type":"module"}"#,
            ),
            ("lib/index.js", b"export const legacy = 'legacy';\n"),
        ]))
        .expect("store legacy");
    let ext_hash = cache
        .store(&archive(&[
            ("package.json", br#"{"name":"ext","version":"1.0.0"}"#),
            ("sub.js", b"module.exports = 1;\n"),
            ("dir/index.js", b"module.exports = 2;\n"),
        ]))
        .expect("store ext");
    let json_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"jsonpkg","version":"1.0.0","exports":"./data.json"}"#,
            ),
            ("data.json", br#"{"answer":42}"#),
        ]))
        .expect("store jsonpkg");
    let cjs_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"cjspkg","version":"1.0.0","exports":"./mod.js"}"#,
            ),
            ("mod.js", b"module.exports = { value: 1 };\n"),
        ]))
        .expect("store cjspkg");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("cjspkg", "1.0.0", cjs_hash.clone(), &[]));
    lockfile.upsert(lock_entry("ext", "1.0.0", ext_hash.clone(), &[]));
    lockfile.upsert(lock_entry("jsonpkg", "1.0.0", json_hash.clone(), &[]));
    lockfile.upsert(lock_entry("legacy", "1.0.0", legacy_hash.clone(), &[]));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[
            ("legacy", "1.0.0"),
            ("ext", "1.0.0"),
            ("jsonpkg", "1.0.0"),
            ("cjspkg", "1.0.0"),
        ]),
    );
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let (legacy_url, _) = resolver
        .locate("legacy", &referrer)
        .expect("legacy main resolves");
    assert_eq!(legacy_url, encode_cache_url(&legacy_hash, "lib/index.js"));

    let (ext_sub_url, _) = resolver
        .locate("ext/sub", &referrer)
        .expect("ext sub resolves");
    assert_eq!(ext_sub_url, encode_cache_url(&ext_hash, "sub.js"));

    let (ext_dir_url, _) = resolver
        .locate("ext/dir", &referrer)
        .expect("ext dir resolves");
    assert_eq!(ext_dir_url, encode_cache_url(&ext_hash, "dir/index.js"));

    let json_spec = resolver
        .locate("jsonpkg", &referrer)
        .expect("json locate")
        .0;
    let json_resolved = resolver
        .resolve("jsonpkg", &referrer)
        .expect("json resolves");
    assert_eq!(json_resolved.kind, ModuleKind::Json);

    let cjs_spec = resolver.locate("cjspkg", &referrer).expect("cjs locate").0;
    let cjs_resolved = resolver.resolve("cjspkg", &referrer).expect("cjs resolves");
    assert_eq!(cjs_resolved.kind, ModuleKind::Cjs);

    let graph = Rc::new(RefCell::new(GraphDb::new()));
    let loader = MeowModuleLoader::new(resolver, graph);
    match loader.load(&json_spec, None, sync_options()) {
        ModuleLoadResponse::Sync(Ok(source)) => {
            assert_eq!(source.module_type, ModuleType::Json);
            match source.code {
                ModuleSourceCode::String(code) => assert_eq!(code.as_str(), "{\"answer\":42}"),
                ModuleSourceCode::Bytes(_) => panic!("expected string json source"),
            }
        }
        ModuleLoadResponse::Sync(Err(err)) => panic!("json load failed: {err}"),
        _ => panic!("expected synchronous json load"),
    }

    let cjs_err = load_result(&loader, &cjs_spec).expect_err("cjs load is refused");
    assert!(
        cjs_err.contains("CommonJS"),
        "cjs boundary is honest: {cjs_err}"
    );

    assert!(
        !proj.join("node_modules").exists(),
        "no node_modules is created"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn malformed_archives_manifests_and_integrity_mismatches_are_typed() {
    let proj = unique_dir("robustness");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let malformed_hash = cache
        .store(&archive(&[
            ("package.json", b"{not json"),
            ("index.js", b"export const broken = true;\n"),
        ]))
        .expect("store malformed manifest package");
    let archive_hash = cache
        .store(b"not a gzip tarball")
        .expect("store invalid archive bytes");
    let integrity_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"integrity","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const ok = true;\n"),
        ]))
        .expect("store integrity package");
    std::fs::write(cache.path_for(&integrity_hash), b"tampered").expect("tamper cache blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("archive", "1.0.0", archive_hash, &[]));
    lockfile.upsert(lock_entry("broken", "1.0.0", malformed_hash, &[]));
    lockfile.upsert(lock_entry("integrity", "1.0.0", integrity_hash, &[]));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[
            ("archive", "1.0.0"),
            ("broken", "1.0.0"),
            ("integrity", "1.0.0"),
        ]),
    );
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let malformed = resolver
        .locate("broken", &referrer)
        .expect_err("malformed manifest errors");
    assert!(matches!(malformed, ResolveError::InvalidManifest { .. }));

    let bad_archive = resolver
        .locate("archive", &referrer)
        .expect_err("bad archive errors");
    assert!(matches!(bad_archive, ResolveError::InvalidArchive { .. }));

    let integrity = resolver
        .locate("integrity", &referrer)
        .expect_err("integrity mismatch errors");
    assert!(matches!(
        integrity,
        ResolveError::Cache(meow_pkg::CacheError::IntegrityMismatch { .. })
    ));

    std::fs::remove_dir_all(&proj).ok();
}
