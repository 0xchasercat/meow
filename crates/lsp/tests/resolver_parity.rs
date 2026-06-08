mod support;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::url::Url;
use deno_core::{ModuleLoader, ResolutionKind};
use meow_graph::GraphDb;
use meow_loader::{encode_cache_url, ModuleKind, ResolveError, Resolver};
use meow_lsp::{EditorResolver, ShadowDeps};
use meow_pkg::{Cache, ContentHash, Lockfile, ResolutionGraph, UnpackedStore};

use support::{archive, dir_url, load_result, locator_key, lock_entry, root_deps, unique_dir};

struct Fixture {
    project: PathBuf,
    cache: Arc<Cache>,
    graph: Arc<ResolutionGraph>,
    dep_hash: ContentHash,
    scoped_hash: ContentHash,
    pattern_hash: ContentHash,
    self_hash: ContentHash,
    a_hash: ContentHash,
    b1_hash: ContentHash,
    c_hash: ContentHash,
    b2_hash: ContentHash,
    cjs_hash: ContentHash,
}

fn fixture() -> Fixture {
    let project = unique_dir("resolver-parity");
    let cache = Arc::new(Cache::with_root(project.join("cache")));

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
    let reqonly_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"reqonly","version":"1.0.0","exports":{".":{"require":"./index.cjs"}}}"#,
            ),
            ("index.cjs", b"module.exports = 1;\n"),
        ]))
        .expect("store reqonly");
    let scoped_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"@scope/name","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const scoped = true;\n"),
        ]))
        .expect("store scoped");
    let pattern_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"pattern","version":"1.0.0","exports":{"./feat/*":"./src/features/*.js"},"type":"module"}"#,
            ),
            ("src/features/a.js", b"export const feature = 'a';\n"),
        ]))
        .expect("store pattern");
    let self_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br##"{"name":"selfpkg","version":"1.0.0","exports":{".":"./index.js","./util":"./src/util.js"},"imports":{"#internal":"./src/internal.js"},"type":"module"}"##,
            ),
            ("index.js", b"export const root = true;\n"),
            ("src/util.js", b"export const util = true;\n"),
            ("src/internal.js", b"export const internal = true;\n"),
            ("src/rel.js", b"export const rel = true;\n"),
        ]))
        .expect("store self package");
    let a_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"a","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const fromA = true;\n"),
        ]))
        .expect("store a");
    let b1_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const value = 'b1';\n"),
        ]))
        .expect("store b1");
    let c_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"c","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const fromC = true;\n"),
        ]))
        .expect("store c");
    let b2_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"2.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const value = 'b2';\n"),
        ]))
        .expect("store b2");
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
    lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash.clone(), &[]));
    lockfile.upsert(lock_entry("reqonly", "1.0.0", reqonly_hash, &[]));
    lockfile.upsert(lock_entry("@scope/name", "1.0.0", scoped_hash.clone(), &[]));
    lockfile.upsert(lock_entry("pattern", "1.0.0", pattern_hash.clone(), &[]));
    lockfile.upsert(lock_entry("selfpkg", "1.0.0", self_hash.clone(), &[]));
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));
    lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    lockfile.upsert(lock_entry("cjspkg", "1.0.0", cjs_hash.clone(), &[]));

    let graph = Arc::new(
        ResolutionGraph::assemble(
            Arc::new(lockfile),
            root_deps(&[
                ("dep", "1.0.0"),
                ("reqonly", "1.0.0"),
                ("@scope/name", "1.0.0"),
                ("pattern", "1.0.0"),
                ("selfpkg", "1.0.0"),
                ("a", "1.0.0"),
                ("c", "1.0.0"),
                ("cjspkg", "1.0.0"),
            ]),
        )
        .expect("assemble graph"),
    );
    graph.verify_cached(&cache).expect("verify cache presence");

    Fixture {
        project,
        cache,
        graph,
        dep_hash,
        scoped_hash,
        pattern_hash,
        self_hash,
        a_hash,
        b1_hash,
        c_hash,
        b2_hash,
        cjs_hash,
    }
}

