use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

fn loader_for(project_root: &Path) -> Rc<dyn deno_core::ModuleLoader> {
    let resolver = Resolver::new(
        Arc::new(Cache::with_root(project_root.join("cache"))),
        Arc::new(Lockfile::new()),
        BTreeMap::<PackageName, Version>::new(),
        dir_url(project_root),
        meow_runtime::native::native_module_registry(),
    );
    Rc::new(MeowModuleLoader::new(
        resolver,
        Rc::new(RefCell::new(GraphDb::new())),
    ))
}

fn node_runtime(
    mode: node::NodeMode,
    cwd: &Path,
    argv: Vec<String>,
) -> (Rc<RefCell<String>>, Runtime) {
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
    }));
    extensions.push(sink);

    let runtime = Runtime::new(RuntimeOptions {
        module_loader: loader_for(cwd),
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
