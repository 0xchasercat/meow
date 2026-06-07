//! RT-005 type-generation + editor-surface tests.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use meow_config::{
    generate_shadow_tsconfig, write_root_tsconfig_shim, write_shadow_types, MeowConfig,
    STRICT_WEB_DTS_FILE,
};
use meow_runtime::native::{native_module_declaration, NATIVE_MODULES};
use meow_runtime::typegen::{
    check_against_dir, emit_to_dir, locate_tsc, TypegenEnv, TypegenError, TypegenLayout,
};

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("meow-rt005-types-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

struct TypegenContext {
    workspace_root: PathBuf,
    home_dir: PathBuf,
    meow_tsc: Option<OsString>,
    compiler: PathBuf,
}

impl TypegenContext {
    fn env(&self) -> TypegenEnv<'_> {
        TypegenEnv {
            project_root: &self.workspace_root,
            home_dir: &self.home_dir,
            meow_tsc: self.meow_tsc.as_deref(),
        }
    }
}

fn typegen_context() -> Option<TypegenContext> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let home_dir = meow_runtime::host::home_dir();
    let meow_tsc = meow_runtime::host::meow_tsc();
    let env = TypegenEnv {
        project_root: &workspace_root,
        home_dir: &home_dir,
        meow_tsc: meow_tsc.as_deref(),
    };
    match locate_tsc(&env) {
        Ok(compiler) => Some(TypegenContext {
            workspace_root,
            home_dir,
            meow_tsc,
            compiler,
        }),
        Err(err) => {
            eprintln!("skipping RT-005 typegen tests: {err}");
            None
        }
    }
}

fn runtime_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn shadow_files() -> Vec<(String, &'static str)> {
    let mut files = Vec::with_capacity(1 + NATIVE_MODULES.len());
    files.push((
        STRICT_WEB_DTS_FILE.to_owned(),
        meow_runtime::web::STRICT_WEB_DTS,
    ));
    for name in NATIVE_MODULES {
        if let Some(decl) = native_module_declaration(name) {
            files.push((format!("types/meow/{name}.d.ts"), decl));
        }
    }
    files
}

fn sync_shadow(root: &Path) {
    generate_shadow_tsconfig(&MeowConfig::default(), root).expect("generate shadow tsconfig");
    write_root_tsconfig_shim(root).expect("write root tsconfig shim");
    let files = shadow_files();
    let refs = files
        .iter()
        .map(|(path, content)| (path.as_str(), *content))
        .collect::<Vec<_>>();
    write_shadow_types(root, &refs).expect("write shadow type files");
}

fn run_tsc_check(ctx: &TypegenContext, root: &Path) -> std::process::Output {
    std::process::Command::new(&ctx.compiler)
        .arg("-p")
        .arg(root.join("tsconfig.json"))
        .arg("--noEmit")
        .current_dir(root)
        .output()
        .expect("run tsc")
}

fn workspace_contains_manifest_ref(path: &Path, needle: &[u8]) -> bool {
    if path.ends_with(".git")
        || path.ends_with("target")
        || path.ends_with(".meow")
        || path.ends_with("harness")
    {
        return false;
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if metadata.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(_) => return false,
        };
        for entry in entries.filter_map(Result::ok) {
            if workspace_contains_manifest_ref(&entry.path(), needle) {
                return true;
            }
        }
        return false;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let relevant = matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
    );
    if !relevant {
        return false;
    }
    match fs::read(path) {
        Ok(bytes) => bytes.windows(needle.len()).any(|window| window == needle),
        Err(_) => false,
    }
}

fn combined_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

