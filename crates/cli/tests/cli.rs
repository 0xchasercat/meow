//! Integration tests for the `meow` binary surface (DIST-001).
//!
//! Exercises the real built binary via `assert_cmd`. The invariant under test is
//! the CRAFT "the action must DO the work" line, mechanically enforced: no command
//! silently succeeds, and "not built yet" (exit 3) is distinguishable from "used
//! wrong" (clap's exit 2).

use std::process::Command as StdCommand;

use assert_cmd::Command;
use predicates::prelude::*;

fn meow() -> Command {
    Command::cargo_bin("meow").expect("meow binary builds")
}

/// Build a minimal npm-style gzip-tar (members under `package/`) so the cache
/// holds a real package archive — LOAD-003 reads tarballs, not raw module bytes.
fn npm_tarball(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut builder = tar::Builder::new(Vec::new());
    for (name, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        builder
            .append_data(&mut header, format!("package/{name}"), content.as_bytes())
            .expect("tar append");
    }
    let tar_bytes = builder.into_inner().expect("tar finish");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar_bytes).expect("gz write");
    gz.finish().expect("gz finish")
}

#[test]
fn version_and_help_succeed() {
    meow()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("meow"));
    meow()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("run"));
}

/// `(argv, verb, phase)` for every subcommand, with minimal valid args.
const CASES: &[(&[&str], &str, &str)] = &[
    (&["add", "p"], "add", "P2"),
    (&["remove", "p"], "remove", "P2"),
    (&["task", "t"], "task", "P4"),
    (&["test"], "test", "P6"),
    (&["check"], "check", "P3"),
    (&["lint"], "lint", "P3"),
    (&["fmt"], "fmt", "P3"),
    (&["bundle", "x.ts"], "bundle", "P3"),
    (&["why-slow"], "why-slow", "P6"),
    (&["why-large"], "why-large", "P6"),
    (&["trace", "x.ts"], "trace", "P6"),
    (&["profile", "x.ts"], "profile", "P6"),
    (&["doctor"], "doctor", "P6"),
];

#[test]
fn every_subcommand_stub_is_honest() {
    assert_eq!(
        CASES.len(),
        13,
        "13 stub subcommands (sync/run/dev/install/types/why-dep are real; doctor reverts to the P6 stub after CFG-003 retires package.json ownership)"
    );
    for (argv, verb, phase) in CASES {
        let expected = format!("meow: not yet implemented — `{verb}` lands in PLAN {phase}");
        meow()
            .args(*argv)
            .assert()
            .code(3)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(expected));
    }
}

#[test]
fn install_vfs_mode_remains_an_honest_stub() {
    meow()
        .args(["install", "--mode", "vfs"])
        .assert()
        .code(3)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("PKG-004"));
}