#[test]
fn runtime_and_editor_share_resolution_corpus() {
    let fixture = fixture();
    let native = meow_runtime::native::native_module_registry();
    let runtime_resolver = Resolver::from_resolution(
        &fixture.graph,
        fixture.cache.clone(),
        dir_url(&fixture.project),
        native.clone(),
    );
    let loader = meow_loader::MeowModuleLoader::new(
        Resolver::from_resolution(
            &fixture.graph,
            fixture.cache.clone(),
            dir_url(&fixture.project),
            native.clone(),
        ),
        Rc::new(RefCell::new(GraphDb::new())),
    );
    let store = UnpackedStore::new(fixture.project.join("unpacked"), fixture.cache.clone());
    let editor = EditorResolver::new(
        &fixture.graph,
        fixture.cache.clone(),
        dir_url(&fixture.project),
        native,
        store,
    );

    let project_referrer =
        Url::from_file_path(fixture.project.join("main.ts")).expect("project referrer");
    let self_referrer = encode_cache_url(&fixture.self_hash, "index.js");
    let a_referrer = encode_cache_url(&fixture.a_hash, "index.js");
    let c_referrer = encode_cache_url(&fixture.c_hash, "index.js");
    let cases = vec![
        (
            "conditional export import",
            "dep",
            project_referrer.clone(),
            encode_cache_url(&fixture.dep_hash, "esm/index.js"),
            format!("cache:{}:esm/index.js", fixture.dep_hash.to_sri()),
        ),
        (
            "scoped package",
            "@scope/name",
            project_referrer.clone(),
            encode_cache_url(&fixture.scoped_hash, "index.js"),
            format!("cache:{}:index.js", fixture.scoped_hash.to_sri()),
        ),
        (
            "pattern subpath",
            "pattern/feat/a",
            project_referrer.clone(),
            encode_cache_url(&fixture.pattern_hash, "src/features/a.js"),
            format!("cache:{}:src/features/a.js", fixture.pattern_hash.to_sri()),
        ),
        (
            "self reference",
            "selfpkg/util",
            self_referrer.clone(),
            encode_cache_url(&fixture.self_hash, "src/util.js"),
            format!("cache:{}:src/util.js", fixture.self_hash.to_sri()),
        ),
        (
            "relative in cached package",
            "./src/rel.js",
            self_referrer.clone(),
            encode_cache_url(&fixture.self_hash, "src/rel.js"),
            format!("cache:{}:src/rel.js", fixture.self_hash.to_sri()),
        ),
        (
            "internal imports map",
            "#internal",
            self_referrer,
            encode_cache_url(&fixture.self_hash, "src/internal.js"),
            format!("cache:{}:src/internal.js", fixture.self_hash.to_sri()),
        ),
        (
            "nested b from a",
            "b",
            a_referrer,
            encode_cache_url(&fixture.b1_hash, "index.js"),
            format!("cache:{}:index.js", fixture.b1_hash.to_sri()),
        ),
        (
            "nested b from c",
            "b",
            c_referrer,
            encode_cache_url(&fixture.b2_hash, "index.js"),
            format!("cache:{}:index.js", fixture.b2_hash.to_sri()),
        ),
    ];

    for (label, specifier, referrer, expected_url, expected_locator) in cases {
        let runtime_url = loader
            .resolve(specifier, referrer.as_str(), ResolutionKind::Import)
            .unwrap_or_else(|err| panic!("{label}: runtime resolve failed: {err}"));
        let (runtime_locate_url, runtime_locator) = runtime_resolver
            .locate(specifier, &referrer)
            .unwrap_or_else(|err| panic!("{label}: runtime locate failed: {err}"));
        let editor_resolved = editor
            .resolve_module(specifier, &referrer)
            .unwrap_or_else(|err| panic!("{label}: editor resolve failed: {err}"));

        assert_eq!(runtime_url, expected_url, "{label}: runtime url");
        assert_eq!(
            runtime_locate_url, expected_url,
            "{label}: runtime locate url"
        );
        assert_eq!(editor_resolved.url, expected_url, "{label}: editor url");
        assert_eq!(runtime_url, editor_resolved.url, "{label}: url parity");
        assert_eq!(
            locator_key(&runtime_locator),
            expected_locator,
            "{label}: runtime locator"
        );
        assert_eq!(
            locator_key(&editor_resolved.locator),
            expected_locator,
            "{label}: editor locator"
        );
        assert_eq!(
            locator_key(&runtime_locator),
            locator_key(&editor_resolved.locator),
            "{label}: locator parity"
        );
        assert_eq!(
            runtime_url.scheme(),
            "meow-cache",
            "{label}: cache URL scheme"
        );
        assert!(
            !runtime_url.as_str().contains("node_modules"),
            "{label}: no node_modules in resolved URL"
        );
    }

    let shadow = ShadowDeps::generate(&fixture.project, &fixture.graph, &editor_store(&fixture))
        .expect("generate shadow deps");
    assert!(shadow.root().ends_with(".meow/deps"));
    assert!(!fixture.project.join("node_modules").exists());

    std::fs::remove_dir_all(&fixture.project).ok();
}

