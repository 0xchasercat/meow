//! Integration tests for `meow-config` (CFG-001).
//!
//! Mapped to the gated invariants:
//! - I-9 (`types-fresh`): generated artifacts are marked + regenerate to identical bytes.
//! - I-1 (`graph-integrity`): the root shim is the single `extends` indirection.
//! - I-11 (`honesty`): the TS-eval boundary is reported, never faked.

use meow_config::{
    generate_shadow_tsconfig, write_root_tsconfig_shim, write_shadow_types, ConfigError,
    MeowConfig, GENERATED_HEADER, ROOT_TSCONFIG_SHIM,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// Self-contained temp-dir guard (no `tempfile` dep). Unique per process via pid +
/// a monotonic counter; removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> TempDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("meow-config-it-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn shadow_path(root: &Path) -> PathBuf {
    root.join(".meow").join("tsconfig.json")
}

// --- I-9: generated artifact is marked, body is valid JSON, reflects config --------

#[test]
fn shadow_has_generated_header_and_valid_json_body() {
    let tmp = TempDir::new();
    let cfg = MeowConfig::default();
    generate_shadow_tsconfig(&cfg, tmp.path()).expect("generate shadow");

    let raw = std::fs::read_to_string(shadow_path(tmp.path())).expect("read shadow");
    let first_line = raw.lines().next().expect("non-empty file");
    assert_eq!(
        first_line, GENERATED_HEADER,
        "first line must be the marker"
    );

    // Remainder (after the JSONC header) parses as JSON.
    let body = &raw[raw.find('\n').expect("header newline") + 1..];
    let json: serde_json::Value = serde_json::from_str(body).expect("body is valid JSON");
    assert_eq!(
        json["compilerOptions"]["strict"],
        serde_json::Value::Bool(true),
        "default config has types.strict = true"
    );
}

// --- I-1: root shim is exactly the one-line extends indirection --------------------

#[test]
fn root_shim_content_is_exact() {
    let tmp = TempDir::new();
    write_root_tsconfig_shim(tmp.path()).expect("write shim");

    let bytes = std::fs::read(tmp.path().join("tsconfig.json")).expect("read shim");
    assert_eq!(bytes, ROOT_TSCONFIG_SHIM.as_bytes());
    // The shim carries `include` (resolved relative to the project root) so tools that
    // read the root tsconfig actually see project source; placing it in the shadow would
    // resolve it to `.meow/` and find nothing (RT-004).
    assert_eq!(
        ROOT_TSCONFIG_SHIM,
        "{ \"extends\": \"./.meow/tsconfig.json\", \"include\": [\".\"] }\n"
    );
}

// --- I-9: regeneration is byte-stable / idempotent --------------------------------

#[test]
fn shadow_regeneration_is_byte_stable() {
    let tmp = TempDir::new();
    let cfg = MeowConfig::default();

    generate_shadow_tsconfig(&cfg, tmp.path()).expect("first gen");
    let first = std::fs::read(shadow_path(tmp.path())).expect("read first");

    generate_shadow_tsconfig(&cfg, tmp.path()).expect("second gen");
    let second = std::fs::read(shadow_path(tmp.path())).expect("read second");

    assert_eq!(first, second, "identical cfg must yield identical bytes");
}

#[test]
fn root_shim_regeneration_is_byte_stable_and_noop_on_correct() {
    let tmp = TempDir::new();

    write_root_tsconfig_shim(tmp.path()).expect("first write");
    let first = std::fs::read(tmp.path().join("tsconfig.json")).expect("read first");

    // Second call must be a no-op on already-correct content.
    write_root_tsconfig_shim(tmp.path()).expect("second write");
    let second = std::fs::read(tmp.path().join("tsconfig.json")).expect("read second");

    assert_eq!(first, second);
    assert_eq!(second, ROOT_TSCONFIG_SHIM.as_bytes());
}

#[test]
fn root_shim_repairs_drifted_content() {
    let tmp = TempDir::new();
    std::fs::write(tmp.path().join("tsconfig.json"), b"{ \"junk\": true }\n").expect("seed drift");

    write_root_tsconfig_shim(tmp.path()).expect("repair");
    let bytes = std::fs::read(tmp.path().join("tsconfig.json")).expect("read repaired");
    assert_eq!(bytes, ROOT_TSCONFIG_SHIM.as_bytes());
}

// --- I-11: honest load boundary ---------------------------------------------------

