//! V8 startup snapshot creator for meow.
//!
//! Builds a JsRuntimeForSnapshot with the COMPLETE meow extension stack
//! (including deno_node) and serializes it to a blob. The Node bootstrap
//! state (argv/cwd/env) is baked into the snapshot with placeholder values;
//! the CLI calls `refresh_bootstrap_state()` at runtime to set the real values.
//!
//! Usage:
//!   cargo run -p meow-snapshot -- --output target/meow-snapshot.bin

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let output_path = args
        .iter()
        .position(|a| a == "--output")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/meow-snapshot.bin"));

    eprintln!("[meow-snapshot] creating V8 startup snapshot...");

    let extensions = build_full_extension_set();

    eprintln!(
        "[meow-snapshot] {} extensions loaded, creating snapshot...",
        extensions.len()
    );

    // Prepend the meow_runtime extension (just like Runtime::new does).
    let mut all_extensions = vec![meow_runtime::meow_runtime::init()];
    all_extensions.extend(extensions);

    let runtime = deno_core::JsRuntimeForSnapshot::new(deno_core::RuntimeOptions {
        extensions: all_extensions,
        extension_transpiler: Some(std::rc::Rc::new(|specifier, source| {
            meow_runtime::maybe_transpile_source(specifier, source)
        })),
        ..Default::default()
    });

    let snapshot_blob = runtime.snapshot();

    eprintln!(
        "[meow-snapshot] snapshot created: {} bytes",
        snapshot_blob.len()
    );

    std::fs::write(&output_path, &*snapshot_blob)
        .unwrap_or_else(|e| panic!("failed to write snapshot to {}: {}", output_path.display(), e));

    eprintln!(
        "[meow-snapshot] snapshot written to {}",
        output_path.display()
    );
}

/// Build the COMPLETE extension stack in the exact order used by `meow run`.
/// All extensions must be present so that V8 extension indices match at runtime.
fn build_full_extension_set() -> Vec<deno_core::Extension> {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let mut extensions = Vec::new();

    // === RT-004: Web globals ===
    // In NodeMode::Enabled (the default), web extensions are NOT loaded separately.

    // === RT-005: HTTP extension ===
    extensions.push(meow_runtime::http_extension());

    // === UI-001: UI extension ===
    extensions.push(meow_runtime::ui_extension());

    // === CJS resolve extension ===
    let cache = Arc::new(meow_pkg::Cache::with_root("/tmp/meow-snapshot-cache"));
    let lockfile = Arc::new(meow_pkg::Lockfile::new());
    let root_deps: BTreeMap<meow_pkg::PackageName, meow_pkg::Version> = BTreeMap::new();
    let project_root =
        deno_core::ModuleSpecifier::parse("file:///tmp/meow-snapshot-root/").unwrap();
    let native = meow_runtime::native::native_module_registry();
    let resolver = meow_loader::Resolver::new(cache, lockfile, root_deps, project_root, native);
    extensions.push(meow_loader::cjs_resolve_extension(resolver));

    // === RT-006: Hermetic extensions ===
    let hermetic_cfg = meow_runtime::hermetic::HermeticConfig::default().with_env_all();
    meow_runtime::hermetic::pin_deterministic_intl(&hermetic_cfg);
    extensions.extend(meow_runtime::hermetic::extensions(hermetic_cfg));

    // === RT-007: Node compatibility extensions ===
    // Included in the snapshot so V8 extension indices match.
    // The Node bootstrap state (argv/cwd/env) will be refreshed at runtime
    // via `refresh_bootstrap_state()`.
    let node_opts = meow_runtime::node::NodeOptions {
        mode: meow_runtime::node::NodeMode::Enabled,
        argv: vec!["meow".to_string(), "snapshot-placeholder".to_string()],
        cwd: PathBuf::from("/"),
        env: BTreeMap::new(),
        deno_node_services: None,
        caps: None,
        user_agent: Some("meow/snapshot".to_string()),
    };
    extensions.extend(meow_runtime::node::extensions(node_opts));

    extensions
}
