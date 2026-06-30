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
// No stub subcommands remain — all are implemented.
// If a new stub is added, add it to this list and update the count.
const CASES: &[(&[&str], &str)] = &[];
#[test]
fn every_subcommand_stub_is_honest() {
    assert_eq!(CASES.len(), 0, "all subcommands are implemented");
    for (argv, verb) in CASES {
        let expected = format!("meow: `{verb}` is not yet implemented");
        meow()
            .args(*argv)
            .assert()
            .code(3)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(expected));
    }
}

#[test]
fn lint_reports_debugger_diagnostics() {
    let tmp = load_tmp("lint-debugger");
    let entry = tmp.join("entry.ts");
    std::fs::write(&entry, "debugger;\n").expect("write linter fixture");
    let out = meow().arg("lint").arg(&entry).output().expect("run lint");
    assert!(
        !out.status.success(),
        "lint should fail on debugger statement: {out:?}"
    );
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    let file_name = entry.file_name().expect("entry file name");
    assert!(
        stderr.contains('^'),
        "lint should render a caret diagnostic: {stderr:?}"
    );
    assert!(
        stderr.contains(entry.to_string_lossy().as_ref())
            || stderr.contains(file_name.to_string_lossy().as_ref()),
        "lint diagnostic should include the file path: {stderr:?}"
    );
    assert!(
        stderr.contains("debugger"),
        "lint diagnostic should include the offending token: {stderr:?}"
    );
    assert!(
        stderr.contains("1:") || stderr.contains("line 1"),
        "lint diagnostic should include a line snippet: {stderr:?}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_check_fails_without_reformatting() {
    let tmp = load_tmp("fmt-check");
    let entry = tmp.join("entry.ts");
    let source = "function  foo ( ) { return  1+2 }\n";
    std::fs::write(&entry, source).expect("write formatter fixture");
    let before = std::fs::read_to_string(&entry).expect("read before formatting");

    let out = meow()
        .arg("fmt")
        .arg("--check")
        .arg(&entry)
        .output()
        .expect("run fmt --check");
    assert!(
        !out.status.success(),
        "fmt --check should fail on unformatted input: {out:?}"
    );
    let after = std::fs::read_to_string(&entry).expect("read after check");
    assert_eq!(
        before, after,
        "fmt --check should be read-only and must not rewrite {entry:?}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn fmt_rewrites_and_then_check_succeeds() {
    let tmp = load_tmp("fmt-rewrite");
    let entry = tmp.join("entry.ts");
    let source = "function  foo ( ) { return  1+2 }\n";
    std::fs::write(&entry, source).expect("write formatter fixture");

    let out = meow().arg("fmt").arg(&entry).output().expect("run fmt");
    assert!(
        out.status.success(),
        "fmt should succeed and rewrite file: {out:?}"
    );
    let formatted = std::fs::read_to_string(&entry).expect("read formatted output");
    assert_ne!(
        formatted, source,
        "fmt should rewrite source from unformatted to canonical form"
    );

    let check = meow()
        .arg("fmt")
        .arg("--check")
        .arg(&entry)
        .output()
        .expect("run fmt --check");
    assert!(
        check.status.success(),
        "fmt --check should succeed after format: {check:?}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn bundle_entry_is_skeleton_with_pending_wiring_message() {
    let tmp = load_tmp("bundle");
    let entry = tmp.join("entry.ts");
    let dist = tmp.join("dist");
    std::fs::write(&entry, "const value = 1; console.log('bundle ok');\n")
        .expect("write bundle entry");
    let out = meow()
        .current_dir(&tmp)
        .arg("bundle")
        .arg(&entry)
        .arg("--out")
        .arg(&dist)
        .output()
        .expect("run bundle");
    assert!(out.status.success(), "bundle should succeed: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stdout_lc = stdout.to_lowercase();
    assert!(
        stdout_lc.contains("emitted 1 chunk"),
        "bundle success should report emitted chunks: {stdout:?}"
    );
    assert!(
        stdout_lc.contains("dist"),
        "bundle output should name output directory: {stdout:?}"
    );
    assert!(
        dist.join("entry.js").is_file(),
        "bundle should write dist/entry.js"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
#[cfg(unix)]
fn run_preserves_native_tty_raw_mode_under_pseudo_terminal() {
    let script_bin = std::path::Path::new("/usr/bin/script");
    if !script_bin.exists() {
        return;
    }
    let tmp = load_tmp("tty-raw-mode");
    let entry = tmp.join("probe.js");
    std::fs::write(
        &entry,
        r#"console.log([
  process.stdin.isTTY,
  process.stdout.isTTY,
  process.stdin.constructor && process.stdin.constructor.name,
  String(process.stdin.setRawMode).includes("_handle.setRawMode"),
  String(process.stdin.setRawMode).includes("io.stdin.setRaw"),
].join(":"));
process.stdin.setRawMode(true);
console.log("raw:" + process.stdin.isRaw);
process.stdin.setRawMode(false);
"#,
    )
    .expect("write tty probe");
    let transcript = tmp.join("typescript");
    let meow_bin = assert_cmd::cargo::cargo_bin("meow");
    // macOS (BSD script): script -q transcript cmd args...
    // Linux (util-linux): script -qc "cmd args..." transcript
    let mut cmd = StdCommand::new(script_bin);
    if cfg!(target_os = "macos") {
        cmd.arg("-q")
            .arg(&transcript)
            .arg(&meow_bin)
            .arg("run")
            .arg(&entry);
    } else {
        let full_cmd = format!("{} run {}", meow_bin.display(), entry.display());
        cmd.arg("-q").arg("-c").arg(&full_cmd).arg(&transcript);
    }
    let status = cmd.status().expect("run script pseudo-terminal");
    assert!(status.success(), "pseudo-terminal run should succeed");
    let output = std::fs::read_to_string(&transcript).expect("read transcript");
    assert!(
        output.contains("true:true:ReadStream:true:false"),
        "stdin must keep native node:tty ReadStream raw-mode method: {output:?}"
    );
    assert!(
        output.contains("raw:true"),
        "native raw-mode transition should report isRaw=true: {output:?}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn install_success_uses_purr_envelope_in_plain_output() {
    let proj = load_tmp("install-ok");
    std::fs::write(proj.join("package.json"), "{}\n").expect("package.json");
    let out = meow()
        .current_dir(&proj)
        .arg("install")
        .output()
        .expect("run install");
    assert!(out.status.success(), "install should succeed: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(
        stdout.contains("0 packages ready"),
        "install output should report the package count: {stdout:?}"
    );
    assert!(
        stdout.contains("meow.lock.jsonl"),
        "install output should name the lockfile: {stdout:?}"
    );
    assert!(
        stdout.contains("materialized"),
        "install should report the materialized projection: {stdout:?}"
    );
    assert!(
        !stdout.contains("virtual"),
        "install output should not frame materialized install as virtual: {stdout:?}"
    );
    assert!(
        stderr.is_empty(),
        "install success should keep stderr empty: {stderr:?}"
    );
    assert!(proj.join("meow.lock.jsonl").is_file(), "lockfile written");
    assert!(
        proj.join("node_modules").exists(),
        "default install should create node_modules"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn install_parse_error_uses_hiss_envelope_in_plain_output() {
    let proj = load_tmp("install-bad");
    std::fs::write(proj.join("package.json"), "{\n").expect("package.json");
    let out = meow()
        .current_dir(&proj)
        .arg("install")
        .output()
        .expect("run install");
    assert!(
        !out.status.success(),
        "install should fail on invalid package.json"
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(
        stdout.is_empty(),
        "install errors stay off stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("meow install:"),
        "install error should use the hiss envelope: {stderr:?}"
    );
    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn sync_generates_shadow_configs() {
    // `meow sync` is real (CFG-001): regenerates the shadow tsconfig + root shim.
    let tmp = load_tmp("sync");
    std::fs::write(tmp.join("meow.config.json"), "{}").expect("write config");
    let out = meow()
        .current_dir(&tmp)
        .arg("sync")
        .output()
        .expect("run sync");
    assert!(out.status.success(), "sync should succeed: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(
        stdout.contains("meow sync: regenerated .meow/tsconfig.json"),
        "sync success should use the purr envelope: {stdout:?}"
    );
    assert!(
        stderr.is_empty(),
        "sync success keeps stderr empty: {stderr:?}"
    );
    assert!(
        tmp.join(".meow/tsconfig.json").is_file(),
        "shadow .meow/tsconfig.json generated"
    );
    assert!(
        tmp.join("tsconfig.json").is_file(),
        "root tsconfig.json shim generated"
    );
    assert!(
        tmp.join(".meow/types/meow/ui.d.ts").is_file(),
        "sync writes the meow:ui declaration"
    );
    assert!(
        !tmp.join("package.json").exists(),
        "sync must not generate or overwrite package.json"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn sync_without_config_uses_default_config() {
    let tmp = load_tmp("sync-default");
    let out = meow()
        .current_dir(&tmp)
        .arg("sync")
        .output()
        .expect("run sync");
    assert!(out.status.success(), "sync without config should succeed");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(
        stdout.contains("meow sync: regenerated .meow/tsconfig.json"),
        "sync success should use the purr envelope: {stdout:?}"
    );
    assert!(
        stderr.is_empty(),
        "sync success keeps stderr empty: {stderr:?}"
    );
    assert!(
        tmp.join(".meow/tsconfig.json").is_file(),
        "shadow .meow/tsconfig.json generated"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn sync_invalid_config_uses_hiss_envelope_in_plain_output() {
    let tmp = load_tmp("sync-invalid");
    std::fs::write(tmp.join("meow.config.json"), "{").expect("write config");
    let out = meow()
        .current_dir(&tmp)
        .arg("sync")
        .output()
        .expect("run sync");
    assert!(
        !out.status.success(),
        "sync with invalid config should fail"
    );
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(stdout.is_empty(), "sync errors stay off stdout: {stdout:?}");
    assert!(
        stderr.contains("meow sync:"),
        "sync error should use the hiss envelope: {stderr:?}"
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
fn node_compat_run_uses_live_clock_and_os_entropy_by_default() {
    let tmp = load_tmp("run-live-hermetic-defaults");
    let entry = tmp.join("live.mjs");
    std::fs::write(
        &entry,
        r#"console.log(`now=${Date.now()}`);
console.log(`random=${Math.random()}`);
"#,
    )
    .expect("write live hermetic probe");

    let run_once = || {
        let out = meow()
            .current_dir(&tmp)
            .arg("run")
            .arg("--no-snapshot")
            .arg(&entry)
            .output()
            .expect("run live hermetic probe");
        assert!(out.status.success(), "meow run should succeed: {out:?}");
        String::from_utf8(out.stdout).expect("stdout utf8")
    };

    let first = run_once();
    let second = run_once();
    let now = first
        .lines()
        .find_map(|line| line.strip_prefix("now="))
        .and_then(|value| value.parse::<f64>().ok())
        .expect("now line");
    assert_ne!(
        now, 1_780_790_400_000.0,
        "node-compatible meow run must not inherit strict-web frozen time by default: {first:?}"
    );
    assert!(
        now > 1_735_689_600_000.0,
        "node-compatible meow run should expose a recent host clock: {first:?}"
    );
    let first_random = first
        .lines()
        .find_map(|line| line.strip_prefix("random="))
        .expect("first random line");
    let second_random = second
        .lines()
        .find_map(|line| line.strip_prefix("random="))
        .expect("second random line");
    assert_ne!(
        first_random, second_random,
        "node-compatible meow run should seed Math.random from OS entropy by default"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn strict_web_run_keeps_deterministic_clock_and_entropy_by_default() {
    let tmp = load_tmp("run-strict-hermetic-defaults");
    std::fs::write(tmp.join("meow.config.json"), br#"{ "mode": "strict-web" }"#)
        .expect("write strict-web config");
    let entry = tmp.join("strict.mjs");
    std::fs::write(
        &entry,
        r#"console.log(`now=${Date.now()}`);
console.log(`random=${Math.random()}`);
"#,
    )
    .expect("write strict hermetic probe");

    let run_once = || {
        let out = meow()
            .current_dir(&tmp)
            .arg("run")
            .arg("--no-snapshot")
            .arg(&entry)
            .output()
            .expect("run strict hermetic probe");
        assert!(
            out.status.success(),
            "strict-web meow run should succeed: {out:?}"
        );
        String::from_utf8(out.stdout).expect("stdout utf8")
    };

    let first = run_once();
    let second = run_once();
    assert!(
        first.contains("now=1780790400000"),
        "strict-web meow run should keep the frozen virtual epoch: {first:?}"
    );
    assert_eq!(
        first, second,
        "strict-web meow run should keep deterministic seeded entropy by default"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[test]
fn run_imports_meow_ui_and_keeps_console_output_raw() {
    let tmp = load_tmp("run-ui");
    let entry = tmp.join("ui.mjs");
    std::fs::write(
        &entry,
        r#"import { ui } from "meow:ui";
console.log("raw stdout");
ui.purr("hello");
console.error("raw stderr");
ui.hiss("boom");
"#,
    )
    .expect("write entry");
    let out = meow()
        .current_dir(&tmp)
        .arg("run")
        .arg(&entry)
        .output()
        .expect("run meow:ui entry");
    assert!(out.status.success(), "meow run should succeed: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(
        stdout.contains("raw stdout\n😸 hello\n"),
        "console.log stays raw while ui.purr is wrapped: {stdout:?}"
    );
    assert!(
        stderr.contains("raw stderr\n🙀 boom\n"),
        "console.error stays raw while ui.hiss is wrapped: {stderr:?}"
    );
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
    // No subcommand now prints the landing screen (exit 0). A malformed
    // invocation (missing a required arg) is still a clap usage error (exit 2),
    // never the "not built" exit 3.
    meow()
        .assert()
        .success()
        .stdout(predicate::str::contains("meow"));
    meow().arg("run").assert().code(2);
}

#[test]
#[ignore = "footprint.sh removed in public release cleanup"]
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
    std::fs::canonicalize(dir).expect("canonicalize temp dir")
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

    // Strip \\?\ UNC prefix on Windows so the assertion matches what
    // std::env::current_dir() returns inside the child process (without prefix).
    fn strip_unc_prefix(p: &std::path::Path) -> String {
        let s = p.to_string_lossy().into_owned();
        s.strip_prefix(r#"\\?\"#).map(String::from).unwrap_or(s)
    }
    let proj_display = strip_unc_prefix(&proj);
    let nested_display = strip_unc_prefix(&nested);
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
fn run_executes_a_cached_commonjs_package_bin_from_unpacked_store() {
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
                "{\"name\":\"toolkit\",\"version\":\"1.0.0\",\"type\":\"commonjs\",\"exports\":{\".\":\"./index.js\",\"./dist/compiled/helper\":\"./dist/compiled/helper.js\"},\"bin\":{\"tool\":\"bin/tool\"}}",
            ),
            ("index.js", "module.exports = {};\n"),
            (
                "bin/tool",
                "const Module = require(\"module\");\nconst childProcess = require(\"child_process\");\nconst crypto = require(\"crypto\");\nconst http = require(\"http\");\nconst https = require(\"https\");\nconst msg = require(\"toolkit/dist/compiled/helper\");\nconst child = process.platform === \"win32\" ? childProcess.execFileSync(\"cmd.exe\", [\"/D\", \"/S\", \"/C\", \"echo child-ok\"], { encoding: \"utf8\" }).trim() : childProcess.execFileSync(\"/bin/sh\", [\"-c\", \"printf child-ok\"], { encoding: \"utf8\" });\nconst hash = crypto.createHash(\"sha256\").update(\"abc\").digest(\"hex\");\nObject.defineProperty(exports, \"__esModule\", { value: true });\nconsole.log(`MODULE=${Module.isBuiltin(\"module\")}`);\nconsole.log(`CHILD=${child}`);\nconsole.log(`CRYPTO=${hash}`);\nconsole.log(`HTTP=${typeof http.createServer}:${typeof https.get}`);\nconsole.log(`BIN=${msg}`);\nconsole.log(`ARGV=${JSON.stringify(process.argv.slice(2))}`);\nconsole.log(`FILE=${__filename}`);\n",
            ),
            ("dist/compiled/helper.js", "module.exports = \"from-bin\";\n"),
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
        .stdout(predicate::str::contains("MODULE=true"))
        .stdout(predicate::str::contains("CHILD=child-ok"))
        .stdout(predicate::str::contains(
            "CRYPTO=ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ))
        .stdout(predicate::str::contains("HTTP=function:function"))
        .stdout(predicate::str::contains("BIN=from-bin"))
        .stdout(predicate::str::contains(
            r#"ARGV=["--from-script","from-cli"]"#,
        ))
        .stdout(predicate::str::contains("FILE=").and(predicate::str::contains("unpacked")));
    assert!(
        !proj.join("node_modules").exists(),
        "run must not synthesize an install projection"
    );
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
    assert!(
        !tmp.join("node_modules").exists(),
        "run must not synthesize an install projection"
    );
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
    assert!(
        !proj.join("node_modules").exists(),
        "run must not synthesize an install projection"
    );
    std::fs::remove_dir_all(&proj).ok();
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
fn install_real_registry_package_materializes_node_modules_by_default() {
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
        proj.join("node_modules").exists(),
        "install must materialize node_modules by default"
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
        proj.join("node_modules").exists(),
        "run should still succeed with the default materialized install layout"
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
    std::fs::write(tmp.join("meow.config.json"), br#"{ "mode": "strict-web" }"#)
        .expect("write strict-web config");
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
    let out = meow()
        .current_dir(&proj)
        .arg("why-dep")
        .arg("target")
        .output()
        .expect("run why-dep");
    assert!(out.status.success(), "why-dep should succeed: {out:?}");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("╭"),
        "human why-dep should render a bento box: {stdout:?}"
    );
    assert!(
        stdout.contains("a@1.0.0 → target@1.0.0"),
        "dependency chain stays precise inside the bento body: {stdout:?}"
    );
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
    assert!(
        out.stderr.is_empty(),
        "json mode should keep stderr empty: {out:?}"
    );
    let stdout = String::from_utf8(out.stdout.clone()).expect("stdout utf8");
    assert!(
        stdout.starts_with("{\n"),
        "json mode should stay raw: {stdout:?}"
    );
    assert!(
        !stdout.contains("[Purrfect!]") && !stdout.contains("╭"),
        "json mode must not be wrapped by the UI layer: {stdout:?}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json output");
    assert_eq!(v["found"], true);
    assert_eq!(v["versions"][0]["node"]["name"], "target");
    std::fs::remove_dir_all(&proj).ok();
}
// === /OBS-001 ===

// === INIT-001 ===
#[test]
fn init_creates_config_and_package_json() {
    let proj = load_tmp("init-ok");
    let _ = std::fs::remove_dir_all(&proj);
    std::fs::create_dir_all(&proj).expect("create project dir");

    meow()
        .current_dir(&proj)
        .arg("init")
        .arg("--no-install")
        .assert()
        .success()
        .stdout(predicate::str::contains("Project initialized!"));

    assert!(
        proj.join("meow.config.json").is_file(),
        "config should exist"
    );
    assert!(
        proj.join("package.json").is_file(),
        "package.json should exist"
    );
    assert!(proj.join("main.ts").is_file(), "main.ts should exist");
    assert!(proj.join(".meow").is_dir(), ".meow dir should exist");

    let config = std::fs::read_to_string(proj.join("meow.config.json")).expect("read config");
    assert!(
        config.contains("strict-web"),
        "default mode should be strict-web"
    );

    let pkg = std::fs::read_to_string(proj.join("package.json")).expect("read package");
    assert!(pkg.contains("0.1.0"), "version should be 0.1.0");
    assert!(pkg.contains("\"type\": \"module\""), "should be ESM");
    assert!(
        pkg.contains("meow run main.ts"),
        "dev script should use meow"
    );

    let main_ts = std::fs::read_to_string(proj.join("main.ts")).expect("read main.ts");
    assert!(
        main_ts.contains("meow:http"),
        "main.ts should import meow:http"
    );

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn init_node_compat_mode() {
    let proj = load_tmp("init-node-compat");
    let _ = std::fs::remove_dir_all(&proj);
    std::fs::create_dir_all(&proj).expect("create project dir");

    meow()
        .current_dir(&proj)
        .arg("init")
        .arg("--mode")
        .arg("node-compat")
        .arg("--no-install")
        .assert()
        .success();

    let config = std::fs::read_to_string(proj.join("meow.config.json")).expect("read config");
    assert!(config.contains("node-compat"), "mode should be node-compat");

    std::fs::remove_dir_all(&proj).ok();
}

#[test]
fn init_refuses_to_overwrite_without_force() {
    let proj = load_tmp("init-no-overwrite");
    let _ = std::fs::remove_dir_all(&proj);
    std::fs::create_dir_all(&proj).expect("create project dir");
    std::fs::write(proj.join("meow.config.json"), "{}").expect("seed config");

    meow()
        .current_dir(&proj)
        .arg("init")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    std::fs::remove_dir_all(&proj).ok();
}
// === /INIT-001 ===
