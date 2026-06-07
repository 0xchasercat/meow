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
    (&["dev", "x.ts"], "dev", "P1"),
    (&["install"], "install", "P2"),
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
    (&["why-dep", "p"], "why-dep", "P2"),
    (&["trace", "x.ts"], "trace", "P6"),
    (&["profile", "x.ts"], "profile", "P6"),
];

#[test]
fn every_subcommand_stub_is_honest() {
    assert_eq!(
        CASES.len(),
        15,
        "15 stub subcommands (sync + run real — CFG-001/RT-001; doctor real — CFG-002)"
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
fn run_refuses_first_party_cjs() {
    // First-party CommonJS is refused in every mode (I-2 / ADR-3) — never executed.
    let tmp = std::env::temp_dir().join(format!("meow-run-cjs-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let entry = tmp.join("app.cjs");
    std::fs::write(&entry, r#"console.log("nope")"#).expect("write entry");
    meow()
        .arg("run")
        .arg(&entry)
        .assert()
        .failure()
        .stdout(predicate::str::contains("nope").not())
        .stderr(predicate::str::contains("CommonJS"));
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_rejects_unwired_argv_instead_of_dropping_it() {
    // Forwarding program args (after `--`) is not wired yet — reject, never silently drop.
    let tmp = std::env::temp_dir().join(format!("meow-run-argv-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("temp dir");
    let entry = tmp.join("noop.mjs");
    std::fs::write(&entry, "").expect("write entry");
    meow()
        .arg("run")
        .arg(&entry)
        .arg("--")
        .arg("foo")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not yet supported"));
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
fn run_imports_a_cached_dependency_with_no_node_modules() {
    // A bare import resolves from the content-addressed cache (no node_modules, I-5).
    // The edge reads meow.lock.jsonl + the host home's ~/.meow/cache.
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
    // `src/main.ts`. `meow run src/main.ts` must climb to the root lockfile (not
    // look for `src/meow.lock.jsonl`) so the pinned bare import resolves.
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
// === /LOAD-001 ===