#[test]
fn sync_generates_shadow_configs() {
    // `meow sync` is real (CFG-001): regenerates the shadow tsconfig + root shim.
    let tmp = std::env::temp_dir().join(format!("meow-sync-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    std::fs::write(tmp.join("meow.config.json"), "{}").expect("write config");
    meow().current_dir(&tmp).arg("sync").assert().success();
    assert!(
        tmp.join(".meow/tsconfig.json").is_file(),
        "shadow .meow/tsconfig.json generated"
    );
    assert!(
        tmp.join("tsconfig.json").is_file(),
        "root tsconfig.json shim generated"
    );
    assert!(
        !tmp.join("package.json").exists(),
        "sync must not generate or overwrite package.json"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_executes_a_trivial_mjs_module() {
    // `meow run` is real (RT-001): a plain ESM file runs through V8 and prints.
    let tmp = std::env::temp_dir().join(format!("meow-run-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let entry = tmp.join("hello.mjs");
    std::fs::write(&entry, r#"console.log("hello from meow")"#).expect("write entry");
    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .success()
        .stdout(predicate::str::contains("hello from meow"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_surfaces_uncaught_errors_without_panicking() {
    // An uncaught JS error is a rendered diagnostic on stderr + non-zero exit,
    // never a Rust panic / backtrace.
    let tmp = std::env::temp_dir().join(format!("meow-run-err-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let entry = tmp.join("boom.mjs");
    std::fs::write(&entry, r#"throw new Error("boom")"#).expect("write entry");
    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .failure()
        .stderr(predicate::str::contains("boom"))
        .stderr(predicate::str::contains("panicked").not());
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_executes_first_party_cjs() {
    let tmp = std::env::temp_dir().join(format!("meow-run-cjs-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let entry = tmp.join("app.cjs");
    std::fs::write(
        &entry,
        r#"module.exports = { value: "ok" }; console.log(module.exports.value)"#,
    )
    .expect("write entry");
    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .success()
        .stdout(predicate::str::contains("ok"))
        .stderr(predicate::str::contains("CommonJS").not());
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_forwards_argv_after_double_dash_to_process_argv() {
    let tmp = std::env::temp_dir().join(format!("meow-run-argv-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let entry = tmp.join("argv.mjs");
    std::fs::write(
        &entry,
        r#"console.log(JSON.stringify(process.argv.slice(-1)))"#,
    )
    .expect("write entry");
    meow()
        .arg("run")
        .arg(&entry)
        .arg("--")
        .arg("foo")
        .assert()
        .success()
        .stdout(predicate::str::contains("[\"foo\"]"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn bad_usage_is_distinct() {
    // No subcommand and a missing required arg are clap usage errors (exit 2),
    // never the "not built" exit 3.
    meow().assert().code(2);
    meow().arg("run").assert().code(2);
}

#[test]
fn footprint_probe_runs_and_is_under_budget() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let out = StdCommand::new("sh")
        .arg("scripts/footprint.sh")
        .current_dir(root)
        .output()
        .expect("footprint.sh runs");
    assert!(
        out.status.success(),
        "footprint exit {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("status=ok"), "stdout: {stdout}");
    let gzip: u64 = stdout
        .split_whitespace()
        .find_map(|t| t.strip_prefix("gzip="))
        .and_then(|n| n.parse().ok())
        .expect("gzip=<n> present in probe output");
    assert!(
        gzip < 62_914_560,
        "gzip {gzip} over the 60 MB compressed budget"
    );
}

// === LOAD-001 ===
/// A unique temp dir per call (pid + monotonic counter — no rand/clock, P16).
fn load_tmp(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-load-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

#[test]
fn run_executes_package_json_script_from_nested_cwd_and_sets_lifecycle_env() {
    let proj = load_tmp("script-dev");
    let nested = proj.join("src").join("client");
    std::fs::create_dir_all(&nested).expect("nested cwd");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "dev": "node ./dev.cjs --script-flag" } }"#,
    )
    .expect("write package.json");
    std::fs::write(
        proj.join("dev.cjs"),
        r#"console.log(`EVENT=${process.env.npm_lifecycle_event}`);
console.log(`SCRIPT=${process.env.npm_lifecycle_script}`);
console.log(`INIT=${process.env.INIT_CWD}`);
console.log(`CWD=${process.cwd()}`);
console.log(`ARGV=${JSON.stringify(process.argv.slice(2))}`);"#,
    )
    .expect("write dev script");

    let proj_display = std::fs::canonicalize(&proj)
        .expect("canon project")
        .to_string_lossy()
        .into_owned();
    let nested_display = std::fs::canonicalize(&nested)
        .expect("canon nested")
        .to_string_lossy()
        .into_owned();
    meow()
        .current_dir(&nested)
        .arg("run")
        .arg("dev")
        .arg("--")
        .arg("from-cli")
        .assert()
        .success()
        .stdout(predicate::str::contains("EVENT=dev"))
        .stdout(predicate::str::contains(
            "SCRIPT=node ./dev.cjs --script-flag",
        ))
        .stdout(predicate::str::contains(format!("INIT={nested_display}")))
        .stdout(predicate::str::contains(format!("CWD={proj_display}")))
        .stdout(predicate::str::contains(
            r#"ARGV=["--script-flag","from-cli"]"#,
        ));
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn dev_shorthand_runs_the_dev_script() {
    let proj = load_tmp("dev-short");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "dev": "node ./dev.cjs" } }"#,
    )
    .expect("write package.json");
    std::fs::write(proj.join("dev.cjs"), r#"console.log("dev shortcut ok")"#)
        .expect("write dev script");

    meow()
        .current_dir(&proj)
        .arg("dev")
        .assert()
        .success()
        .stdout(predicate::str::contains("dev shortcut ok"));
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_prefers_a_script_over_a_same_named_file() {
    let proj = load_tmp("script-wins");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "dev.mjs": "node ./script.cjs" } }"#,
    )
    .expect("write package.json");
    std::fs::write(proj.join("script.cjs"), r#"console.log("script wins")"#)
        .expect("write script target");
    std::fs::write(
        proj.join("dev.mjs"),
        r#"console.log("file should not run")"#,
    )
    .expect("write file target");

    meow()
        .current_dir(&proj)
        .arg("run")
        .arg("dev.mjs")
        .assert()
        .success()
        .stdout(predicate::str::contains("script wins"))
        .stdout(predicate::str::contains("file should not run").not());
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_executes_pre_and_post_hooks_in_order() {
    let proj = load_tmp("hooks");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "predev": "node ./step.cjs pre", "dev": "node ./step.cjs main", "postdev": "node ./step.cjs post" } }"#,
    )
    .expect("write package.json");
    std::fs::write(
        proj.join("step.cjs"),
        r#"const fs = require("node:fs");
let prev = "";
try { prev = fs.readFileSync("order.txt", "utf8"); } catch {}
fs.writeFileSync("order.txt", `${prev}${process.argv[2]}\n`);"#,
    )
    .expect("write hook script");

    meow()
        .current_dir(&proj)
        .arg("run")
        .arg("dev")
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(proj.join("order.txt")).expect("read order"),
        "pre\nmain\npost\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_stops_after_a_failing_pre_hook() {
    let proj = load_tmp("pre-fail");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "predev": "node ./step.cjs pre 17", "dev": "node ./step.cjs main", "postdev": "node ./step.cjs post" } }"#,
    )
    .expect("write package.json");
    std::fs::write(
        proj.join("step.cjs"),
        r#"const fs = require("node:fs");
let prev = "";
try { prev = fs.readFileSync("order.txt", "utf8"); } catch {}
fs.writeFileSync("order.txt", `${prev}${process.argv[2]}\n`);
process.exit(Number(process.argv[3] || 0));"#,
    )
    .expect("write hook script");

    meow()
        .current_dir(&proj)
        .arg("run")
        .arg("dev")
        .assert()
        .code(17);
    assert_eq!(
        std::fs::read_to_string(proj.join("order.txt")).expect("read order"),
        "pre\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_returns_the_post_hook_exit_code() {
    let proj = load_tmp("post-fail");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "predev": "node ./step.cjs pre", "dev": "node ./step.cjs main", "postdev": "node ./step.cjs post 23" } }"#,
    )
    .expect("write package.json");
    std::fs::write(
        proj.join("step.cjs"),
        r#"const fs = require("node:fs");
let prev = "";
try { prev = fs.readFileSync("order.txt", "utf8"); } catch {}
fs.writeFileSync("order.txt", `${prev}${process.argv[2]}\n`);
process.exit(Number(process.argv[3] || 0));"#,
    )
    .expect("write hook script");

    meow()
        .current_dir(&proj)
        .arg("run")
        .arg("dev")
        .assert()
        .code(23);
    assert_eq!(
        std::fs::read_to_string(proj.join("order.txt")).expect("read order"),
        "pre\nmain\npost\n"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[cfg(unix)]
#[test]
fn run_falls_back_to_the_shell_for_compound_scripts() {
    let proj = load_tmp("shell");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "say": "echo hi && echo ok" } }"#,
    )
    .expect("write package.json");

    meow()
        .current_dir(&proj)
        .arg("run")
        .arg("say")
        .assert()
        .success()
        .stdout(predicate::str::contains("hi"))
        .stdout(predicate::str::contains("ok"));
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_missing_script_and_file_is_an_honest_error() {
    let proj = load_tmp("missing");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "scripts": { "dev": "node ./dev.cjs" } }"#,
    )
    .expect("write package.json");
    std::fs::write(proj.join("dev.cjs"), r#"console.log("ok")"#).expect("write dev script");

    meow()
        .current_dir(&proj)
        .arg("run")
        .arg("missing")
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot find missing"));
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_executes_a_cached_commonjs_package_bin_without_node_modules() {
    use meow_pkg::{
        Cache, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
    };
    use std::collections::BTreeMap;

    let proj = load_tmp("bin");
    let home = proj.join("home");
    std::fs::create_dir_all(&home).expect("home dir");

    let hash = Cache::in_home(&home)
        .store(&npm_tarball(&[
            (
                "package.json",
                "{\"name\":\"toolkit\",\"version\":\"1.0.0\",\"type\":\"commonjs\",\"bin\":{\"tool\":\"bin/tool.cjs\"}}",
            ),
            (
                "bin/tool.cjs",
                "const msg = require(\"../lib.cjs\");\nconsole.log(`BIN=${msg}`);\nconsole.log(`ARGV=${JSON.stringify(process.argv.slice(2))}`);\nconsole.log(`FILE=${__filename}`);\n",
            ),
            ("lib.cjs", "module.exports = \"from-bin\";\n"),
        ]))
        .expect("store toolkit blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(LockEntry {
        name: PackageName::new("toolkit"),
        version: Version::parse("1.0.0").expect("version"),
        integrity: hash,
        dependencies: BTreeMap::new(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: VersionReq::parse(">=0.0.0").expect("req"),
    });
    lockfile
        .write_canonical(&proj.join("meow.lock.jsonl"))
        .expect("write lockfile");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "devDependencies": { "toolkit": "^1.0.0" }, "scripts": { "dev": "tool --from-script" } }"#,
    )
    .expect("write package.json");

    meow()
        .env("HOME", &home)
        .current_dir(&proj)
        .arg("run")
        .arg("dev")
        .arg("--")
        .arg("from-cli")
        .assert()
        .success()
        .stdout(predicate::str::contains("BIN=from-bin"))
        .stdout(predicate::str::contains(
            r#"ARGV=["--from-script","from-cli"]"#,
        ))
        .stdout(predicate::str::contains("FILE=").and(predicate::str::contains("unpacked")));
    assert!(!proj.join("node_modules").exists(), "no node_modules (I-5)");
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_strips_and_runs_typescript_entry() {
    // P0 exit: `meow run app.ts` strips the types and runs end-to-end (LOAD-001
    // replaces RT-001's honest `.ts` refusal). A type annotation is erased; the
    // runtime code executes.
    let tmp = load_tmp("ts");
    let entry = tmp.join("hello.ts");
    std::fs::write(&entry, "const x: number = 1;\nconsole.log(\"ts ok\");\n").expect("write");
    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .success()
        .stdout(predicate::str::contains("ts ok"));
    assert!(!tmp.join("node_modules").exists(), "no node_modules (I-5)");
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_imports_a_cached_dev_dependency_from_stock_package_json() {
    // A stock package.json project's devDependencies participate in direct root
    // resolution; no meow.config.json is required.
    use meow_pkg::{
        Cache, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
    };
    use std::collections::BTreeMap;

    let proj = load_tmp("dep");
    let home = proj.join("home");
    std::fs::create_dir_all(&home).expect("home dir");

    // Populate the cache (rooted at the host home) with the dependency's bytes.
    let hash = Cache::in_home(&home)
        .store(&npm_tarball(&[
            (
                "package.json",
                "{\"name\":\"dep\",\"version\":\"1.0.0\",\"type\":\"module\",\"exports\":\"./index.js\"}",
            ),
            ("index.js", "export const greet = () => \"from cache\";\n"),
        ]))
        .expect("store dep blob");

    // A canonical meow.lock.jsonl mapping `dep` → that content hash.
    let mut lockfile = Lockfile::new();
    lockfile.upsert(LockEntry {
        name: PackageName::new("dep"),
        version: Version::parse("1.0.0").expect("version"),
        integrity: hash,
        dependencies: BTreeMap::new(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: VersionReq::parse(">=0.0.0").expect("req"),
    });
    lockfile
        .write_canonical(&proj.join("meow.lock.jsonl"))
        .expect("write lockfile");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "devDependencies": { "dep": "^1.0.0" } }"#,
    )
    .expect("write package.json");

    let entry = proj.join("main.ts");
    std::fs::write(
        &entry,
        "import { greet } from \"dep\";\nconst msg: string = greet();\nconsole.log(msg);\n",
    )
    .expect("write entry");

    meow()
        .env("HOME", &home)
        .arg("run")
        .arg(&entry)
        .assert()
        .success()
        .stdout(predicate::str::contains("from cache"));
    assert!(!proj.join("node_modules").exists(), "no node_modules (I-5)");
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_rejects_non_erasable_typescript_with_an_honest_diagnostic() {
    // `enum` emits runtime code → cannot be type-stripped. The run fails honestly
    // (non-zero + the GRAPH diagnostic), never fabricates a success.
    let tmp = load_tmp("enum");
    let entry = tmp.join("bad.ts");
    std::fs::write(&entry, "enum E { A }\nconsole.log(\"should not run\");\n").expect("write");
    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .failure()
        .stdout(predicate::str::contains("should not run").not())
        .stderr(predicate::str::contains("Enums emit runtime code"))
        .stderr(predicate::str::contains("panicked").not());
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_finds_root_lockfile_from_a_nested_entry() {
    // Finding 1: the lockfile lives at the project ROOT and the entry is nested at
    // `src/main.ts`. `meow run src/main.ts` must climb to the root lockfile + root
    // package.json (not look for `src/meow.lock.jsonl`) so the pinned bare import resolves.
    use meow_pkg::{
        Cache, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
    };
    use std::collections::BTreeMap;

    let proj = load_tmp("rootlock");
    let home = proj.join("home");
    std::fs::create_dir_all(&home).expect("home dir");

    let hash = Cache::in_home(&home)
        .store(&npm_tarball(&[
            (
                "package.json",
                "{\"name\":\"dep\",\"version\":\"1.0.0\",\"type\":\"module\",\"exports\":\"./index.js\"}",
            ),
            ("index.js", "export const greet = () => \"from root lock\";\n"),
        ]))
        .expect("store dep blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(LockEntry {
        name: PackageName::new("dep"),
        version: Version::parse("1.0.0").expect("version"),
        integrity: hash,
        dependencies: BTreeMap::new(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: VersionReq::parse(">=0.0.0").expect("req"),
    });
    lockfile
        .write_canonical(&proj.join("meow.lock.jsonl"))
        .expect("write lockfile");
    std::fs::write(
        proj.join("package.json"),
        br#"{ "dependencies": { "dep": "^1.0.0" } }"#,
    )
    .expect("write package.json");

    let src = proj.join("src");
    std::fs::create_dir_all(&src).expect("src dir");
    let entry = src.join("main.ts");
    std::fs::write(
        &entry,
        "import { greet } from \"dep\";\nconsole.log(greet());\n",
    )
    .expect("write entry");

    meow()
        .env("HOME", &home)
        .arg("run")
        .arg(&entry)
        .assert()
        .success()
        .stdout(predicate::str::contains("from root lock"));
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn run_rejects_a_lockfile_with_duplicate_package_names() {
    // Finding 2: two entries share a package name. The name-keyed bare map cannot
    // represent that until LOAD-003, so `meow run` must fail honestly (non-zero +
    // the ambiguity diagnostic), never silently keep one version.
    use meow_pkg::{
        ContentHash, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
    };
    use std::collections::BTreeMap;

    let proj = load_tmp("dup");
    let entry_line = |version: &str, payload: &[u8]| LockEntry {
        name: PackageName::new("dep"),
        version: Version::parse(version).expect("version"),
        integrity: ContentHash::of(payload),
        dependencies: BTreeMap::new(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: VersionReq::parse(">=0.0.0").expect("req"),
    };
    let mut lockfile = Lockfile::new();
    lockfile.upsert(entry_line("1.0.0", b"v1"));
    lockfile.upsert(entry_line("2.0.0", b"v2"));
    lockfile
        .write_canonical(&proj.join("meow.lock.jsonl"))
        .expect("write lockfile");

    let entry = proj.join("main.ts");
    std::fs::write(&entry, "console.log(\"unreachable\");\n").expect("write entry");

    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .failure()
        .stdout(predicate::str::contains("unreachable").not())
        .stderr(predicate::str::contains("multiple versions of `dep`"))
        .stderr(predicate::str::contains("1.0.0"))
        .stderr(predicate::str::contains("2.0.0"))
        .stderr(predicate::str::contains("panicked").not());
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
#[ignore = "real network reality-check"]
fn install_real_registry_package_and_run_without_node_modules() {
    use meow_pkg::{resolve_roots, Lockfile, PackageName};

    let proj = load_tmp("real-install");
    let home = proj.join("home");
    std::fs::create_dir_all(&home).expect("home dir");

    let first = meow()
        .env("HOME", &home)
        .current_dir(&proj)
        .args(["install", "camelcase-keys@^10"])
        .output()
        .expect("run install");
    assert!(first.status.success(), "install failed: {first:?}");
    let first_stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        first_stdout.contains("installed "),
        "stdout was {first_stdout:?}"
    );
    assert!(
        !proj.join("node_modules").exists(),
        "install must not materialize node_modules"
    );

    let lock_path = proj.join("meow.lock.jsonl");
    let first_lock = std::fs::read(&lock_path).expect("read lockfile");
    let lockfile = Lockfile::read(&lock_path).expect("parse lockfile");
    assert!(
        lockfile
            .iter()
            .any(|entry| entry.name == PackageName::new("camelcase-keys")),
        "root package pinned"
    );
    assert!(
        lockfile
            .iter()
            .any(|entry| entry.name == PackageName::new("map-obj")),
        "transitive dependency pinned"
    );

    let package_json = meow_config::PackageJson::read(&proj).expect("read package.json");
    let direct = package_json
        .direct_dependencies()
        .expect("package.json direct deps");
    let roots = resolve_roots(&direct, &lockfile).expect("resolve roots");
    assert!(
        roots.contains_key(&PackageName::new("camelcase-keys")),
        "package.json + lockfile derive the runtime root"
    );
    assert_eq!(
        package_json
            .dependencies
            .get(&PackageName::new("camelcase-keys"))
            .map(String::as_str),
        Some("^10")
    );

    let entry = proj.join("main.ts");
    std::fs::write(
        &entry,
        "import camelcaseKeys from \"camelcase-keys\";\n\
         console.log(JSON.stringify(camelcaseKeys({\"foo-bar\": true})));\n",
    )
    .expect("write entry");

    meow()
        .env("HOME", &home)
        .current_dir(&proj)
        .arg("run")
        .arg(&entry)
        .assert()
        .success()
        .stdout(predicate::str::contains("{\"fooBar\":true}"));
    assert!(
        !proj.join("node_modules").exists(),
        "run must resolve from the cache, not node_modules"
    );

    std::fs::remove_file(&lock_path).expect("remove first lockfile");
    let second = meow()
        .env("HOME", &home)
        .current_dir(&proj)
        .arg("install")
        .output()
        .expect("run reinstall");
    assert!(second.status.success(), "reinstall failed: {second:?}");
    let second_lock = std::fs::read(&lock_path).expect("read second lockfile");
    assert_eq!(
        first_lock, second_lock,
        "lockfile bytes must be deterministic"
    );

    std::fs::remove_dir_all(&proj).ok();
}
// === /LOAD-001 ===

// === RT-006 ===
#[test]
fn run_is_intl_deterministic_across_tz_and_locale() {
    // craft-7 M3: under the default (virtual) clock, Date/Intl rendering must be
    // identical regardless of the host timezone + locale (pin_deterministic_intl
    // pins TZ=UTC + a fixed default locale before the isolate is created).
    let tmp = load_tmp("intl");
    let entry = tmp.join("intl.ts");
    std::fs::write(
        &entry,
        "console.log(new Date().toString() + \"|\" + \
         new Intl.DateTimeFormat().resolvedOptions().locale + \"|\" + \
         (1234.5).toLocaleString());\n",
    )
    .expect("write entry");

    let run = |tz: &str, lang: &str| -> String {
        let out = meow()
            .env("TZ", tz)
            .env("LANG", lang)
            .env("LC_ALL", lang)
            .arg("run")
            .arg(&entry)
            .output()
            .expect("run meow");
        assert!(out.status.success(), "run failed: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let ny_fr = run("America/New_York", "fr_FR.UTF-8");
    let tokyo_ja = run("Asia/Tokyo", "ja_JP.UTF-8");
    assert_eq!(
        ny_fr, tokyo_ja,
        "Date/Intl rendering must be deterministic across host TZ + locale"
    );
    assert!(
        ny_fr.contains("GMT+0000") && ny_fr.contains("en-US"),
        "default clock pins UTC + en-US, got: {ny_fr}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}
// === /RT-006 ===

// === OBS-001 ===
fn why_dep_entry(name: &str, version: &str, deps: &[(&str, &str)]) -> meow_pkg::LockEntry {
    meow_pkg::LockEntry {
        name: meow_pkg::PackageName::new(name),
        version: meow_pkg::Version::parse(version).expect("version"),
        integrity: meow_pkg::ContentHash::of(format!("{name}@{version}").as_bytes()),
        dependencies: deps
            .iter()
            .map(|(n, v)| {
                (
                    meow_pkg::PackageName::new(*n),
                    meow_pkg::Version::parse(v).expect("dep version"),
                )
            })
            .collect(),
        registry: meow_pkg::RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: meow_pkg::VersionReq::parse(">=0.0.0").expect("req"),
    }
}

/// Write a temp project: stock package.json (declared deps) + a canonical lockfile.
fn why_dep_project(
    tag: &str,
    direct_deps: &[(&str, &str)],
    entries: Vec<meow_pkg::LockEntry>,
) -> std::path::PathBuf {
    let proj = load_tmp(tag);
    let deps_json: Vec<String> = direct_deps
        .iter()
        .map(|(n, r)| format!("\"{n}\":\"{r}\""))
        .collect();
    std::fs::write(
        proj.join("package.json"),
        format!("{{\"dependencies\":{{{}}}}}", deps_json.join(",")),
    )
    .expect("write package.json");
    let mut lf = meow_pkg::Lockfile::new();
    for e in entries {
        lf.upsert(e);
    }
    lf.write_canonical(&proj.join("meow.lock.jsonl"))
        .expect("write lockfile");
    proj
}

#[test]
fn why_dep_traces_the_chain() {
    let proj = why_dep_project(
        "whydep",
        &[("a", "^1")],
        vec![
            why_dep_entry("a", "1.0.0", &[("target", "1.0.0")]),
            why_dep_entry("target", "1.0.0", &[]),
        ],
    );
    meow()
        .current_dir(&proj)
        .arg("why-dep")
        .arg("target")
        .assert()
        .success()
        .stdout(predicate::str::contains("a@1.0.0 → target@1.0.0"));
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn why_dep_not_found_exits_1_honestly() {
    let proj = why_dep_project(
        "whydepnf",
        &[("a", "^1")],
        vec![why_dep_entry("a", "1.0.0", &[])],
    );
    meow()
        .current_dir(&proj)
        .arg("why-dep")
        .arg("ghost")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not in the dependency tree"))
        .stderr(predicate::str::contains("panicked").not());
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn why_dep_malformed_lockfile_errors_without_panic() {
    let proj = load_tmp("whydepbad");
    std::fs::write(proj.join("package.json"), "{\"dependencies\":{}}").expect("package.json");
    std::fs::write(
        proj.join("meow.lock.jsonl"),
        "this is not canonical jsonl {{{\n",
    )
    .expect("lock");
    meow()
        .current_dir(&proj)
        .arg("why-dep")
        .arg("x")
        .assert()
        .failure()
        .stderr(predicate::str::contains("panicked").not());
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn why_dep_json_is_machine_readable() {
    let proj = why_dep_project(
        "whydepjson",
        &[("a", "^1")],
        vec![
            why_dep_entry("a", "1.0.0", &[("target", "1.0.0")]),
            why_dep_entry("target", "1.0.0", &[]),
        ],
    );
    let out = meow()
        .current_dir(&proj)
        .arg("why-dep")
        .arg("target")
        .arg("--json")
        .output()
        .expect("run");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json output");
    assert_eq!(v["found"], true);
    assert_eq!(v["versions"][0]["node"]["name"], "target");
    std::fs::remove_dir_all(&proj).ok();
}
// === /OBS-001 ===
