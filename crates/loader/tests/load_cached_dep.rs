//! LOAD-001 behavior tests. Asserts invariants, not plumbing: a cached dependency
//! resolves from the content-addressed cache and runs end-to-end through the real
//! runtime with NO `node_modules` (I-5); one `Resolver` type/algorithm serves both
//! relative files and bare specifiers (I-1); `.ts` is type-erased by the shared
//! graph on load (I-1); a non-erasable TS construct and a tampered cache blob both
//! surface as honest, typed errors (I-7) — never fabricated, never swallowed.

use std::cell::RefCell;
use std::collections::HashMap;
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
use meow_pkg::{Cache, CacheError, ContentHash};
use meow_runtime::{print_sink_extension, PrintSink, Runtime, RuntimeOptions};

/// A unique temp directory per test, created eagerly. Uniqueness from pid + a
/// monotonic counter — no `rand`, no clock (the P16 grep bans both even in tests).
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

fn bare_map(name: &str, hash: ContentHash) -> HashMap<String, ContentHash> {
    let mut m = HashMap::new();
    m.insert(name.to_owned(), hash);
    m
}

/// A print sink that accumulates `console.log` output, plus the extension to install it.
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

/// Drive `load` synchronously and unwrap the returned source code (asserts `Sync`).
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

// I-5 + I-1: a local `.ts` entry imports a bare dependency that lives ONLY in the
// content-addressed cache; the program runs to completion through V8 and the dep's
// exported behavior is observed — and no `node_modules` is created anywhere.
#[tokio::test]
async fn cached_dep_runs_end_to_end_with_no_node_modules() {
    let proj = unique_dir("e2e");
    let cache_root = proj.join("cache");
    let hash = Cache::with_root(&cache_root)
        .store(b"export const greet = () => \"from cache\";\n")
        .expect("store dep blob");

    // The first-party entry: a `.ts` annotation (erasable) + a bare import of the dep.
    let entry = proj.join("main.ts");
    std::fs::write(
        &entry,
        "import { greet } from \"dep\";\nconst msg: string = greet();\nconsole.log(msg);\n",
    )
    .expect("write entry");

    let resolver = Resolver::new(
        cache_arc(&cache_root),
        bare_map("dep", hash),
        dir_url(&proj),
    );
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

    assert_eq!(
        *out.borrow(),
        "from cache\n",
        "dep's exported behavior is observed"
    );
    assert!(
        !proj.join("node_modules").exists(),
        "no node_modules is created (I-5)"
    );
    std::fs::remove_dir_all(&proj).ok();
}

// I-1: ONE `Resolver` type + algorithm serves both a relative file and a bare
// specifier — there is no second resolver.
#[test]
fn one_resolver_handles_relative_and_bare() {
    let proj = unique_dir("single");
    let cache_root = proj.join("cache");
    let hash = Cache::with_root(&cache_root)
        .store(b"export const v = 1;\n")
        .expect("store");

    let resolver = Resolver::new(
        cache_arc(&cache_root),
        bare_map("dep", hash.clone()),
        dir_url(&proj),
    );

    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer URL");
    let (rel_url, rel_loc) = resolver
        .locate("./a.ts", &referrer)
        .expect("relative resolves");
    assert_eq!(rel_url.scheme(), "file");
    assert!(matches!(rel_loc, ModuleLocator::LocalFile(_)));

    let (bare_url, bare_loc) = resolver.locate("dep", &referrer).expect("bare resolves");
    assert_eq!(bare_url.scheme(), "meow-cache");
    match bare_loc {
        ModuleLocator::Cached(h) => assert_eq!(h, hash),
        other => panic!("expected Cached, got {other:?}"),
    }
    std::fs::remove_dir_all(&proj).ok();
}

// I-1: a `.ts` module with a type annotation is type-erased by the shared graph on
// load — the annotation is gone, the runtime code is kept (the loader feeds the
// graph, never hands raw `.ts` to V8).
#[test]
fn ts_source_is_stripped_on_load() {
    let proj = unique_dir("strip");
    let entry = proj.join("typed.ts");
    std::fs::write(&entry, "export const n: number = 41 + 1;\n").expect("write");

    let resolver = Resolver::new(
        cache_arc(&proj.join("cache")),
        HashMap::new(),
        dir_url(&proj),
    );
    let loader = MeowModuleLoader::new(resolver, Rc::new(RefCell::new(GraphDb::new())));

    let spec = ModuleSpecifier::from_file_path(&entry).expect("file URL");
    let code = load_code(&loader, &spec).expect("typed module loads");
    assert!(!code.contains("number"), "type annotation erased: {code:?}");
    assert!(code.contains("41 + 1"), "runtime expression kept: {code:?}");
    std::fs::remove_dir_all(&proj).ok();
}

// I-1 (honesty): a non-erasable TS construct (`enum`) cannot be whitespace-stripped
// to correct JS, so load fails with the GRAPH diagnostic — never fabricated as
// "runnable".
#[test]
fn non_erasable_ts_enum_is_a_load_error() {
    let proj = unique_dir("enum");
    let entry = proj.join("bad.ts");
    std::fs::write(&entry, "enum E { A }\nexport const e = E.A;\n").expect("write");

    let resolver = Resolver::new(
        cache_arc(&proj.join("cache")),
        HashMap::new(),
        dir_url(&proj),
    );
    let loader = MeowModuleLoader::new(resolver, Rc::new(RefCell::new(GraphDb::new())));

    let spec = ModuleSpecifier::from_file_path(&entry).expect("file URL");
    let err = load_code(&loader, &spec).expect_err("enum module fails to load");
    assert!(
        err.contains("cannot load"),
        "carries an honest load diagnostic: {err:?}"
    );
    std::fs::remove_dir_all(&proj).ok();
}

// I-7: a tampered cache blob fails `Cache::read`'s integrity recompute, and the
// resolver propagates it as `ResolveError::Cache` — never serves the bytes.
#[test]
fn tampered_cache_blob_surfaces_integrity_error() {
    let proj = unique_dir("tamper");
    let cache_root = proj.join("cache");
    let cache = Cache::with_root(&cache_root);
    let hash = cache.store(b"export const v = 1;\n").expect("store");

    // Tamper: overwrite the on-disk blob with different bytes (its hash no longer matches).
    std::fs::write(cache.path_for(&hash), b"export const v = 999;\n").expect("tamper blob");

    let resolver = Resolver::new(
        cache_arc(&cache_root),
        bare_map("dep", hash),
        dir_url(&proj),
    );

    let referrer = Url::from_file_path(proj.join("main.ts")).expect("referrer");
    let err = resolver
        .resolve("dep", &referrer)
        .expect_err("tampered blob is refused");
    assert!(
        matches!(
            err,
            ResolveError::Cache(CacheError::IntegrityMismatch { .. })
        ),
        "integrity failure surfaces (I-7), got: {err:?}"
    );
    std::fs::remove_dir_all(&proj).ok();
}
