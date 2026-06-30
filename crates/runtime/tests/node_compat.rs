use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deno_core::url::Url;
use meow_graph::GraphDb;
use meow_loader::{MeowModuleLoader, Resolver};
use meow_pkg::{Cache, Lockfile, PackageName, UnpackedStore, Version};
use meow_runtime::{
    hermetic, node, print_sink_extension, web, AllowAll, ModuleSpecifier, PrintSink, Runtime,
    RuntimeError, RuntimeOptions,
};
use node::NpmPackageFolderResolver;
use node_resolver::errors::{PackageFolderResolveErrorKind, PackageNotFoundError};

fn unique_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("meow-rt007-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn capture() -> (Rc<RefCell<String>>, deno_core::Extension) {
    let out = Rc::new(RefCell::new(String::new()));
    let target = out.clone();
    let sink = PrintSink(Rc::new(move |msg: &str, is_err: bool| {
        if !is_err {
            target.borrow_mut().push_str(msg);
        }
    }));
    (out, print_sink_extension(sink))
}

fn dir_url(path: &Path) -> Url {
    Url::from_directory_path(path).expect("dir url")
}

fn js_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\"))
}

struct RuntimeNodeBridge {
    resolver: Resolver,
    store: UnpackedStore,
}

impl RuntimeNodeBridge {
    fn new(resolver: Resolver, cache: Arc<Cache>) -> RuntimeNodeBridge {
        RuntimeNodeBridge {
            resolver,
            store: UnpackedStore::new(cache.root().join("unpacked"), cache),
        }
    }

    fn package_folder_error(
        kind: PackageFolderResolveErrorKind,
    ) -> node::PackageFolderResolveError {
        node::PackageFolderResolveError(Box::new(kind))
    }

    fn missing_package(
        &self,
        package_name: &str,
        referrer: &node::UrlOrPathRef,
    ) -> node::PackageFolderResolveError {
        self.missing_package_with_extra(package_name, referrer, None)
    }

    fn missing_package_with_extra(
        &self,
        package_name: &str,
        referrer: &node::UrlOrPathRef,
        referrer_extra: Option<String>,
    ) -> node::PackageFolderResolveError {
        Self::package_folder_error(PackageFolderResolveErrorKind::PackageNotFound(
            PackageNotFoundError {
                package_name: package_name.to_owned(),
                referrer: referrer.display(),
                referrer_extra,
            },
        ))
    }

    fn referrer_url(
        &self,
        referrer: &node::UrlOrPathRef,
    ) -> Result<node::Url, node::PackageFolderResolveError> {
        referrer.url().cloned().map_err(|err| {
            Self::package_folder_error(PackageFolderResolveErrorKind::PathToUrl(err))
        })
    }

    fn module_kind(&self, specifier: &node::Url) -> Option<meow_loader::ModuleKind> {
        let resolved = self
            .resolver
            .resolve_require(specifier.as_str(), specifier)
            .ok()?;
        Some(resolved.kind)
    }
}

impl node::NpmPackageFolderResolver for RuntimeNodeBridge {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        referrer: &node::UrlOrPathRef,
    ) -> Result<std::path::PathBuf, node::PackageFolderResolveError> {
        let referrer_url = self.referrer_url(referrer)?;
        let resolved = self
            .resolver
            .resolve_require(specifier, &referrer_url)
            .map_err(|err| {
                self.missing_package_with_extra(specifier, referrer, Some(err.to_string()))
            })?;

        let package_root = match resolved.locator {
            meow_loader::ModuleLocator::Cached { package, .. } => {
                self.store.ensure(&package).map_err(|err| {
                    self.missing_package_with_extra(specifier, referrer, Some(err.to_string()))
                })?
            }
            meow_loader::ModuleLocator::LocalFile(ref path) => {
                path.parent().unwrap_or(path.as_path()).to_path_buf()
            }
            meow_loader::ModuleLocator::Native { .. } => {
                return Err(self.missing_package(specifier, referrer))
            }
        };
        Ok(package_root)
    }

    fn resolve_types_package_folder(
        &self,
        types_package_name: &str,
        _maybe_package_version: Option<&node::Version>,
        maybe_referrer: Option<&node::UrlOrPathRef>,
    ) -> Option<std::path::PathBuf> {
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

impl node::InNpmPackageChecker for RuntimeNodeBridge {
    fn in_npm_package(&self, specifier: &node::Url) -> bool {
        let Ok(path) = specifier.to_file_path() else {
            return false;
        };
        path.starts_with(self.store.root())
    }
}

impl node::NodeRequireLoader for RuntimeNodeBridge {
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
        Ok(std::string::String::from_utf8_lossy(&source)
            .into_owned()
            .into())
    }

    fn is_maybe_cjs(&self, specifier: &node::Url) -> Result<bool, node::PackageJsonLoadError> {
        if let Ok(path) = specifier.to_file_path() {
            if path.starts_with(self.store.root()) {
                return Ok(matches!(
                    self.module_kind(specifier),
                    Some(meow_loader::ModuleKind::Cjs)
                ));
            }
            match path.extension().and_then(|ext| ext.to_str()) {
                None | Some("cjs") | Some("cts") | Some("ts") => return Ok(true),
                Some("json") | Some("mjs") | Some("mts") => return Ok(false),
                _ => {}
            }
        }
        Ok(matches!(
            self.module_kind(specifier),
            Some(meow_loader::ModuleKind::Cjs)
        ))
    }

    fn is_maybe_cjs_from_require(
        &self,
        specifier: &node::Url,
    ) -> Result<bool, node::PackageJsonLoadError> {
        self.is_maybe_cjs(specifier)
    }

    fn resolve_require_node_module_paths(&self, from: &Path) -> Vec<String> {
        let mut paths = Vec::with_capacity(from.components().count());
        let mut current_path = from;
        let mut maybe_parent = Some(current_path);
        while let Some(parent) = maybe_parent {
            if !parent.ends_with("node_modules") {
                paths.push(parent.join("node_modules").to_string_lossy().into_owned());
            }
            current_path = parent;
            maybe_parent = current_path.parent();
        }
        paths
    }

    fn resolve_package_folder_from_name(&self, package_name: &str) -> Option<std::path::PathBuf> {
        let referrer = node::UrlOrPathRef::from_url(self.resolver.project_root());
        self.resolve_package_folder_from_package(package_name, &referrer)
            .ok()
    }
}

