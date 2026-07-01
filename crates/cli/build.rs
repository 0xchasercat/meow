//! Build script: emit linker flags, bake the V8 startup snapshot, and capture
//! the residual lazy ESM/JS sources deno_core needs at runtime.
//!
//! Runs automatically during `cargo build`. Builds the COMPLETE meow runtime
//! extension stack (the exact order `meow run` uses, so V8 extension indices
//! match), serializes a `JsRuntimeForSnapshot` into `OUT_DIR/meow-snapshot.bin`,
//! and — because `lazy_loaded_esm`/`lazy_loaded_js` sources are NOT baked into the
//! V8 heap — extracts those sources from the built extensions and emits
//! `OUT_DIR/snapshot_data.rs` (`SNAPSHOT_BLOB` + `RESIDUAL_LAZY_ESM` +
//! `RESIDUAL_LAZY_JS`), which `src/main.rs` `include!`s. If snapshot generation
//! fails, the build fails.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../runtime/src/js");
    println!("cargo:rerun-if-changed=../runtime/src/js/node_globals.js");
    deno_napi::print_linker_flags("meow");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));

    let extensions = build_full_extension_set();

    // deno_core keeps lazy_loaded_esm/js out of the V8 snapshot heap, so they must
    // be re-supplied at runtime via RuntimeOptions. Pull them off the built
    // Extensions (specifier is already the lookup key, e.g. "node:http").
    let mut esm: Vec<(String, String)> = Vec::new();
    let mut js: Vec<(String, String)> = Vec::new();
    for ext in &extensions {
        for f in ext.lazy_loaded_esm_files.iter() {
            let (code, _) = meow_runtime::maybe_transpile_source(
                f.specifier.to_string().into(),
                f.load().expect("lazy esm source"),
            )
            .unwrap_or_else(|e| panic!("transpile lazy esm {}: {e}", f.specifier));
            esm.push((f.specifier.to_string(), code.as_str().to_string()));
        }
        for f in ext.lazy_loaded_js_files.iter() {
            let (code, _) = meow_runtime::maybe_transpile_source(
                f.specifier.to_string().into(),
                f.load().expect("lazy js source"),
            )
            .unwrap_or_else(|e| panic!("transpile lazy js {}: {e}", f.specifier));
            js.push((f.specifier.to_string(), code.as_str().to_string()));
        }
    }

    let runtime = deno_core::JsRuntimeForSnapshot::new(deno_core::RuntimeOptions {
        extensions,
        extension_transpiler: Some(std::rc::Rc::new(|specifier, source| {
            meow_runtime::maybe_transpile_source(specifier, source)
        })),
        ..Default::default()
    });
    let snapshot = runtime.snapshot();
    std::fs::write(out_dir.join("meow-snapshot.bin"), &*snapshot)
        .unwrap_or_else(|e| panic!("failed to write V8 snapshot: {e}"));

    let mut data = String::new();
    data.push_str(
        "pub static SNAPSHOT_BLOB: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/meow-snapshot.bin\"));\n\n",
    );
    emit_table(&mut data, &out_dir, "RESIDUAL_LAZY_ESM", "lazy_esm", &esm);
    emit_table(&mut data, &out_dir, "RESIDUAL_LAZY_JS", "lazy_js", &js);
    std::fs::write(out_dir.join("snapshot_data.rs"), data)
        .unwrap_or_else(|e| panic!("failed to write snapshot_data.rs: {e}"));
}

fn emit_table(
    out: &mut String,
    out_dir: &std::path::Path,
    name: &str,
    prefix: &str,
    items: &[(String, String)],
) {
    out.push_str(&format!("pub static {name}: &[(&str, &str)] = &[\n"));
    for (i, (spec, src)) in items.iter().enumerate() {
        let fname = format!("{prefix}_{i}.src");
        std::fs::write(out_dir.join(&fname), src)
            .unwrap_or_else(|e| panic!("failed to write {fname}: {e}"));
        out.push_str(&format!(
            "    ({spec:?}, include_str!(concat!(env!(\"OUT_DIR\"), \"/{fname}\"))),\n"
        ));
    }
    out.push_str("];\n\n");
}

fn build_full_extension_set() -> Vec<deno_core::Extension> {
    let mut extensions = vec![meow_runtime::meow_runtime::init()];
    extensions.push(meow_runtime::http_extension());
    extensions.push(meow_runtime::ui_extension());

    let cache = Arc::new(meow_pkg::Cache::with_root("/tmp/meow-snapshot-cache"));
    let lockfile = Arc::new(meow_pkg::Lockfile::new());
    let root_deps: BTreeMap<meow_pkg::PackageName, meow_pkg::Version> = BTreeMap::new();
    let project_root =
        deno_core::ModuleSpecifier::parse("file:///meow-snapshot-root/").expect("valid root url");
    let native = meow_runtime::native::native_module_registry();
    let resolver = meow_loader::Resolver::new(cache, lockfile, root_deps, project_root, native);
    extensions.push(meow_loader::cjs_resolve_extension(resolver));

    let hermetic_cfg = meow_runtime::hermetic::HermeticConfig::default().with_env_all();
    meow_runtime::hermetic::pin_deterministic_intl(&hermetic_cfg);
    extensions.extend(meow_runtime::hermetic::extensions(hermetic_cfg));

    let node_opts = meow_runtime::node::NodeOptions {
        mode: meow_runtime::node::NodeMode::Enabled,
        argv: vec!["meow".to_string(), "snapshot-placeholder".to_string()],
        main_module: None,
        cwd: PathBuf::from("/"),
        env: BTreeMap::from([("MEOW_SNAPSHOT_BUILD".to_string(), "1".to_string())]),
        deno_node_services: None,
        caps: None,
        user_agent: Some("meow/snapshot".to_string()),
        // === SEC-001 === snapshot build: no enforcement baked in (allow_all).
        sandbox: None,
    };
    extensions.extend(meow_runtime::node::extensions(node_opts));

    // === WORKER-001 === bake the cooperative-isolate worker ops into the
    // snapshot so `core.ops.op_meow_worker_*` resolve from it (runtime-only
    // extension ops are NOT exposed on `core.ops` under a snapshot). `None`
    // spawner at snapshot time — the binary edge installs the real spawner at
    // runtime via `worker_extension(Some(..))`. MUST stay LAST to match the
    // order in `run_native_request` + `build_worker_runtime` (op indices are
    // positional).
    extensions.push(meow_runtime::worker::worker_extension(None));
    extensions
}