#[test]
fn runtime_and_editor_share_locate_time_error_variants() {
    let fixture = fixture();
    let native = meow_runtime::native::native_module_registry();
    let runtime = Resolver::from_resolution(
        &fixture.graph,
        fixture.cache.clone(),
        dir_url(&fixture.project),
        native.clone(),
    );
    let editor = EditorResolver::new(
        &fixture.graph,
        fixture.cache.clone(),
        dir_url(&fixture.project),
        native,
        editor_store(&fixture),
    );
    let project_referrer =
        Url::from_file_path(fixture.project.join("main.ts")).expect("project referrer");
    let self_referrer = encode_cache_url(&fixture.self_hash, "index.js");

    let runtime_missing = runtime
        .locate("missing", &project_referrer)
        .expect_err("missing dep must error");
    let editor_missing = editor
        .resolve_module("missing", &project_referrer)
        .expect_err("missing dep must error");
    assert_same_variant(&runtime_missing, &editor_missing, "missing dep");
    assert!(matches!(
        runtime_missing,
        ResolveError::BareSpecifierNotInLockfile { .. }
    ));
    assert!(matches!(
        editor_missing,
        ResolveError::BareSpecifierNotInLockfile { .. }
    ));

    let runtime_reqonly = runtime
        .locate("reqonly", &project_referrer)
        .expect_err("require-only dep must error");
    let editor_reqonly = editor
        .resolve_module("reqonly", &project_referrer)
        .expect_err("require-only dep must error");
    assert_same_variant(&runtime_reqonly, &editor_reqonly, "require-only dep");
    assert!(matches!(
        runtime_reqonly,
        ResolveError::NoMatchingCondition { ref tried, .. } if tried == &vec!["meow", "import", "node", "default"]
    ));
    assert!(matches!(
        editor_reqonly,
        ResolveError::NoMatchingCondition { ref tried, .. } if tried == &vec!["meow", "import", "node", "default"]
    ));

    let runtime_hidden = runtime
        .locate("selfpkg/missing", &self_referrer)
        .expect_err("missing self export must error");
    let editor_hidden = editor
        .resolve_module("selfpkg/missing", &self_referrer)
        .expect_err("missing self export must error");
    assert_same_variant(&runtime_hidden, &editor_hidden, "missing self export");
    assert!(matches!(
        runtime_hidden,
        ResolveError::SubpathNotExported { .. }
    ));
    assert!(matches!(
        editor_hidden,
        ResolveError::SubpathNotExported { .. }
    ));

    let runtime_native = runtime
        .locate("meow:missing", &project_referrer)
        .expect_err("unknown native must error");
    let editor_native = editor
        .resolve_module("meow:missing", &project_referrer)
        .expect_err("unknown native must error");
    assert_same_variant(&runtime_native, &editor_native, "unknown native");
    assert!(matches!(
        runtime_native,
        ResolveError::UnknownNativeModule { .. }
    ));
    assert!(matches!(
        editor_native,
        ResolveError::UnknownNativeModule { .. }
    ));

    std::fs::remove_dir_all(&fixture.project).ok();
}

#[test]
fn runtime_load_accepts_cjs_after_shared_resolution() {
    let fixture = fixture();
    let native = meow_runtime::native::native_module_registry();
    let runtime_resolver = Resolver::from_resolution(
        &fixture.graph,
        fixture.cache.clone(),
        dir_url(&fixture.project),
        native.clone(),
    );
    let loader = meow_loader::MeowModuleLoader::new(
        Resolver::from_resolution(
            &fixture.graph,
            fixture.cache.clone(),
            dir_url(&fixture.project),
            native.clone(),
        ),
        Rc::new(RefCell::new(GraphDb::new())),
    );
    let editor = EditorResolver::new(
        &fixture.graph,
        fixture.cache.clone(),
        dir_url(&fixture.project),
        native,
        editor_store(&fixture),
    );
    let project_referrer =
        Url::from_file_path(fixture.project.join("main.ts")).expect("project referrer");

    let runtime_url = loader
        .resolve("cjspkg", project_referrer.as_str(), ResolutionKind::Import)
        .expect("runtime locate CJS package");
    let (runtime_locate_url, runtime_locator) = runtime_resolver
        .locate("cjspkg", &project_referrer)
        .expect("runtime locate CJS package");
    let editor_resolved = editor
        .resolve_module("cjspkg", &project_referrer)
        .expect("editor locate CJS package");
    let runtime_kind = runtime_resolver
        .resolve("cjspkg", &project_referrer)
        .expect("runtime resolve CJS package")
        .kind;

    assert_eq!(runtime_url, encode_cache_url(&fixture.cjs_hash, "mod.js"));
    assert_eq!(runtime_locate_url, runtime_url);
    assert_eq!(editor_resolved.url, runtime_url);
    assert_eq!(
        locator_key(&runtime_locator),
        locator_key(&editor_resolved.locator),
        "CJS locate parity"
    );
    assert_eq!(runtime_kind, ModuleKind::Cjs);

    let loaded = load_result(&loader, &runtime_url).expect("runtime loads CJS");
    assert!(
        loaded.contains("__meowCjsExports"),
        "cjs load delegates to the native CJS runtime shim: {loaded}"
    );
    assert!(!fixture.project.join("node_modules").exists());

    std::fs::remove_dir_all(&fixture.project).ok();
}

fn editor_store(fixture: &Fixture) -> UnpackedStore {
    UnpackedStore::new(fixture.project.join("unpacked"), fixture.cache.clone())
}

fn assert_same_variant(runtime: &ResolveError, editor: &ResolveError, label: &str) {
    assert_eq!(
        std::mem::discriminant(runtime),
        std::mem::discriminant(editor),
        "{label}: runtime and editor errors diverged\nruntime: {runtime:?}\neditor: {editor:?}"
    );
}