fn runtime_parts_for(
    project_root: &Path,
) -> (Rc<dyn deno_core::ModuleLoader>, Resolver, Arc<Cache>) {
    let cache = Arc::new(Cache::with_root(project_root.join("cache")));
    let resolver = Resolver::new(
        cache.clone(),
        Arc::new(Lockfile::new()),
        BTreeMap::<PackageName, Version>::new(),
        dir_url(project_root),
        meow_runtime::native::native_module_registry(),
    );
    let loader = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    (loader, resolver, cache)
}

fn node_runtime(
    mode: node::NodeMode,
    cwd: &Path,
    argv: Vec<String>,
) -> (Rc<RefCell<String>>, Runtime) {
    let (loader, resolver, cache) = runtime_parts_for(cwd);
    let deno_node_bridge: Rc<dyn node::DenoNodeBridge> =
        Rc::new(RuntimeNodeBridge::new(resolver.clone(), cache));
    let deno_node_services = node::DenoNodeServicesBuilder::new(deno_node_bridge).build();
    let (out, sink) = capture();
    let caps: web::NetCaps = Arc::new(AllowAll);
    let mut extensions = Vec::new();
    extensions.extend(node::extensions(node::NodeOptions {
        mode,
        argv,
        main_module: None,
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
        deno_node_services: Some(deno_node_services),
        caps: Some(caps),
        user_agent: Some("meow-test".to_owned()),
    }));
    extensions.push(meow_loader::cjs_resolve_extension(resolver));
    let hermetic_cfg = if matches!(mode, node::NodeMode::Enabled) {
        hermetic::HermeticConfig::default().with_env_all()
    } else {
        hermetic::HermeticConfig::default()
    };
    extensions.extend(hermetic::extensions(hermetic_cfg));
    extensions.push(sink);

    let mut runtime = Runtime::new(RuntimeOptions {
        module_loader: loader,
        extensions,
        max_heap_size: None,
        startup_snapshot: None,
        residual_lazy_js_sources: &[],
        residual_lazy_esm_sources: &[],
        v8_flags: None,
    })
    .expect("runtime initializes");
    runtime
        .apply_hermetic_shadows()
        .expect("hermetic shadows apply");
    (out, runtime)
}

async fn run_src(rt: &mut Runtime, root: &Path, name: &str, src: &str) -> Result<(), RuntimeError> {
    let specifier = ModuleSpecifier::from_file_path(root.join(name)).expect("valid specifier");
    rt.run_main_module_from_source(&specifier, src.to_owned())
        .await
}

async fn run_file(rt: &mut Runtime, path: &Path) -> Result<(), RuntimeError> {
    let specifier = ModuleSpecifier::from_file_path(path).expect("valid file specifier");
    rt.run_main_module(&specifier).await
}

