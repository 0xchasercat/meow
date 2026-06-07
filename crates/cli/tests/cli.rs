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
    (&["sync"], "sync", "P1"),
];

#[test]
fn every_subcommand_stub_is_honest() {
    assert_eq!(CASES.len(), 18, "all 18 subcommands covered");
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
