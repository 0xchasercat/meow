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
use meow_loader::{encode_cache_url, MeowModuleLoader, ModuleLocator, Resolver};
use meow_pkg::{
    Cache, ContentHash, LockEntry, Lockfile, PackageName, RegistryProvenance, ResolutionGraph,
    Version, VersionReq,
};
use meow_runtime::{print_sink_extension, PrintSink, Runtime, RuntimeOptions};

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("meow-loader-pnp-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn dir_url(dir: &Path) -> Url {
    Url::from_directory_path(dir).expect("dir url")
}

fn parsed_version(text: &str) -> Version {
    Version::parse(text).expect("valid version")
}

fn root_deps(entries: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
    entries
        .iter()
        .map(|(name, ver)| (PackageName::new(*name), parsed_version(ver)))
        .collect()
}

fn lock_entry(
    name: &str,
    version: &str,
    integrity: ContentHash,
    deps: &[(&str, &str)],
) -> LockEntry {
    LockEntry {
        name: PackageName::new(name),
        version: parsed_version(version),
        integrity,
        dependencies: deps
            .iter()
            .map(|(dep, ver)| (PackageName::new(*dep), parsed_version(ver)))
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

fn capture() -> (Rc<RefCell<String>>, deno_core::Extension) {
    let out = Rc::new(RefCell::new(String::new()));
    let sink_out = out.clone();
    let sink = PrintSink(Rc::new(move |msg: &str, is_err: bool| {
        if !is_err {
            sink_out.borrow_mut().push_str(msg);
        }
    }));
    (out, print_sink_extension(sink))
}

fn locator_key(locator: &ModuleLocator) -> String {
    match locator {
        ModuleLocator::LocalFile(path) => format!("file:{}", path.display()),
        ModuleLocator::Cached { package, member } => {
            format!("cache:{}:{member}", package.to_sri())
        }
        ModuleLocator::Native { name } => format!("native:{name}"),
    }
}

#[tokio::test]
async fn resolution_graph_runs_without_node_modules_and_preserves_multi_version_edges() {
    let project = unique_dir("runtime");
    let cache = Arc::new(Cache::with_root(project.join("cache")));

    let a_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"a","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            (
                "index.js",
                b"import { value } from \"b\";\nexport const fromA = value;\n",
            ),
        ]))
        .expect("store a");
    let c_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"c","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            (
                "index.js",
                b"import { value } from \"b\";\nexport const fromC = value;\n",
            ),
        ]))
        .expect("store c");
    let b1_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const value = 'b1';\n"),
        ]))
        .expect("store b1");
    let b2_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"2.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const value = 'b2';\n"),
        ]))
        .expect("store b2");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));

    let graph = Arc::new(
        ResolutionGraph::assemble(
            Arc::new(lockfile),
            root_deps(&[("a", "1.0.0"), ("c", "1.0.0")]),
        )
        .expect("graph assembles"),
    );
    graph
        .verify_cached(&cache)
        .expect("cache presence verified");

    let resolver = Resolver::from_resolution(
        &graph,
        cache.clone(),
        dir_url(&project),
        meow_runtime::native::native_module_registry(),
    );
    let a_referrer = encode_cache_url(&a_hash, "index.js");
    let c_referrer = encode_cache_url(&c_hash, "index.js");
    let (from_a, _) = resolver.locate("b", &a_referrer).expect("a resolves b@1");
    let (from_c, _) = resolver.locate("b", &c_referrer).expect("c resolves b@2");
    assert_eq!(from_a, encode_cache_url(&b1_hash, "index.js"));
    assert_eq!(from_c, encode_cache_url(&b2_hash, "index.js"));
    assert_eq!(from_a.scheme(), "meow-cache");
    assert_eq!(from_c.scheme(), "meow-cache");
    assert!(!from_a.as_str().contains("node_modules"));
    assert!(!from_c.as_str().contains("node_modules"));

    let entry = project.join("main.ts");
    std::fs::write(
        &entry,
        "import { fromA } from \"a\";\nimport { fromC } from \"c\";\nconsole.log(`${fromA}:${fromC}`);\n",
    )
    .expect("write entry");

    let loader = Rc::new(MeowModuleLoader::new(
        Resolver::from_resolution(
            &graph,
            cache,
            dir_url(&project),
            meow_runtime::native::native_module_registry(),
        ),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let (out, sink_ext) = capture();
    let mut runtime = Runtime::new(RuntimeOptions {
        module_loader: loader,
        extensions: vec![sink_ext],
    })
    .expect("runtime initializes");
    let spec = ModuleSpecifier::from_file_path(&entry).expect("entry url");
    runtime.run_main_module(&spec).await.expect("entry runs");

    assert_eq!(*out.borrow(), "b1:b2\n");
    assert!(
        !project.join("node_modules").exists(),
        "no node_modules tree exists"
    );

    std::fs::remove_dir_all(&project).ok();
}

#[test]
fn from_resolution_matches_locator_results_for_runtime_and_lsp_corpus() {
    let project = unique_dir("parity");
    let cache = Arc::new(Cache::with_root(project.join("cache")));

    let a_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"a","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const a = 'a';\n"),
        ]))
        .expect("store a");
    let c_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"c","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const c = 'c';\n"),
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
    let tool_hash = cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"tool","version":"1.0.0","exports":{".":"./index.js","./feature":"./src/feature.js"},"type":"module"}"#,
            ),
            ("index.js", b"export const root = true;\n"),
            ("src/feature.js", b"export const feature = true;\n"),
        ]))
        .expect("store tool");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));
    lockfile.upsert(lock_entry("tool", "1.0.0", tool_hash.clone(), &[]));

    let graph = Arc::new(
        ResolutionGraph::assemble(
            Arc::new(lockfile),
            root_deps(&[("a", "1.0.0"), ("c", "1.0.0"), ("tool", "1.0.0")]),
        )
        .expect("graph assembles"),
    );
    graph
        .verify_cached(&cache)
        .expect("cache presence verified");

    let runtime_resolver = Resolver::from_resolution(
        &graph,
        cache.clone(),
        dir_url(&project),
        meow_runtime::native::native_module_registry(),
    );
    let lsp_resolver = Resolver::from_resolution(
        &graph,
        cache.clone(),
        dir_url(&project),
        meow_runtime::native::native_module_registry(),
    );
    let loader = MeowModuleLoader::new(
        Resolver::from_resolution(
            &graph,
            cache,
            dir_url(&project),
            meow_runtime::native::native_module_registry(),
        ),
        Rc::new(RefCell::new(GraphDb::new())),
    );

    let project_referrer = Url::from_file_path(project.join("main.ts")).expect("project referrer");
    let cases = vec![
        (
            "bare root dep",
            "a",
            project_referrer.clone(),
            encode_cache_url(&a_hash, "index.js"),
            format!("cache:{}:index.js", a_hash.to_sri()),
        ),
        (
            "subpath export",
            "tool/feature",
            project_referrer,
            encode_cache_url(&tool_hash, "src/feature.js"),
            format!("cache:{}:src/feature.js", tool_hash.to_sri()),
        ),
        (
            "nested b from a",
            "b",
            encode_cache_url(&a_hash, "index.js"),
            encode_cache_url(&b1_hash, "index.js"),
            format!("cache:{}:index.js", b1_hash.to_sri()),
        ),
        (
            "nested b from c",
            "b",
            encode_cache_url(&c_hash, "index.js"),
            encode_cache_url(&b2_hash, "index.js"),
            format!("cache:{}:index.js", b2_hash.to_sri()),
        ),
    ];

    for (label, specifier, referrer, expected_url, expected_locator) in cases {
        let (runtime_url, runtime_locator) = runtime_resolver
            .locate(specifier, &referrer)
            .unwrap_or_else(|err| panic!("{label}: runtime locate failed: {err}"));
        let (lsp_url, lsp_locator) = lsp_resolver
            .locate(specifier, &referrer)
            .unwrap_or_else(|err| panic!("{label}: lsp locate failed: {err}"));

        assert_eq!(runtime_url, expected_url, "{label}: runtime url");
        assert_eq!(lsp_url, expected_url, "{label}: lsp url");
        assert_eq!(
            locator_key(&runtime_locator),
            expected_locator,
            "{label}: runtime locator"
        );
        assert_eq!(
            locator_key(&lsp_locator),
            expected_locator,
            "{label}: lsp locator"
        );
        assert_eq!(runtime_url, lsp_url, "{label}: url parity");
        assert_eq!(
            locator_key(&runtime_locator),
            locator_key(&lsp_locator),
            "{label}: locator parity"
        );

        let loaded = load_result(&loader, &runtime_url)
            .unwrap_or_else(|err| panic!("{label}: loader failed to load resolved url: {err}"));
        assert!(
            !loaded.is_empty(),
            "{label}: runtime loader returned source"
        );
        assert!(
            !runtime_url.as_str().contains("node_modules"),
            "{label}: no node_modules url"
        );
    }

    std::fs::remove_dir_all(&project).ok();
}
