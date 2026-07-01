//! `meow test` — discover test files, execute each through a hermetic isolate,
//! and render results through meow-ui. Tests import from `meow:test` for the
//! test/expect API. Reuses [`RuntimeNodeBridge`] / [`build_runtime_context`]
//! from `run`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::cli::commands::run::{build_runtime_context, RuntimeNodeBridge};
use crate::cli::{hiss, purr, ui, TestArgs};

// === TEST-001 ===
/// `meow test` — discover test files, execute each through a hermetic isolate,
/// and render results through meow-ui. Tests import from `meow:test` for the
/// test/expect API.
pub fn cmd_test(_args: &TestArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow test: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    // Use cwd for test discovery — find_project_root may walk past project boundaries
    // when no meow.config.json or package.json exists in the project tree.
    let root = cwd.clone();

    let test_files = discover_test_files(&root);
    if test_files.is_empty() {
        purr("meow test: no test files found");
        return ExitCode::SUCCESS;
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            hiss(&format!("meow test: cannot start async runtime: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let test_count = test_files.len();
    let mut total_passed = 0usize;
    let mut total_failed = 0usize;

    for file in &test_files {
        let display = file.strip_prefix(&root).unwrap_or(file).display();
        let label = display.to_string();
        ui().pounce(&label);

        match runtime.block_on(run_test_file_inner(&root, file)) {
            Ok(results) => {
                for result in &results {
                    let name = result["name"].as_str().unwrap_or("<unknown>");
                    let passed = result["passed"].as_bool().unwrap_or(false);
                    if passed {
                        total_passed += 1;
                        ui().purr(&format!("  ✓ {name}"));
                    } else {
                        total_failed += 1;
                        let msg = result["error"].as_str().unwrap_or("unknown error");
                        ui().hiss(&format!("  ✗ {name}"));
                        if let Some(stack) = result["stack"].as_str() {
                            let first_line = stack.lines().next().unwrap_or(msg);
                            ui().hiss(&format!("    {first_line}"));
                        } else {
                            ui().hiss(&format!("    {msg}"));
                        }
                    }
                }
            }
            Err(err) => {
                total_failed += 1;
                ui().hiss(&format!("  ✗ {} — file error: {err}", file.display()));
            }
        }
    }

    let summary = if total_failed == 0 {
        format!(
            "{} passed · {} file{}",
            total_passed,
            test_count,
            if test_count == 1 { "" } else { "s" },
        )
    } else {
        format!(
            "{} passed, {} failed · {} file{}",
            total_passed,
            total_failed,
            test_count,
            if test_count == 1 { "" } else { "s" },
        )
    };

    let u = ui();
    let tone = if total_failed == 0 {
        meow_ui::Tone::Purr
    } else {
        meow_ui::Tone::Hiss
    };
    let lines = vec![u.sigil(tone, &summary)];
    u.panel("meow test", &lines);

    if total_failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn run_test_file_inner(root: &Path, file: &Path) -> Result<Vec<serde_json::Value>, String> {
    let ctx = build_runtime_context(root, false).map_err(|e| e.to_string())?;
    let resolver = meow_loader::Resolver::from_resolution(
        &ctx.graph,
        ctx.cache.clone(),
        ctx.project_root.clone(),
        meow_runtime::native::native_module_registry(),
    );
    let loader: std::rc::Rc<dyn meow_runtime::deno_core::ModuleLoader> =
        std::rc::Rc::new(meow_loader::MeowModuleLoader::new(
            resolver.clone(),
            std::rc::Rc::new(std::cell::RefCell::new(meow_graph::GraphDb::new())),
        ));
    let deno_node_bridge: std::rc::Rc<dyn meow_runtime::node::DenoNodeBridge> =
        std::rc::Rc::new(RuntimeNodeBridge::new(resolver.clone(), ctx.cache.clone()));
    let deno_node_services =
        meow_runtime::node::DenoNodeServicesBuilder::new(deno_node_bridge).build();

    let caps: meow_runtime::web::NetCaps = std::sync::Arc::new(meow_runtime::AllowAll);
    let mut extensions = vec![
        meow_runtime::http_extension(),
        meow_runtime::ui_extension(),
        meow_runtime::test_extension(),
        meow_loader::cjs_resolve_extension(resolver.clone()),
    ];

    // Tests run with full hermetic by default (deterministic).
    let hermetic = meow_runtime::hermetic::HermeticConfig::default();
    meow_runtime::hermetic::pin_deterministic_intl(&hermetic);
    extensions.extend(meow_runtime::hermetic::extensions(hermetic));

    let node_argv = vec!["meow".to_owned(), file.to_string_lossy().into_owned()];
    extensions.extend(meow_runtime::node::extensions(
        meow_runtime::node::NodeOptions {
            mode: meow_runtime::node::NodeMode::StrictWeb,
            argv: node_argv,
            main_module: Some(file.to_string_lossy().into_owned()),
            cwd: root.to_path_buf(),
            env: BTreeMap::new(),
            deno_node_services: Some(deno_node_services),
            caps: Some(caps),
            user_agent: Some(format!("meow/{}", env!("CARGO_PKG_VERSION"))),
            // === SEC-001 === `meow test` runs the user's own project: trusted.
            sandbox: None,
        },
    ));

    let mut runtime = meow_runtime::Runtime::new(meow_runtime::RuntimeOptions {
        module_loader: loader,
        extensions,
        max_heap_size: None,
        startup_snapshot: Some(crate::SNAPSHOT_BLOB),
        residual_lazy_js_sources: crate::RESIDUAL_LAZY_JS,
        residual_lazy_esm_sources: crate::RESIDUAL_LAZY_ESM,
        v8_flags: None,
    })
    .map_err(|e| e.to_string())?;
    runtime
        .apply_hermetic_shadows()
        .map_err(|e| e.to_string())?;

    let spec = meow_runtime::ModuleSpecifier::from_file_path(file)
        .map_err(|()| format!("invalid test file path: {}", file.display()))?;

    runtime
        .run_main_module(&spec)
        .await
        .map_err(|e| format!("{}", e))?;

    // After module evaluation, call the test runner. Results are stored in OpState
    // via the op_test_store_results op.
    runtime
        .execute_script(
            "meow:test/runner",
            String::from("globalThis.__meowTestRunAll()"),
        )
        .map_err(|e| format!("test runner error: {e}"))?;

    let result_str = runtime
        .take_test_results()
        .ok_or_else(|| "no test results stored — did the test file call test()?".to_owned())?;

    let entries: Vec<serde_json::Value> =
        serde_json::from_str(&result_str).map_err(|e| format!("test results parse error: {e}"))?;

    Ok(entries)
}

/// Discover test files in the project tree. Filters ignored directories at push time
/// to avoid traversing into node_modules, target, .git, and hidden directories.
fn discover_test_files(root: &Path) -> Vec<PathBuf> {
    const TEST_EXTENSIONS: &[&str] = &["ts", "js", "tsx", "jsx", "mts", "mjs", "cts", "cjs"];
    let mut files = Vec::new();
    let mut queue = vec![root.to_path_buf()];

    while let Some(dir) = queue.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name,
                None => continue,
            };
            if path.is_dir() {
                // Skip ignored directories before pushing
                if !file_name.starts_with('.')
                    && file_name != "node_modules"
                    && file_name != "target"
                    && file_name != "vendor"
                {
                    queue.push(path);
                }
            } else if path.is_file() {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                if TEST_EXTENSIONS.contains(&ext)
                    && (stem.ends_with(".test") || stem.ends_with(".spec"))
                {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    files
}
// === /TEST-001 ===
