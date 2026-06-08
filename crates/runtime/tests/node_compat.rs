use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use deno_core::url::Url;
use meow_graph::GraphDb;
use meow_loader::{MeowModuleLoader, Resolver};
use meow_pkg::{Cache, Lockfile, PackageName, Version};
use meow_runtime::{
    hermetic, node, print_sink_extension, web, AllowAll, ModuleSpecifier, PrintSink, Runtime,
    RuntimeError, RuntimeOptions,
};

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
        let dirname = std::path::PathBuf::from(&filename)
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

fn runtime_parts_for(project_root: &Path) -> (Rc<dyn deno_core::ModuleLoader>, Resolver) {
    let resolver = Resolver::new(
        Arc::new(Cache::with_root(project_root.join("cache"))),
        Arc::new(Lockfile::new()),
        BTreeMap::<PackageName, Version>::new(),
        dir_url(project_root),
        meow_runtime::native::native_module_registry(),
    );
    let loader = Rc::new(MeowModuleLoader::new(
        resolver.clone(),
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    (loader, resolver)
}

fn node_runtime(
    mode: node::NodeMode,
    cwd: &Path,
    argv: Vec<String>,
) -> (Rc<RefCell<String>>, Runtime) {
    let (loader, resolver) = runtime_parts_for(cwd);
    let (out, sink) = capture();
    let caps: web::NetCaps = Arc::new(AllowAll);
    let mut extensions = web::extensions(web::WebOptions {
        caps,
        user_agent: "meow-test".to_owned(),
    });
    let hermetic_cfg = if matches!(mode, node::NodeMode::Enabled) {
        hermetic::HermeticConfig::default().with_env_all()
    } else {
        hermetic::HermeticConfig::default()
    };
    extensions.extend(hermetic::extensions(hermetic_cfg));
    extensions.extend(node::extensions(node::NodeOptions {
        mode,
        argv,
        cwd: cwd.to_path_buf(),
        env: BTreeMap::new(),
        cjs_resolver: Some(Arc::new(TestCjsResolver(Mutex::new(resolver)))),
    }));
    extensions.push(sink);

    let runtime = Runtime::new(RuntimeOptions {
        module_loader: loader,
        extensions,
    })
    .expect("runtime initializes");
    (out, runtime)
}

async fn run_src(rt: &mut Runtime, spec: &str, src: &str) -> Result<(), RuntimeError> {
    let specifier = ModuleSpecifier::parse(spec).expect("valid specifier");
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
        "file:///path.mjs",
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
        "file:///dns.mjs",
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
        "file:///buffer.mjs",
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
        "file:///process.mjs",
        r#"
        console.log(process.argv.join("|"));
        console.log(process.cwd());
        console.log(process.platform + ":" + process.arch);
        "#,
    )
    .await
    .expect("module runs");
    let expected = format!(
        "meow|{entry}|one|two\n{}\ndarwin:arm64\n",
        proj.to_string_lossy()
    );
    assert_eq!(*out.borrow(), expected);
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
        "file:///fs-default.mjs",
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
        "file:///fs-eq.mjs",
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
    run_file(&mut rt, &entry)
        .await
        .expect("strict-web CJS still instantiates");
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
        "file:///strict-web.mjs",
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
        "file:///events.mjs",
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
        "file:///assert.mjs",
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
        "file:///crypto.mjs",
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
