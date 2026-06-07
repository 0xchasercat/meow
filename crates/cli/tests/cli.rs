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
    (&["run", "x.ts"], "run", "P1"),
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
    (&["doctor"], "doctor", "P6"),
];

#[test]
fn every_subcommand_stub_is_honest() {
    assert_eq!(
        CASES.len(),
        17,
        "all 17 stub subcommands covered (sync is real — CFG-001)"
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