#[test]
fn typegen_roundtrips() {
    let Some(ctx) = typegen_context() else {
        return;
    };

    let runtime_root = runtime_root();
    let source_dir = runtime_root.join("src/js/meow");
    let committed_dir = runtime_root.join("types/meow");
    let layout = TypegenLayout {
        source_dir: &source_dir,
        committed_types_dir: &committed_dir,
    };

    check_against_dir(&ctx.env(), &layout).expect("committed meow types stay fresh");

    let emit_a = unique_dir("emit-a");
    let emit_b = unique_dir("emit-b");
    emit_to_dir(&ctx.env(), &layout, &emit_a).expect("emit declarations to temp A");
    emit_to_dir(&ctx.env(), &layout, &emit_b).expect("emit declarations to temp B");

    let committed_http =
        fs::read(committed_dir.join("http.d.ts")).expect("read committed http.d.ts");
    let emit_a_http = fs::read(emit_a.join("http.d.ts")).expect("read emitted A http.d.ts");
    let emit_b_http = fs::read(emit_b.join("http.d.ts")).expect("read emitted B http.d.ts");
    assert_eq!(
        emit_a_http, committed_http,
        "emit matches the committed declaration"
    );
    assert_eq!(emit_a_http, emit_b_http, "emit is byte-stable across runs");

    let drift_dir = unique_dir("drift");
    fs::create_dir_all(&drift_dir).expect("create drift dir");
    fs::write(
        drift_dir.join("http.d.ts"),
        format!(
            "{}\n// drift\n",
            String::from_utf8(committed_http).expect("utf8 dts")
        ),
    )
    .expect("seed drifted dts");
    let drift_layout = TypegenLayout {
        source_dir: &source_dir,
        committed_types_dir: &drift_dir,
    };
    let err = check_against_dir(&ctx.env(), &drift_layout).expect_err("drift is detected");
    assert!(
        matches!(err, TypegenError::Drift { .. }),
        "typed drift error: {err:?}"
    );
    assert!(
        err.to_string().contains("http.d.ts"),
        "drift names the file: {err}"
    );

    assert!(
        !workspace_contains_manifest_ref(&ctx.workspace_root, b"@types/meow"),
        "the workspace must not declare an @types/meow dependency in its manifests"
    );

    std::fs::remove_dir_all(&emit_a).ok();
    std::fs::remove_dir_all(&emit_b).ok();
    std::fs::remove_dir_all(&drift_dir).ok();
}

#[test]
fn editor_resolves_meow_http() {
    let Some(ctx) = typegen_context() else {
        return;
    };

    let ok_root = unique_dir("editor-ok");
    sync_shadow(&ok_root);
    let shadow_tsconfig =
        fs::read_to_string(ok_root.join(".meow/tsconfig.json")).expect("read shadow tsconfig");
    assert!(
        shadow_tsconfig.contains("\"meow:*\""),
        "paths entry added: {shadow_tsconfig}"
    );
    assert!(
        shadow_tsconfig.contains("./types/meow/*"),
        "paths target added: {shadow_tsconfig}"
    );
    assert!(
        ok_root.join(".meow/types/meow/http.d.ts").is_file(),
        "meow:http declaration synced into .meow/types/"
    );
    fs::write(
        ok_root.join("ok.ts"),
        "import { serve } from 'meow:http';\nserve(() => new Response('x'));\n",
    )
    .expect("write ok fixture");
    let ok = run_tsc_check(&ctx, &ok_root);
    assert!(
        ok.status.success(),
        "editor fixture should typecheck:\n{}",
        combined_output(&ok)
    );

    let bad_root = unique_dir("editor-bad");
    sync_shadow(&bad_root);
    fs::write(
        bad_root.join("bad.ts"),
        "import { serve } from 'meow:http';\nserve(123);\n",
    )
    .expect("write bad fixture");
    let bad = run_tsc_check(&ctx, &bad_root);
    assert!(!bad.status.success(), "bad fixture must fail to typecheck");
    let diagnostics = combined_output(&bad);
    assert!(
        diagnostics.contains("Argument of type 'number'") || diagnostics.contains("not assignable"),
        "serve(123) should be a type error, got:\n{diagnostics}"
    );

    std::fs::remove_dir_all(&ok_root).ok();
    std::fs::remove_dir_all(&bad_root).ok();
}
