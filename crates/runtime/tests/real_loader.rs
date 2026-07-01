use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use meow_graph::GraphDb;
use meow_loader::{MeowModuleLoader, Resolver};
use meow_pkg::{Cache, Lockfile};
use meow_runtime::{deno_core, ModuleSpecifier};

pub fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-runtime-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create test project root");
    dir
}

pub fn loader_for(project_root: &Path) -> Rc<dyn deno_core::ModuleLoader> {
    let resolver = Resolver::new(
        Arc::new(Cache::with_root(project_root.join("cache"))),
        Arc::new(Lockfile::new()),
        BTreeMap::new(),
        ModuleSpecifier::from_directory_path(project_root).expect("project root URL"),
        meow_runtime::native::native_module_registry(),
    );
    Rc::new(MeowModuleLoader::new(
        resolver,
        Rc::new(RefCell::new(GraphDb::new())),
    ))
}
