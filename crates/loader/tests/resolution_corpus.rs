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
use meow_loader::{MeowModuleLoader, ModuleKind, ModuleLocator, ResolveError, Resolver};
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

/// Canonical `file://` URL for a cached member in the unpacked store.
/// removal: the REAL unpacked-store path `<cache>/unpacked/<algo>-<hex>/<member>`.
fn cache_url(proj: &std::path::Path, hash: &meow_pkg::ContentHash, member: &str) -> Url {
    let path = proj
        .join("cache")
        .join("unpacked")
        .join(hash.to_url_host())
        .join(member);
    Url::from_file_path(path).expect("cached member file URL")
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
        meow_runtime::native::native_module_registry(),
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
    assert_eq!(url, cache_url(&proj, &dep_hash, "esm/index.js"));
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
    assert_eq!(exact_url, cache_url(&proj, &pattern_hash, "src/special.js"));

    let (pattern_url, _) = resolver
        .locate("pattern/feat/a", &referrer)
        .expect("pattern subpath resolves");
    assert_eq!(
        pattern_url,
        cache_url(&proj, &pattern_hash, "src/features/a.js")
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
    let referrer = cache_url(&proj, &hash, "index.js");

    let (self_url, _) = resolver
        .locate("selfpkg/util", &referrer)
        .expect("self-reference resolves");
    assert_eq!(self_url, cache_url(&proj, &hash, "src/util.js"));

    let (imports_url, _) = resolver
        .locate("#internal", &referrer)
        .expect("imports map resolves");
    assert_eq!(imports_url, cache_url(&proj, &hash, "src/internal.js"));

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

    let a_referrer = cache_url(&proj, &a_hash, "index.js");
    let c_referrer = cache_url(&proj, &c_hash, "index.js");
    let (from_a, _) = resolver.locate("b", &a_referrer).expect("a resolves b@1");
    let (from_c, _) = resolver.locate("b", &c_referrer).expect("c resolves b@2");
    assert_eq!(from_a, cache_url(&proj, &b1_hash, "index.js"));
    assert_eq!(from_c, cache_url(&proj, &b2_hash, "index.js"));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn phantom_dependency_fallback_selects_highest_locked_version() {
    let proj = unique_dir("phantom");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let owner_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"owner","version":"1.0.0","exports":"./index.js","type":"module","dependencies":{"dep":"1.0.0"}}"#,
            ),
            ("index.js", b"export const owner = true;\n"),
        ]))
        .expect("store owner");
    let dep1_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const version = 'dep-1';\n"),
        ]))
        .expect("store dep1");
    let dep2_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"2.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const version = 'dep-2';\n"),
        ]))
        .expect("store dep2");
    let phantom1_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"phantom","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const version = 'phantom-1';\n"),
        ]))
        .expect("store phantom1");
    let phantom2_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"phantom","version":"2.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const version = 'phantom-2';\n"),
        ]))
        .expect("store phantom2");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dep", "1.0.0", dep1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("dep", "2.0.0", dep2_hash.clone(), &[]));
    lockfile.upsert(lock_entry(
        "owner",
        "1.0.0",
        owner_hash.clone(),
        &[("dep", "1.0.0")],
    ));
    lockfile.upsert(lock_entry("phantom", "1.0.0", phantom1_hash, &[]));
    lockfile.upsert(lock_entry("phantom", "2.0.0", phantom2_hash.clone(), &[]));
    let resolver = resolver_with(&proj, cache, lockfile, root_deps(&[("owner", "1.0.0")]));
    let owner_referrer = cache_url(&proj, &owner_hash, "index.js");

    let (explicit_url, _) = resolver
        .locate("dep", &owner_referrer)
        .expect("explicit owner dependency resolves");
    assert_eq!(explicit_url, cache_url(&proj, &dep1_hash, "index.js"));

    let (phantom_url, _) = resolver
        .locate("phantom", &owner_referrer)
        .expect("phantom dependency fallback resolves");
    assert_eq!(phantom_url, cache_url(&proj, &phantom2_hash, "index.js"));

    let missing = resolver
        .locate("missing", &owner_referrer)
        .expect_err("missing phantom still errors");
    assert!(matches!(
        missing,
        ResolveError::BareSpecifierNotInLockfile { .. }
    ));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn explicit_dependency_export_miss_can_rescue_with_highest_locked_version() {
    let proj = unique_dir("phantom-export");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let owner_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"owner","version":"1.0.0","exports":"./index.js","type":"module","dependencies":{"helper":"1.0.0"}}"#,
            ),
            ("index.js", b"export const owner = true;\n"),
        ]))
        .expect("store owner");
    let helper1_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"helper","version":"1.0.0","exports":{".":"./index.js"},"type":"module"}"#,
            ),
            ("index.js", b"export const root = 'helper-1';\n"),
        ]))
        .expect("store helper1");
    let helper2_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"helper","version":"2.0.0","exports":{".":"./index.js","./markdown":"./markdown.js"},"type":"module"}"#,
            ),
            ("index.js", b"export const root = 'helper-2';\n"),
            ("markdown.js", b"export const markdown = true;\n"),
        ]))
        .expect("store helper2");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("helper", "1.0.0", helper1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("helper", "2.0.0", helper2_hash.clone(), &[]));
    lockfile.upsert(lock_entry(
        "owner",
        "1.0.0",
        owner_hash.clone(),
        &[("helper", "1.0.0")],
    ));
    let resolver = resolver_with(&proj, cache, lockfile, root_deps(&[("owner", "1.0.0")]));
    let owner_referrer = cache_url(&proj, &owner_hash, "index.js");

    let (explicit_root_url, _) = resolver
        .locate("helper", &owner_referrer)
        .expect("explicit root dependency still wins");
    assert_eq!(
        explicit_root_url,
        cache_url(&proj, &helper1_hash, "index.js")
    );

    let (rescued_subpath_url, _) = resolver
        .locate("helper/markdown", &owner_referrer)
        .expect("export miss rescues through highest locked version");
    assert_eq!(
        rescued_subpath_url,
        cache_url(&proj, &helper2_hash, "markdown.js")
    );

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn projected_pnpm_referrer_recovers_cached_owner_for_package_imports() {
    let proj = unique_dir("projected-owner");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let starlight_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br##"{"name":"@astrojs/starlight","version":"0.39.3","type":"module","exports":"./index.js","imports":{"#import-plugin":"./integrations/import-plugin.js"}}"##,
            ),
            ("index.js", b"export const starlight = true;\n"),
            ("integrations/remark-rehype.js", b"import plugin from '#import-plugin';\nexport default plugin;\n"),
            ("integrations/import-plugin.js", b"export default function plugin() {}\n"),
        ]))
        .expect("store starlight");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "@astrojs/starlight",
        "0.39.3",
        starlight_hash.clone(),
        &[],
    ));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[("@astrojs/starlight", "0.39.3")]),
    );
    let projected_root = proj
        .join("node_modules")
        .join(".pnpm")
        .join("@astrojs+starlight@0.39.3_astro@6.4.8")
        .join("node_modules")
        .join("@astrojs")
        .join("starlight");
    std::fs::create_dir_all(projected_root.join("integrations")).expect("create projected tree");
    std::fs::write(
        projected_root.join("package.json"),
        br##"{"name":"@astrojs/starlight","version":"0.39.3","type":"module","exports":"./index.js","imports":{"#import-plugin":"./integrations/import-plugin.js"}}"##,
    )
    .expect("write projected manifest");
    std::fs::write(
        projected_root.join("integrations/remark-rehype.js"),
        b"import plugin from '#import-plugin';\nexport default plugin;\n",
    )
    .expect("write projected referrer");

    let referrer = Url::from_file_path(projected_root.join("integrations/remark-rehype.js"))
        .expect("projected referrer URL");
    let (url, locator) = resolver
        .locate("#import-plugin", &referrer)
        .expect("package import resolves against projected package owner");
    assert_eq!(
        url,
        cache_url(&proj, &starlight_hash, "integrations/import-plugin.js")
    );
    assert!(matches!(
        locator,
        ModuleLocator::Cached { package, member }
            if package == starlight_hash && member == "integrations/import-plugin.js"
    ));

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn projected_path_prefers_exact_pnpm_entry_over_stale_top_level_alias() {
    let proj = unique_dir("pnpm-projection");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let workerd_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"workerd","version":"1.20260623.1","main":"lib/main.js"}"#,
            ),
            ("lib/main.js", b"module.exports = 'fresh';\n"),
        ]))
        .expect("store workerd");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "workerd",
        "1.20260623.1",
        workerd_hash.clone(),
        &[],
    ));
    let resolver = resolver_with(
        &proj,
        cache,
        lockfile,
        root_deps(&[("workerd", "1.20260623.1")]),
    );

    let stale_root = proj.join("node_modules").join("workerd");
    std::fs::create_dir_all(stale_root.join("lib")).expect("create stale alias");
    std::fs::write(
        stale_root.join("package.json"),
        br#"{"name":"workerd","version":"1.20260609.1","main":"lib/main.js"}"#,
    )
    .expect("write stale manifest");
    std::fs::write(
        stale_root.join("lib/main.js"),
        b"module.exports = 'stale';\n",
    )
    .expect("write stale main");

    let pnpm_root = proj
        .join("node_modules")
        .join(".pnpm")
        .join("workerd@1.20260623.1")
        .join("node_modules")
        .join("workerd");
    std::fs::create_dir_all(pnpm_root.join("lib")).expect("create pnpm package");
    std::fs::write(
        pnpm_root.join("package.json"),
        br#"{"name":"workerd","version":"1.20260623.1","main":"lib/main.js"}"#,
    )
    .expect("write pnpm manifest");
    std::fs::write(
        pnpm_root.join("lib/main.js"),
        b"module.exports = 'fresh';\n",
    )
    .expect("write pnpm main");

    let projected = resolver
        .projected_path_for(&ModuleLocator::Cached {
            package: workerd_hash,
            member: "lib/main.js".to_owned(),
        })
        .expect("exact pnpm projection exists");
    assert_eq!(projected, pnpm_root.join("lib/main.js"));

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
    assert_eq!(legacy_url, cache_url(&proj, &legacy_hash, "lib/index.js"));

    let (ext_sub_url, _) = resolver
        .locate("ext/sub", &referrer)
        .expect("ext sub resolves");
    assert_eq!(ext_sub_url, cache_url(&proj, &ext_hash, "sub.js"));

    let (ext_dir_url, _) = resolver
        .locate("ext/dir", &referrer)
        .expect("ext dir resolves");
    assert_eq!(ext_dir_url, cache_url(&proj, &ext_hash, "dir/index.js"));

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

    let cjs_code = load_result(&loader, &cjs_spec).expect("cjs load succeeds");
    assert!(
        cjs_code.contains("__meowCjsExports"),
        "cjs load delegates to the native CJS runtime shim: {cjs_code}"
    );

    assert!(
        !proj.join("node_modules").exists(),
        "no node_modules is created"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn legacy_package_module_field_is_import_only_entrypoint() {
    let proj = unique_dir("legacy-module-field");
    let cache = Arc::new(Cache::with_root(proj.join("cache")));
    let dual_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"dual","version":"1.0.0","main":"./cjs/index.cjs","module":"./esm/index.js","type":"module"}"#,
            ),
            ("cjs/index.cjs", b"module.exports = { AttributeAction: 'cjs' };\n"),
            ("esm/index.js", b"export const AttributeAction = 'esm';\n"),
        ]))
        .expect("store dual package");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("dual", "1.0.0", dual_hash.clone(), &[]));
    let resolver = resolver_with(&proj, cache, lockfile, root_deps(&[("dual", "1.0.0")]));
    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");

    let imported = resolver
        .resolve("dual", &referrer)
        .expect("import resolves");
    assert_eq!(imported.url, cache_url(&proj, &dual_hash, "esm/index.js"));
    assert_eq!(imported.kind, ModuleKind::Esm);

    let required = resolver
        .resolve_require("dual", &referrer)
        .expect("require resolves");
    assert_eq!(required.url, cache_url(&proj, &dual_hash, "cjs/index.cjs"));
    assert_eq!(required.kind, ModuleKind::Cjs);

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
