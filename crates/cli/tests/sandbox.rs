//! Integration tests for the SEC-001 filesystem sandbox (`meow x` default /
//! `meow run --sandbox`).
//!
//! `meow x` is sandboxed by default; `meow run --sandbox` opts a normal run into
//! the SAME [`SandboxPolicy`]: reads anywhere, writes confined to the cwd /
//! workspace / OS-temp, network denied, subprocesses + native addons denied.
//! These tests drive `meow run --sandbox <local entry>` — a local file, so no
//! registry install or real network is needed — and assert the **filesystem**
//! half of the policy end-to-end, plus that `--trust` overrides it. (The network
//! half is enforced through meow's fetch gate / the Node permission container;
//! it's exercised manually rather than here, since a hermetic net assertion needs
//! a loopback server and the fetch gate intentionally bypasses IP literals.)
//!
//! The "outside" write target lives under `$HOME` — guaranteed to be outside
//! every sandbox root — so "denied" vs "allowed" is unambiguous and message
//! independent (we assert on the write succeeding or throwing, not on wording).

use std::path::PathBuf;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

/// Generous per-test ceiling: startup + a V8 isolate is well under this, but a
/// hang (e.g. a sandbox that deadlocks bootstrap) fails fast instead of stalling.
const TIMEOUT: Duration = Duration::from_secs(120);

fn meow() -> Command {
    Command::cargo_bin("meow").expect("meow binary builds")
}

fn unique(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("meow-sandbox-it-{tag}-{}-{n}", std::process::id())
}

/// A fresh temp project dir. No `meow.config.json`, so it runs in `node-compat`
/// (where `node:fs` exists). Canonicalized to match how the sandbox canonicalizes
/// its write roots.
fn tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique(tag));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::canonicalize(dir).expect("canonicalize temp dir")
}

/// A directory guaranteed to sit OUTSIDE every sandbox root (the project dir, the
/// cwd, and the OS temp dir): a fresh dir under `$HOME`. Created now so the write
/// target's parent exists; removed on drop.
struct OutsideDir {
    path: PathBuf,
}

impl OutsideDir {
    fn new(tag: &str) -> OutsideDir {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .expect("HOME or USERPROFILE is set");
        let path = home.join(unique(tag));
        std::fs::create_dir_all(&path).expect("outside dir");
        OutsideDir { path }
    }

    fn out_file(&self) -> PathBuf {
        self.path.join("out.txt")
    }

    /// The would-be output file as a forward-slashed string safe to embed in a JS
    /// double-quoted literal (forward slashes work in Node's `fs` on every
    /// platform, and generated temp names never contain quotes).
    fn js_file(&self) -> String {
        self.out_file().to_string_lossy().replace('\\', "/")
    }
}

impl Drop for OutsideDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Run `meow run <flags> main.mjs` in a fresh temp project and return the finished
/// assert. The project dir is removed once the process has exited.
fn run(tag: &str, source: &str, flags: &[&str]) -> assert_cmd::assert::Assert {
    let dir = tmp(tag);
    std::fs::write(dir.join("main.mjs"), source).expect("write entry");
    let mut cmd = meow();
    cmd.current_dir(&dir).arg("run");
    for flag in flags {
        cmd.arg(flag);
    }
    let assert = cmd.arg("main.mjs").timeout(TIMEOUT).assert();
    let _ = std::fs::remove_dir_all(&dir);
    assert
}

/// Entry that writes one file inside the project (a sandbox root) and one at
/// `outside_file` (never a root), printing an unambiguous marker for each. Uses
/// `try/catch` so a denial is a caught throw, not a process crash — the run exits
/// 0 either way and the markers say what happened.
fn write_probe(outside_file: &str) -> String {
    format!(
        r#"import {{ writeFileSync }} from "node:fs";
try {{ writeFileSync("./inside.txt", "x"); console.log("CWD_OK"); }}
catch {{ console.log("CWD_DENIED"); }}
try {{ writeFileSync("{outside_file}", "x"); console.log("OUT_ALLOWED"); }}
catch {{ console.log("OUT_DENIED"); }}
"#
    )
}

#[test]
fn sandbox_confines_writes_to_the_project() {
    // `--sandbox`: a write inside the project succeeds; a write outside every root
    // (here under $HOME) is denied, and the file is never created.
    let outside = OutsideDir::new("confine");
    run("confine", &write_probe(&outside.js_file()), &["--sandbox"])
        .success()
        .stdout(predicate::str::contains("CWD_OK"))
        .stdout(predicate::str::contains("OUT_DENIED"));
    assert!(
        !outside.out_file().exists(),
        "the sandbox let a write escape to {}",
        outside.out_file().display()
    );
}

#[test]
fn trusted_run_is_not_confined() {
    // Default `meow run` (no `--sandbox`) is trusted: the same outside write
    // succeeds, proving the default path stays unrestricted (allow-all).
    let outside = OutsideDir::new("trusted");
    run("trusted", &write_probe(&outside.js_file()), &[])
        .success()
        .stdout(predicate::str::contains("CWD_OK"))
        .stdout(predicate::str::contains("OUT_ALLOWED"));
    assert!(
        outside.out_file().exists(),
        "trusted run should have written {}",
        outside.out_file().display()
    );
}

#[test]
fn trust_overrides_sandbox() {
    // `--trust` wins over `--sandbox`: full host access, so the outside write is
    // allowed even though `--sandbox` was also passed.
    let outside = OutsideDir::new("override");
    run(
        "override",
        &write_probe(&outside.js_file()),
        &["--sandbox", "--trust"],
    )
    .success()
    .stdout(predicate::str::contains("OUT_ALLOWED"));
    assert!(outside.out_file().exists());
}