#[tokio::test]
async fn node_path_bare_and_node_round_trip() {
    let proj = unique_dir("path");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "path.mjs",
        r#"
        import path from "path";
        import nodePath from "node:path";
        const sample = path.join("alpha", "beta", "file.txt");
        const round = path.format(path.parse(sample));
        console.log(String(sample === round) + ":" + String(nodePath.join("a", "b") === path.join("a", "b")));
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn node_dns_bare_and_node_import_round_trip() {
    let proj = unique_dir("dns");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "dns.mjs",
        r#"
        import dns from "dns";
        import nodeDns from "node:dns";
        dns.lookup("127.0.0.1", (error, address, family) => {
          if (error) {
            throw error;
          }
          nodeDns.lookup("127.0.0.1", { family: 4 }, (allError, secondAddress, secondFamily) => {
            if (allError) {
              throw allError;
            }
            console.log(`${dns === nodeDns}:${address}:${family}:${secondAddress}:${secondFamily}`);
          });
        });
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:127.0.0.1:4:127.0.0.1:4\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_http_import_loads_telemetry_dependency() {
    let proj = unique_dir("http-telemetry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "http-telemetry.mjs",
        r#"
        import http from "node:http";
        import https from "node:https";
        const httpAgent = new http.Agent({ keepAlive: true });
        const httpsAgent = new https.Agent({ keepAlive: true });
        console.log(`${typeof http.Agent}:${typeof https.Agent}:${httpAgent.keepAlive}:${httpsAgent.keepAlive}`);
        "#,
    )
    .await
    .expect("http imports load Deno telemetry extension scripts");
    assert_eq!(*out.borrow(), "function:function:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn buffer_from_and_to_string() {
    let proj = unique_dir("buffer");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "buffer.mjs",
        r#"
        const first = Buffer.from("meow");
        const second = Buffer.from("meow");
        console.log(first.toString("utf8") + ":" + String(first.equals(second)));
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "meow:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn process_argv_cwd_and_platform_are_wired() {
    let proj = unique_dir("process");
    let entry = proj.join("entry.mjs").to_string_lossy().into_owned();
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            entry.clone(),
            "one".to_owned(),
            "two".to_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "process.mjs",
        r#"
        console.log(process.argv.join("|"));
        console.log(process.cwd());
        console.log(process.platform + ":" + process.arch);
        console.log(`${typeof process.pid}:${typeof process.ppid}:${typeof process.pid.toString(36)}`);
        "#,
    )
    .await
    .expect("module runs");
    let platform = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x64"
    } else {
        "unknown"
    };
    let expected = format!(
        "meow|{entry}|one|two\n{}\n{platform}:{arch}\nnumber:number:string\n",
        proj.to_string_lossy()
    );
    assert_eq!(*out.borrow(), expected);
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn process_hrtime_shape_is_compatible() {
    let proj = unique_dir("process-hrtime");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "process.hrtime.mjs",
        r#"
        const hrtime = process.hrtime;
        const isHrtimeFunction = typeof hrtime === "function";
        const hrtimeBigint = isHrtimeFunction ? hrtime.bigint : undefined;
        const start = isHrtimeFunction ? hrtime() : [];
        const delta = isHrtimeFunction ? hrtime(start) : [];
        const hasTwoNumericEntries =
            Array.isArray(delta) &&
            delta.length === 2 &&
            typeof delta[0] === "number" &&
            typeof delta[1] === "number";
        const hrtimeBigintType = typeof hrtimeBigint;
        const hrtimeBigintReturnType =
            hrtimeBigintType === "function" ? typeof hrtimeBigint() : "not-a-function";
        console.log(
            `${typeof hrtime}:${hrtimeBigintType}:${hrtimeBigintReturnType}:${String(hasTwoNumericEntries)}`
        );
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "function:function:bigint:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn process_memory_usage_shape_is_compatible() {
    let proj = unique_dir("process-memory-usage");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "process.memoryUsage.mjs",
        r#"
        const memoryUsage = process.memoryUsage;
        const isFunction = typeof memoryUsage === "function";
        const usage = isFunction ? memoryUsage() : undefined;
        const hasNumericFields =
            isFunction &&
            usage !== undefined &&
            typeof usage.rss === "number" &&
            typeof usage.heapTotal === "number" &&
            typeof usage.heapUsed === "number" &&
            typeof usage.external === "number" &&
            typeof usage.arrayBuffers === "number";
        const isConsistent =
            hasNumericFields && usage.heapTotal >= usage.heapUsed;
        console.log(`${isFunction}:${hasNumericFields}:${isConsistent}`);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn process_umask_shape_is_compatible() {
    let proj = unique_dir("process-umask");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "process.umask.mjs",
        r#"
        const isFunction = typeof process.umask === "function";
        const hasNumericReturn = isFunction ? typeof process.umask() === "number" : false;
        const hasNumericNestedReturn = isFunction
            ? typeof process.umask(process.umask()) === "number"
            : false;
        console.log(`${isFunction}:${hasNumericReturn}:${hasNumericNestedReturn}`);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_v8_get_heap_statistics_shape() {
    let proj = unique_dir("v8-get-heap-stats");
    let entry = proj.join("v8-heap-stats.cjs");
    std::fs::write(
        &entry,
        r#"const v8 = require('v8');
const hasGetHeapStatistics = typeof v8.getHeapStatistics === "function";
const statistics = hasGetHeapStatistics ? v8.getHeapStatistics() : undefined;
const hasNumericFields =
  statistics !== undefined &&
  typeof statistics.used_heap_size === "number" &&
  typeof statistics.heap_size_limit === "number";
console.log(String(hasGetHeapStatistics) + ":" + String(hasNumericFields));
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("node v8 getHeapStatistics regression runs");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_querystring_stringify_and_parse_shape() {
    let proj = unique_dir("querystring");
    let entry = proj.join("querystring.cjs");
    std::fs::write(
        &entry,
        r#"const qs = require('querystring');
const hasStringify = typeof qs.stringify === "function";
const encoded = qs.stringify({ a: "1", b: "x y" });
const parsed = qs.parse("a=1&b=x%20y").b === "x y";
console.log(
  String(hasStringify) + ":" +
  String(encoded.includes("a=1")) + ":" +
  String(encoded.includes("b=x%20y")) + ":" +
  String(parsed)
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("querystring shim regression runs");
    assert_eq!(*out.borrow(), "true:true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn global_aliases_global_this_in_node_mode() {
    let proj = unique_dir("global");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "global.mjs",
        r#"
        console.log(String(global === globalThis) + ":" + typeof global.process);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:object\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn node_mode_global_atob_and_btoa_are_functions() {
    let proj = unique_dir("node-globals-atob");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "atob-btoa.mjs",
        r#"
        console.log(typeof atob + ":" + typeof btoa);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "function:function\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn node_mode_console_methods_are_functions() {
    let proj = unique_dir("node-globals-console-methods");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "console-global.mjs",
        r#"
        console.log(
            typeof console.assert + ":" +
            typeof console.time + ":" +
            typeof console.timeEnd + ":" +
            typeof console.timeLog + ":" +
            typeof console.trace + ":" +
            typeof console.count + ":" +
            typeof console.dir
        );
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(
        *out.borrow(),
        "function:function:function:function:function:function:function\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn buffer_alloc_unsafe_shape_exists() {
    let proj = unique_dir("buffer-alloc-unsafe");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "buffer-alloc-unsafe.mjs",
        r#"
        import { Buffer } from "node:buffer";
        const buf = Buffer.allocUnsafe(4);
        console.log(`${typeof Buffer.allocUnsafe}:${typeof Buffer.allocUnsafeSlow}:${buf.length}`);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "function:function:4\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_console_builtin_shape() {
    let proj = unique_dir("cjs-console-builtin");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const consoleMod = require("console");
const logShape = typeof consoleMod.log === "function" ||
    (typeof consoleMod.default === "object" && typeof consoleMod.default.log === "function");
const logger = new consoleMod.Console(process.stdout, process.stderr);
console.log(
  `${typeof consoleMod.Console === "function"}:${String(logShape)}:${typeof logger}`
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS console builtin shape runs");
    assert_eq!(*out.borrow(), "true:true:object\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_tty_isatty_and_stdstreams_are_tty_booleans() {
    let proj = unique_dir("cjs-tty-shape");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const tty = require("tty");
const hasIsatty = typeof tty.isatty === "function";
const stdoutIsTTY = typeof process.stdout.isTTY === "boolean";
const stderrIsTTY = typeof process.stderr.isTTY === "boolean";
console.log(`${hasIsatty}:${stdoutIsTTY}:${stderrIsTTY}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS tty shim shape runs");
    assert_eq!(*out.borrow(), "true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn non_tty_stdin_uses_generic_raw_mode_fallback_shape() {
    let proj = unique_dir("cjs-non-tty-raw-mode-shape");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"require("tty");
const source = String(process.stdin.setRawMode);
console.log([
  process.stdin.constructor && process.stdin.constructor.name,
  typeof process.stdin._handle?.setRawMode,
  source.includes("_handle.setRawMode"),
  source.includes("io.stdin.setRaw"),
].join(":"));
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS non-TTY raw mode fallback shape runs");
    // Two legitimate outputs depending on environment:
    //   TTY (interactive):       ReadStream:function:true:false
    //   Non-TTY (redirected/CI): Duplex:undefined:false:true
    // Both are correct.  The test verifies the polyfill source wiring is
    // present when a real TTY handle isn't available (non-TTY path: parts[3]=true),
    // and that `_handle` exists in both paths.
    let output = out.borrow();
    let parts: Vec<&str> = output.trim().split(':').collect();
    assert_eq!(
        parts.len(),
        4,
        "expected 4 colon-delimited fields, got {output:?}"
    );
    if parts[0] == "Duplex" {
        // Non-TTY: polyfill path — io.stdin.setRaw must be in the source
        assert_eq!(
            parts[1], "undefined",
            "expected undefined _handle.setRawMode in Duplex path"
        );
        assert_eq!(
            parts[2], "false",
            "expected no _handle.setRawMode in Duplex path"
        );
        assert_eq!(
            parts[3], "true",
            "expected io.stdin.setRaw in setRawMode source for Duplex path"
        );
    } else if parts[0] == "ReadStream" {
        // TTY: real handle path — _handle.setRawMode is a native function,
        // io.stdin.setRaw fallback not needed
        assert_eq!(
            parts[1], "function",
            "expected function _handle.setRawMode in TTY path"
        );
        assert_eq!(
            parts[2], "true",
            "expected _handle.setRawMode in source for TTY path"
        );
        assert_eq!(
            parts[3], "false",
            "expected no io.stdin.setRaw in source for TTY path"
        );
    } else {
        panic!("unexpected constructor name: {}", parts[0]);
    }
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_mode_event_and_event_target_are_functions() {
    let proj = unique_dir("node-globals-events");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "events-global.mjs",
        r#"
        console.log(typeof Event + ":" + typeof EventTarget);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "function:function\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_mode_structured_clone_is_function() {
    let proj = unique_dir("node-globals-structured-clone");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "structured-clone.mjs",
        r#"
        console.log(typeof structuredClone);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "function\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_mode_stream_globals_are_functions() {
    let proj = unique_dir("node-globals-streams");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "streams.mjs",
        r#"
        let writableOk = false;
        try {
            new WritableStream({
                write() {},
            });
            writableOk = true;
        } catch {
            writableOk = false;
        }
        console.log(
            typeof ReadableStream + ":" +
            typeof WritableStream + ":" +
            typeof TransformStream + ":" +
            typeof TextEncoderStream + ":" +
            typeof TextDecoderStream + ":" +
            String(writableOk)
        );
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(
        *out.borrow(),
        "function:function:function:function:function:true\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn global_performance_shape_in_node_mode() {
    let proj = unique_dir("perf-hooks");
    let entry = proj.join("perf-hooks.cjs");
    std::fs::write(
        &entry,
        r#"const { performance: nodePerformance } = require("node:perf_hooks");
const globalPerformance = globalThis.performance;

globalPerformance.mark("node-mode-global-mark");
nodePerformance.mark("node-mode-node-mark");

const equivalent =
  globalPerformance === nodePerformance ||
  (typeof globalPerformance.mark === "function" && typeof nodePerformance.mark === "function");

console.log(
  String(typeof globalPerformance) + ":" +
  String(typeof performance.mark) + ":" +
  String(typeof nodePerformance) + ":" +
  String(typeof nodePerformance.mark) + ":" +
  String(equivalent)
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("node perf hooks regression runs");
    assert_eq!(*out.borrow(), "object:function:object:function:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn node_perf_hooks_performance_observer_shape() {
    let proj = unique_dir("perf-hooks-observer");
    let entry = proj.join("perf-hooks-observer.cjs");
    std::fs::write(
        &entry,
        r#"const { PerformanceObserver } = require('perf_hooks');
const observer = new PerformanceObserver(() => {});
console.log(
  String(typeof PerformanceObserver) + ":" +
  String(typeof observer.observe) + ":" +
  String(typeof observer.disconnect) + ":" +
  String(Array.isArray(PerformanceObserver.supportedEntryTypes))
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("node perf hooks observer regression runs");
    assert_eq!(*out.borrow(), "function:function:function:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_path_default_exposes_posix_and_win32() {
    let proj = unique_dir("path-default-variants");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "path-default.mjs",
        r#"
        import path from "path";
        console.log(`${typeof path.win32}:${typeof path.win32?.isAbsolute}:${typeof path.posix}:${typeof path.posix?.isAbsolute}`);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "object:function:object:function\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn fs_default_runtime_read_write_and_stat_through_temp_dir() {
    let proj = unique_dir("fs-default");
    let nested = proj.join("nested");
    let file = nested.join("note.txt");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "fs-default.mjs",
        &format!(
            r#"
            import fs from "fs";
            const dir = {dir};
            const file = {file};
            fs.mkdirSync(dir, {{ recursive: true }});
            await fs.promises.writeFile(file, "hello from promises");
            const text = fs.readFileSync(file, "utf8");
            const stat = fs.statSync(file);
            const names = await fs.promises.readdir(dir);
            await fs.promises.access(file);
            await fs.promises.rm(file);
            console.log(text + ":" + names[0] + ":" + String(stat.isFile()) + ":" + String(fs.existsSync(file)));
            "#,
            dir = js_string(&nested.to_string_lossy()),
            file = js_string(&file.to_string_lossy()),
        ),
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "hello from promises:note.txt:true:false\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn fs_readdir_with_file_types_and_missing_stat_shape() {
    let proj = unique_dir("fs-dirent");
    let dir = proj.join("dir");
    let subdir = dir.join("subdir");
    let file = dir.join("file.txt");
    std::fs::create_dir_all(&subdir).expect("create subdir");
    std::fs::write(&file, "hello").expect("write test file");

    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "fs-dirent.mjs",
        &format!(
            r#"
            import fs from "fs";
            const dir = {dir};
            const entries = await fs.promises.readdir(dir, {{ withFileTypes: true }});
            const names = entries
              .map((entry) => (typeof entry?.name === "string" ? entry.name : ""))
              .sort()
              .join(",");
            const hasNames = entries.every((entry) => typeof entry?.name === "string");
            const hasDirectory = entries.some(
              (entry) => typeof entry?.isDirectory === "function" && entry.isDirectory()
            );
            const directoryName = entries.find(
              (entry) => typeof entry?.isDirectory === "function" && entry.isDirectory()
            )?.name;
            let statError;
            try {{
              fs.statSync("missing");
            }} catch (error) {{
              statError = error;
            }}
            const hasErrorMessage = typeof statError?.message === "string" && statError.message.length > 0;
            const hasEnoent = statError?.code === "ENOENT";
            console.log(
              names + ":" + String(hasNames) + ":" + String(hasDirectory) + ":" + String(directoryName) + ":" +
              String(hasErrorMessage) + ":" + String(hasEnoent)
            );
            "#,
            dir = js_string(&dir.to_string_lossy()),
        ),
    )
    .await
    .expect("module runs");
    assert_eq!(
        *out.borrow(),
        "file.txt,subdir:true:true:subdir:true:true\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn esm_named_import_from_commonjs_exports_object_works() {
    let proj = unique_dir("esm-cjs-named-export");
    let entry = proj.join("main.mjs");
    let dep = proj.join("dep.cjs");
    std::fs::write(&dep, "exports.Answer = 42;\n").expect("write cjs dep");
    std::fs::write(
        &entry,
        "import { Answer } from \"./dep.cjs\";\nconsole.log(String(Answer));\n",
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry).await.expect("esm import runs");
    assert_eq!(*out.borrow(), "42\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_fs_missing_stat_throws_enoent() {
    let proj = unique_dir("cjs-fs-missing-stat");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const fs = require("fs");
const assert = require("assert");

// Test statSync
try {
  fs.statSync("missing-file-stat");
  throw new Error("should throw");
} catch (error) {
  assert.strictEqual(error.code, "ENOENT");
  assert.strictEqual(error.errno, -2);
  assert.strictEqual(error.syscall, "stat");
  assert.strictEqual(error.path, "missing-file-stat");
  assert.strictEqual(error.message, "ENOENT: no such file or directory, stat 'missing-file-stat'");
}

// Test readFileSync
try {
  fs.readFileSync("missing-file-read");
  throw new Error("should throw");
} catch (error) {
  assert.strictEqual(error.code, "ENOENT");
  assert.strictEqual(error.errno, -2);
  assert.strictEqual(error.syscall, "open");
  assert.strictEqual(error.path, "missing-file-read");
  assert.strictEqual(error.message, "ENOENT: no such file or directory, open 'missing-file-read'");
}

console.log("PASS");
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS fs missing stat runs");
    assert_eq!(*out.borrow(), "PASS\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_fs_descriptor_apis_are_implemented() {
    let proj = unique_dir("fs-descriptor-apis");
    let file = proj.join("payload.txt");
    let contents = "descriptor-regression-content";
    std::fs::write(&file, contents).expect("write fixture");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "fs-descriptor-apis.mjs",
        &format!(
            r#"
            import fs from "fs";
            const file = {file};
            const content = {content};
            const expectedSlice = content.slice(4, 8);
            const fd = fs.openSync(file, "r");
            const isFdNumber = typeof fd === "number";
            const buffer = Buffer.alloc(8);
            const bytesRead = fs.readSync(fd, buffer, 0, 4, 4);
            fs.closeSync(fd);
            const chunks = [];
            let dataEvents = 0;
            const stream = fs.createReadStream(file);
            const allData = await new Promise((resolve, reject) => {{
              stream.on("data", (chunk) => {{
                dataEvents += 1;
                chunks.push(chunk);
              }});
              stream.on("error", reject);
              stream.on("end", () => {{
                resolve(Buffer.concat(chunks).toString("utf8"));
              }});
            }});
            const slice = buffer.slice(0, bytesRead).toString("utf8");
            console.log(
              String(isFdNumber) + ":" +
              String(bytesRead) + ":" +
              String(slice === expectedSlice) + ":" +
              String(dataEvents > 0) + ":" +
              String(allData === content)
            );
            "#,
            file = js_string(&file.to_string_lossy()),
            content = js_string(contents),
        ),
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:4:true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_fs_equals_bare_fs() {
    let proj = unique_dir("fs-eq");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "fs-eq.mjs",
        r#"
        import fs from "fs";
        import nodeFs from "node:fs";
        console.log(String(fs === nodeFs));
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_fs_create_write_stream_round_trip() {
    let proj = unique_dir("cjs-fs-write-stream");
    let entry = proj.join("main.cjs");
    let target = proj.join("trace.txt");
    std::fs::write(
        &entry,
        format!(
            r#"
const fs = require('fs');
const file = {file};
const stream = fs.createWriteStream(file, {{ flags: 'a', encoding: 'utf8' }});

stream.write('chunk-one');
stream.write('chunk-two');

stream.end(() => {{
  const text = fs.readFileSync(file, 'utf8');
  const hadWrite = typeof stream.write === 'function';
  const hadEnd = typeof stream.end === 'function';
  const beforeUnlink = fs.existsSync(file);
  fs.unlinkSync(file);
  const afterUnlink = fs.existsSync(file);
  console.log(
    String(hadWrite) + ':' +
    String(hadEnd) + ':' +
    text + ':' +
    String(beforeUnlink) + ':' +
    String(afterUnlink)
  );
}});
"#,
            file = js_string(&target.to_string_lossy()),
        ),
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS fs write stream runs");
    assert_eq!(*out.borrow(), "true:true:chunk-onechunk-two:true:false\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_fs_and_node_fs_round_trip() {
    let proj = unique_dir("cjs-fs");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const fs = require('fs');\nconst nodeFs = require('node:fs');\nconsole.log(String(fs === nodeFs) + ':' + typeof fs.readFileSync);\n",
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry).await.expect("CommonJS fs runs");
    assert_eq!(*out.borrow(), "true:function\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_fs_realpath_sync_native_shape() {
    let proj = unique_dir("cjs-fs-realpath");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const fs = require("fs");
const hasRealpathSync = typeof fs.realpathSync === "function";
const hasNative = typeof fs.realpathSync.native === "function";
const hasNativeString = hasNative && typeof fs.realpathSync.native(".") === "string";
console.log(`${hasRealpathSync}:${hasNative}:${hasNativeString}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS fs realpath sync native runs");
    assert_eq!(*out.borrow(), "true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_os_type_shape() {
    let proj = unique_dir("cjs-os-type");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const os = require("os");
const hasTypeFunction = typeof os.type === "function";
const type = os.type();
console.log(`${hasTypeFunction}:${type}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS os.type runs");
    let expected = if cfg!(windows) {
        "true:Windows_NT\n"
    } else if cfg!(target_os = "macos") {
        "true:Darwin\n"
    } else if cfg!(target_os = "linux") {
        "true:Linux\n"
    } else {
        let output = out.borrow();
        assert!(
            output.starts_with("true:") && output.trim().len() > "true:".len(),
            "os.type should return a non-empty string, got: {output:?}"
        );
        std::fs::remove_dir_all(&proj).ok();
        return;
    };
    assert_eq!(*out.borrow(), expected);
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_util_debuglog_shape() {
    let proj = unique_dir("cjs-util-debuglog");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const util = require("util");
const debug = util.debuglog("undici");
console.log(`${typeof util.debuglog}:${typeof debug}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS util debuglog runs");
    assert_eq!(*out.borrow(), "function:function\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_util_types_shape() {
    let proj = unique_dir("cjs-util-types");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const types = require("util/types");
const hasFunction = typeof types.isUint8Array === "function";
const hasValue = types.isUint8Array(new Uint8Array()) === true;
console.log(`${hasFunction}:${hasValue}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS util/types subpath works");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_util_inherits() {
    let proj = unique_dir("cjs-util-inherits");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const util = require("util");

function Base() {}
Base.prototype.ping = () => 'pong';

function Child() {}
util.inherits(Child, Base);
const instance = new Child();
const hasInstance = instance instanceof Base;
const hasPing = instance.ping() === 'pong';
console.log(`${hasInstance}:${hasPing}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS util inherits works");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_util_deprecate_shape() {
    let proj = unique_dir("cjs-util-deprecate");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const util = require("util");
const wrapped = util.deprecate((x) => x + 1, "deprecated");
const wrappedType = typeof wrapped === "function";
const wrappedResult = wrapped(1) === 2;
console.log(`${wrappedType}:${wrappedResult}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS util deprecate works");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_http2_constants_shape() {
    let proj = unique_dir("cjs-http2-constants");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const http2 = require("http2");
const hasAuthority = http2?.constants?.HTTP2_HEADER_AUTHORITY === ":authority";
const hasStatus = http2?.constants?.HTTP2_HEADER_STATUS === ":status";
console.log(`${hasAuthority}:${hasStatus}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS http2 constants runs");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_http_and_https_agent_surface() {
    let proj = unique_dir("cjs-http-https-agent");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const http = require("http");
const https = require("https");

let httpAgentConstructed = false;
let httpsAgentConstructed = false;

try {
  httpAgentConstructed = new http.Agent({ keepAlive: true }).keepAlive === true;
} catch (error) {
  httpAgentConstructed = false;
}

try {
  httpsAgentConstructed = new https.Agent({ keepAlive: true }).keepAlive === true;
} catch (error) {
  httpsAgentConstructed = false;
}

console.log(
  `${typeof http.Agent === "function"}:${typeof https.Agent === "function"}:${httpAgentConstructed}:${httpsAgentConstructed}`
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS http/https agent surface runs");
    assert_eq!(*out.borrow(), "true:true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_constants_shape() {
    let proj = unique_dir("cjs-constants");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const constants = require("constants");
const isR_OK_number = typeof constants.R_OK === "number";
const fsObject = typeof constants.fs === "object";
const sameR_OK = constants.fs?.R_OK === constants.R_OK;
console.log(`${String(isR_OK_number)}:${String(sameR_OK)}:${String(fsObject)}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS constants builtin runs");
    assert_eq!(*out.borrow(), "true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_diagnostics_channel_has_shape_and_subscribe_toggle() {
    let proj = unique_dir("cjs-diagnostics-channel");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const dc = require("diagnostics_channel");
const ch = dc.channel("x");
const before = ch.hasSubscribers;
let published = 0;
const handler = () => {
  published += 1;
};
ch.subscribe(handler);
const during = ch.hasSubscribers;
ch.publish({ value: 1 });
ch.unsubscribe(handler);
const after = ch.hasSubscribers;
ch.publish({ value: 2 });
console.log(
  `${typeof dc.channel === "function"}:${typeof ch.publish === "function"}:${typeof ch.hasSubscribers === "boolean"}:${String(before)}:${String(during)}:${String(after)}:${String(published)}`
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS diagnostics_channel runs");
    assert_eq!(*out.borrow(), "true:true:true:false:true:false:1\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_with_shebang_runs() {
    let proj = unique_dir("cjs-shebang");
    let entry = proj.join("entry.cjs");
    std::fs::write(
        &entry,
        "#!/usr/bin/env node\nconsole.log(\"shebang execution\");\n",
    )
    .expect("write shebang entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    let spec = ModuleSpecifier::from_file_path(&entry).expect("entry spec");
    rt.run_main_module(&spec)
        .await
        .expect("shebang CommonJS runs");
    assert_eq!(*out.borrow(), "shebang execution\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn strict_web_commonjs_still_parses_but_fs_is_withdrawn_at_use_time() {
    let proj = unique_dir("strict-web-cjs");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        "const fs = require('fs');\ntry {\n  fs.readFileSync('nope');\n} catch (error) {\n  console.log(String(error.message ?? error));\n}\n",
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::StrictWeb,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    if let Err(err) = run_file(&mut rt, &entry).await {
        panic!(
            "strict-web CJS still instantiates failed: {:?}, logs: {}",
            err,
            out.borrow()
        );
    }
    assert!(
        out.borrow()
            .contains("strict-web mode withdraws node:fs access"),
        "withdrawal stays at API use time: {}",
        out.borrow()
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn strict_web_withdraws_fs_and_real_env() {
    let proj = unique_dir("strict-web");
    let (out, mut rt) = node_runtime(
        node::NodeMode::StrictWeb,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "strict-web.mjs",
        r#"
        import fs from "node:fs";
        import process from "node:process";
        let fsDenied = false;
        let envDenied = false;
        try {
          fs.readFileSync("/definitely/missing", "utf8");
        } catch (error) {
          fsDenied = error instanceof Error && error.code === "ERR_STRICT_WEB_WITHDRAWN";
        }
        try {
          process.env.PATH;
        } catch (error) {
          envDenied = error instanceof Error && error.code === "ERR_STRICT_WEB_WITHDRAWN";
        }
        console.log(String(fsDenied) + ":" + String(envDenied) + ":" + String(typeof globalThis.process === "undefined"));
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_events_returns_constructor_shape() {
    let proj = unique_dir("cjs-events-shape");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const Events = require("events");
const ee = new Events();
const derived = class extends Events {};
console.log(`${typeof Events}:${typeof Events.EventEmitter}:${typeof Events.getMaxListeners}:${ee instanceof Events}:${new derived() instanceof Events}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS events constructor shape runs");
    assert_eq!(*out.borrow(), "function:function:function:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_events_legacy_constructor_call_compat() {
    let proj = unique_dir("cjs-events-legacy-constructor");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const { EventEmitter } = require("events");
const util = require("util");

function Child() {
  EventEmitter.call(this);
}

util.inherits(Child, EventEmitter);

const child = new Child();
let triggered = false;
child.on("x", () => {
  triggered = true;
});
child.emit("x");
console.log(`${child instanceof EventEmitter}:${triggered}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS EventEmitter legacy constructor compatibility works");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_stream_constructability_shape() {
    let proj = unique_dir("cjs-stream-construct");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const stream = require("stream");
const streamSubclassConstructable = (() => {
  class StreamSubclass extends stream {}
  const instance = new StreamSubclass();
  return instance instanceof stream;
})();
const readableSubclassConstructable = (() => {
  class ReadableSubclass extends stream.Readable {}
  const instance = new ReadableSubclass();
  return instance instanceof stream.Readable;
})();
let passThroughConstructed = false;
try {
  const passThrough = new stream.PassThrough();
  passThroughConstructed = true;
} catch (error) {
  passThroughConstructed = false;
}
console.log(
  `${typeof stream}:${typeof stream.Readable}:${typeof stream.Writable}:${typeof stream.pipeline}:${String(streamSubclassConstructable)}:${String(readableSubclassConstructable)}:${String(passThroughConstructed)}`
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS stream constructor shape runs");
    assert_eq!(
        *out.borrow(),
        "function:function:function:function:true:true:true\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_require_node_stream_web_writable_stream_is_constructable() {
    let proj = unique_dir("cjs-node-stream-web");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const web = require("node:stream/web");
const writableStreamFunction = typeof web.WritableStream === "function";
let writableStreamConstructed = false;
if (writableStreamFunction) {
  try {
    new web.WritableStream({
      write() {},
    });
    writableStreamConstructed = true;
  } catch {
    writableStreamConstructed = false;
  }
}
console.log(`${String(writableStreamFunction)}:${String(writableStreamConstructed)}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS node:stream/web constructability runs");
    assert_eq!(*out.borrow(), "true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_async_hooks_async_resource_run_in_async_scope() {
    let proj = unique_dir("cjs-async-hooks");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const { AsyncResource } = require("async_hooks");
class MyResource extends AsyncResource {}
let constructed = false;
let isInstance = false;
let callbackRan = false;
const resource = new MyResource("regression");
constructed = true;
isInstance = resource instanceof AsyncResource;
resource.runInAsyncScope(() => {
  callbackRan = true;
});
console.log(`${typeof AsyncResource}:${String(constructed)}:${String(isInstance)}:${String(callbackRan)}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS async_hooks AsyncResource runs");
    assert_eq!(*out.borrow(), "function:true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_async_local_storage_shape() {
    let proj = unique_dir("cjs-async-local-storage");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const { AsyncLocalStorage } = require("async_hooks");
const storage = new AsyncLocalStorage();
let observed;
storage.run({ value: "meow" }, () => {
  observed = storage.getStore().value;
});
console.log(`${typeof AsyncLocalStorage}:${observed}:${typeof AsyncLocalStorage.snapshot}`);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS async_hooks AsyncLocalStorage runs");
    assert_eq!(*out.borrow(), "function:meow:function\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn event_emitter_emit_and_on() {
    let proj = unique_dir("events");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "events.mjs",
        r#"
        import { EventEmitter } from "node:events";
        const events = new EventEmitter();
        let seen = "";
        events.on("tick", (left, right) => {
          seen = String(left) + String(right);
        });
        console.log(String(events.emit("tick", "me", "ow")) + ":" + seen);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true:meow\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_assert_module_works() {
    let proj = unique_dir("assert");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj,
        "assert.mjs",
        r#"
        import assert from "node:assert";
        assert.strictEqual(1 + 1, 2);
        let failed = false;
        try {
          assert.ok(false, "nope");
        } catch (error) {
          failed = error.code === "ERR_ASSERTION";
        }
        console.log(String(failed));
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "true\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_mode_timer_globals_are_functions() {
    let proj = unique_dir("node-timer-globals");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "timers.mjs",
        r#"
        console.log(`${typeof setTimeout}:${typeof clearTimeout}:${typeof setInterval}:${typeof clearInterval}`);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "function:function:function:function\n");
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn node_crypto_hash_hmac_random_and_bare_round_trip() {
    let proj = unique_dir("crypto");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec![
            "meow".to_owned(),
            proj.join("main.mjs").to_string_lossy().into_owned(),
        ],
    );
    run_src(
        &mut rt,
        &proj, "crypto.mjs",
        r#"
        import crypto from "crypto";
        import nodeCrypto, { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
        const hash = createHash("sha256").update("abc").digest("hex");
        const mac = createHmac("sha256", "key").update("data").digest("hex");
        const bytes = randomBytes(8);
        const equal = timingSafeEqual(Buffer.from("same"), Buffer.from("same"));
        console.log(`${crypto === nodeCrypto}:${hash}:${mac}:${bytes.length}:${equal}`);
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(
        *out.borrow(),
        "true:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad:5031fe3d989c6d1537a013fa6e739da23463fdaec3b70137d828e36ace221bd0:8:true\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[tokio::test]
async fn commonjs_require_dns_lookup_returns_node_shapes() {
    let proj = unique_dir("cjs-dns");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const dns = require("dns");
const nodeDns = require("node:dns");
const nodeModule = require("module");
dns.lookup("127.0.0.1", (error, address, family) => {
  if (error) throw error;
  nodeDns.lookup("127.0.0.1", { family: 4, all: true }, (allError, addresses) => {
    if (allError) throw allError;
    const first = addresses[0];
    console.log(`${dns === nodeDns}:${nodeModule.isBuiltin("dns")}:${nodeModule.builtinModules.includes("dns")}:${address}:${family}:${Array.isArray(addresses)}:${first.address}:${first.family}`);
  });
});
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry).await.expect("CommonJS dns runs");
    assert_eq!(
        *out.borrow(),
        "true:true:true:127.0.0.1:4:true:127.0.0.1:4\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_module_compat_require_hook_shape() {
    let proj = unique_dir("cjs-module-hook");
    let entry = proj.join("main.cjs");
    std::fs::write(
        &entry,
        r#"const mod = require("module");
const path = require("path");
const fs = require("fs");
const originalRequire = mod.prototype.require;
const originalResolve = mod._resolveFilename;
const resolved = require.resolve("path");
const viaOriginal = originalRequire.call(module, "path");
mod._resolveFilename = function(request, parent) {
  return request === "meow:path" ? resolved : originalResolve.call(this, request, parent);
};
const viaAlias = originalRequire.call(module, "meow:path");
console.log(
  `${typeof mod.prototype.require === "function"}:${typeof mod._resolveFilename === "function"}:${typeof require.resolve === "function"}:${typeof resolved === "string"}:${viaOriginal.basename("a/b") === path.basename("a/b")}:${viaAlias.basename("a/b") === path.basename("a/b")}:${typeof fs.readFileSync === "function"}`
);
"#,
    )
    .expect("write entry");
    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS module hook shape runs");
    assert_eq!(*out.borrow(), "true:true:true:true:true:true:true\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_dynamic_import_uses_calling_file_as_base() {
    let proj = unique_dir("cjs-dynamic-import");
    let dep = proj.join("dep.mjs");
    std::fs::write(
        &dep,
        "console.log('dep-side-effect');\nexport default 'dep-default';\n",
    )
    .expect("write dep");
    let entry = proj.join("entry.cjs");
    std::fs::write(
        &entry,
        "import('./dep.mjs').then((mod) => {\n  console.log(mod.default);\n});\n",
    )
    .expect("write entry");

    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS dynamic import resolves relative module");

    assert_eq!(*out.borrow(), "dep-side-effect\ndep-default\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_dynamic_import_cjs_target_exposes_exports_directly() {
    let proj = unique_dir("cjs-dynamic-import-cjs");
    let dep = proj.join("dep.cjs");
    std::fs::write(&dep, "exports.answer = 42;\n").expect("write dep");
    let entry = proj.join("entry.cjs");
    std::fs::write(
        &entry,
        "import('./dep.cjs').then((mod) => console.log(typeof mod.answer + ':' + mod.answer + ':' + typeof mod.default));\n",
    )
    .expect("write entry");

    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS dynamic import receives cjs exports");

    assert_eq!(*out.borrow(), "number:42:object\n");
    std::fs::remove_dir_all(&proj).ok();
}
#[tokio::test]
async fn commonjs_dynamic_import_file_url_esm_target_exposes_named_export() {
    let proj = unique_dir("cjs-dynamic-import-file-url-esm");
    let dep = proj.join("dep.mjs");
    std::fs::write(&dep, "export const answer = 42;\n").expect("write dep");
    let entry = proj.join("entry.cjs");
    std::fs::write(
        &entry,
        "const { pathToFileURL } = require('node:url');\nconst { join } = require('node:path');\nvoid (async () => {\n  const depUrl = pathToFileURL(join(__dirname, 'dep.mjs')).href;\n  const mod = await import(depUrl);\n  console.log(typeof mod.answer + ':' + mod.answer);\n})();\n",
    )
    .expect("write entry");

    let (out, mut rt) = node_runtime(
        node::NodeMode::Enabled,
        &proj,
        vec!["meow".to_owned(), entry.to_string_lossy().into_owned()],
    );
    run_file(&mut rt, &entry)
        .await
        .expect("CommonJS dynamic import resolves file URL");

    assert_eq!(*out.borrow(), "number:42\n");
    std::fs::remove_dir_all(&proj).ok();
}