#[test]
fn load_ts_only_project_is_not_supported() {
    let tmp = TempDir::new();
    std::fs::write(tmp.path().join("meow.config.ts"), b"export default {}").expect("seed ts");

    let err = MeowConfig::load(tmp.path()).expect_err("ts-only must error");
    assert!(
        matches!(err, ConfigError::TsNotSupported),
        "got {err:?}, want TsNotSupported (honest boundary, not a default config)"
    );
}

#[test]
fn load_missing_config_is_not_found() {
    let tmp = TempDir::new();
    let err = MeowConfig::load(tmp.path()).expect_err("empty project must error");
    assert!(matches!(err, ConfigError::NotFound(_)), "got {err:?}");
}

#[test]
fn load_malformed_json_reports_parse_with_path() {
    let tmp = TempDir::new();
    let path = tmp.path().join("meow.config.json");
    std::fs::write(&path, b"{ not json").expect("seed malformed");

    let err = MeowConfig::load(tmp.path()).expect_err("malformed must error");
    match err {
        ConfigError::Parse { path: p, .. } => assert_eq!(p, path),
        other => panic!("got {other:?}, want Parse"),
    }
}

#[test]
fn load_unknown_key_is_rejected() {
    let tmp = TempDir::new();
    std::fs::write(
        tmp.path().join("meow.config.json"),
        br#"{ "totallyUnknownKey": 1 }"#,
    )
    .expect("seed unknown key");

    let err =
        MeowConfig::load(tmp.path()).expect_err("unknown key must error (deny_unknown_fields)");
    assert!(matches!(err, ConfigError::Parse { .. }), "got {err:?}");
}

#[test]
fn load_minimal_config_takes_canon_defaults() {
    let tmp = TempDir::new();
    std::fs::write(tmp.path().join("meow.config.json"), b"{}").expect("seed minimal");

    let cfg = MeowConfig::load(tmp.path()).expect("minimal config loads");
    assert_eq!(cfg, MeowConfig::default());
    assert!(cfg.types.strict, "types.strict defaults true");
    assert!(cfg.test.isolate, "test.isolate defaults true");
}

// --- mapping fidelity -------------------------------------------------------------

#[test]
fn strict_false_propagates_and_erasable_flags_always_present() {
    let tmp = TempDir::new();
    let cfg = MeowConfig {
        types: meow_config::Types { strict: false },
        ..MeowConfig::default()
    };
    generate_shadow_tsconfig(&cfg, tmp.path()).expect("generate");

    let raw = std::fs::read_to_string(shadow_path(tmp.path())).expect("read shadow");
    let body = &raw[raw.find('\n').expect("header newline") + 1..];
    let json: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
    let opts = &json["compilerOptions"];

    assert_eq!(opts["strict"], serde_json::Value::Bool(false));
    for flag in [
        "verbatimModuleSyntax",
        "isolatedModules",
        "erasableSyntaxOnly",
        "noEmit",
    ] {
        assert_eq!(
            opts[flag],
            serde_json::Value::Bool(true),
            "erasable-only flag {flag} must always be present and true (I-3)"
        );
    }
    // RT-004: `include` moved to the root shim (resolved relative to the project root);
    // the shadow carries `lib: ["esnext"]` (no DOM) + a top-level `files` referencing the
    // curated strict-web ambient decl loaded into every program.
    assert!(
        json.get("include").is_none(),
        "shadow must NOT carry `include` (it would resolve to .meow/ — RT-004)"
    );
    assert_eq!(opts["lib"], serde_json::json!(["esnext"]));
    assert_eq!(json["files"], serde_json::json!(["./strict-web.d.ts"]));
    assert!(
        opts.get("types").is_none(),
        "ambient libs load via `files`, not `compilerOptions.types` (RT-004)"
    );
}

// --- RT-004: shadow type files are written verbatim, nested paths create dirs -----

#[test]
fn write_shadow_types_writes_verbatim_and_nests() {
    let tmp = TempDir::new();
    write_shadow_types(
        tmp.path(),
        &[
            ("strict-web.d.ts", "declare var x: number;\n"),
            (
                "types/meow/http.d.ts",
                "export declare function serve(): void;\n",
            ),
        ],
    )
    .expect("write shadow types");

    // Ambient lib lands next to where the shadow tsconfig's `files` references it.
    let ambient =
        std::fs::read_to_string(tmp.path().join(".meow/strict-web.d.ts")).expect("ambient");
    assert_eq!(
        ambient, "declare var x: number;\n",
        "content is written verbatim (no injected header)"
    );

    // A nested module-type path (RT-005 `meow:*`) creates intermediate dirs.
    let nested =
        std::fs::read_to_string(tmp.path().join(".meow/types/meow/http.d.ts")).expect("nested");
    assert_eq!(nested, "export declare function serve(): void;\n");
}
