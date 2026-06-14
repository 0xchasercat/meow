//! UI-001 runtime bridge tests for `meow:ui`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use meow_graph::GraphDb;
use meow_loader::MeowModuleLoader;
use meow_pkg::{Cache, Lockfile};
use meow_runtime::{
    print_sink_extension, ui_extension, ModuleSpecifier, PrintSink, Runtime, RuntimeOptions,
};

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-ui-rt-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn capture() -> (
    Rc<RefCell<String>>,
    Rc<RefCell<String>>,
    deno_core::Extension,
) {
    let out = Rc::new(RefCell::new(String::new()));
    let err = Rc::new(RefCell::new(String::new()));
    let (stdout, stderr) = (out.clone(), err.clone());
    let sink = PrintSink(Rc::new(move |msg: &str, is_err: bool| {
        if is_err {
            stderr.borrow_mut().push_str(msg);
        } else {
            stdout.borrow_mut().push_str(msg);
        }
    }));
    (out, err, print_sink_extension(sink))
}

#[tokio::test]
async fn meow_ui_import_emits_enveloped_lines() {
    let root = unique_dir("bridge");
    let (out, err, sink_ext) = capture();
    let resolver = meow_loader::Resolver::new(
        Arc::new(Cache::with_root(root.join("cache"))),
        Arc::new(Lockfile::new()),
        BTreeMap::new(),
        ModuleSpecifier::from_directory_path(&root).expect("project root URL"),
        meow_runtime::native::native_module_registry(),
    );
    let loader: Rc<dyn meow_runtime::deno_core::ModuleLoader> = Rc::new(MeowModuleLoader::new(
        resolver,
        Rc::new(RefCell::new(GraphDb::new())),
    ));
    let mut runtime = Runtime::new(RuntimeOptions {
            module_loader: loader,
            extensions: vec![sink_ext, ui_extension()],
            max_heap_size: None,
        })
    .expect("runtime initializes");
    let spec = ModuleSpecifier::from_file_path(root.join("main.mjs")).expect("main specifier");
    runtime
        .run_main_module_from_source(
            &spec,
            r#"import { ui, hiss, pounce } from "meow:ui";
ui.purr("hello");
pounce("working");
hiss("boom");
"#
            .to_string(),
        )
        .await
        .expect("module runs");

    assert_eq!(
        *out.borrow(),
        "😸 [Purrfect!] hello\n🐾 [Pouncing...] working\n"
    );
    assert_eq!(*err.borrow(), "🙀 [Bad Kitty!] boom\n");
    std::fs::remove_dir_all(&root).ok();
}
