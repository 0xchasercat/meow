//! Integration tests for `node:worker_threads` (WORKER-001).
//!
//! These exercise the real `meow` binary end-to-end, because workers need the
//! full stack: the snapshot-baked ops, the thread-per-worker spawner, and the
//! polyfill. Each test drives a small ESM entry through `meow run` and asserts on
//! stdout. Every test has a timeout so a regression fails fast instead of
//! hanging CI (workers deadlocking was the original failure mode).
//!
//! Coverage: eval workers, cross-isolate `SharedArrayBuffer` + `Atomics`,
//! transferable `MessagePort` + `receiveMessageOnPort`, the `env` worker option,
//! `unref()` letting the process exit, and nested workers — the pieces
//! miniflare's synchronous fetch (SvelteKit's Cloudflare adapter) relies on.

use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

/// Generous per-test ceiling: worker startup + a V8 isolate is well under this,
/// but a deadlock (the bug class these tests guard) is caught rather than hung.
const TIMEOUT: Duration = Duration::from_secs(120);

fn meow() -> Command {
    Command::cargo_bin("meow").expect("meow binary builds")
}

fn tmp(tag: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-worker-it-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::canonicalize(dir).expect("canonicalize temp dir")
}

/// Write `main.mjs` in a fresh temp dir, run it through `meow run`, and assert it
/// succeeds (within the timeout) and prints `expect`.
fn run_worker_entry(tag: &str, source: &str, expect: &str) {
    let dir = tmp(tag);
    let entry = dir.join("main.mjs");
    std::fs::write(&entry, source).expect("write worker entry");
    meow()
        .current_dir(&dir)
        .arg("run")
        .arg(&entry)
        .timeout(TIMEOUT)
        .assert()
        .success()
        .stdout(predicate::str::contains(expect));
    let _ = std::fs::remove_dir_all(&dir);
    // Best-effort cleanup of any eval temp modules this run staged.
    cleanup_eval_temp_modules();
}

/// eval workers stage a temp `.cjs`; drop any this process's runs left behind.
fn cleanup_eval_temp_modules() {
    let tmp_dir = std::env::temp_dir();
    if let Ok(entries) = std::fs::read_dir(&tmp_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("meow-worker-eval-") && name.ends_with(".cjs") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[test]
fn eval_worker_round_trips_worker_data() {
    // `new Worker(code, { eval: true })` runs inline CommonJS source; workerData
    // crosses via structured clone and the result comes back over parentPort.
    run_worker_entry(
        "eval",
        r#"import { Worker } from "node:worker_threads";
const w = new Worker(
  "const { parentPort, workerData } = require('node:worker_threads'); parentPort.postMessage(workerData * 2);",
  { eval: true, workerData: 21 },
);
w.on("message", (m) => { console.log("RESULT:" + m); w.terminate(); });
"#,
        "RESULT:42",
    );
}

#[test]
fn shared_array_buffer_atomics_cross_thread() {
    // A SharedArrayBuffer in workerData must share one backing store across
    // isolates so Atomics.wait (main) / Atomics.notify (worker) work cross-thread.
    run_worker_entry(
        "sab",
        r#"import { Worker } from "node:worker_threads";
const sab = new Int32Array(new SharedArrayBuffer(4));
const w = new Worker(
  "const { workerData } = require('node:worker_threads'); Atomics.store(workerData, 0, 42); Atomics.notify(workerData, 0);",
  { eval: true, workerData: sab },
);
Atomics.wait(sab, 0, 0);
console.log("SAB:" + sab[0]);
w.terminate();
"#,
        "SAB:42",
    );
}

#[test]
fn transferable_message_port_and_receive_message_on_port() {
    // Transfer a MessagePort into the worker, round-trip a message, and read the
    // reply synchronously via receiveMessageOnPort after Atomics.wait wakes us --
    // exactly miniflare's synchronous-fetch handshake.
    run_worker_entry(
        "port",
        r#"import { Worker, MessageChannel, receiveMessageOnPort } from "node:worker_threads";
const { port1, port2 } = new MessageChannel();
const sab = new Int32Array(new SharedArrayBuffer(4));
const w = new Worker(
  `const { workerData } = require('node:worker_threads');
   const { port, sab } = workerData;
   port.on('message', (msg) => {
     port.postMessage('echo:' + msg);
     Atomics.store(sab, 0, 1); Atomics.notify(sab, 0);
   });
   port.start();`,
  { eval: true, workerData: { port: port2, sab }, transferList: [port2] },
);
port1.postMessage("ping");
Atomics.wait(sab, 0, 0);
const received = receiveMessageOnPort(port1);
console.log("PORT:" + (received ? received.message : "none"));
w.terminate();
"#,
        "PORT:echo:ping",
    );
}

#[test]
fn env_worker_option_sets_process_env() {
    // Node's `env` worker option sets the worker's process.env (tools signal fork
    // mode this way, e.g. SvelteKit's SVELTEKIT_FORK).
    run_worker_entry(
        "env",
        r#"import { Worker } from "node:worker_threads";
const w = new Worker(
  "const { parentPort } = require('node:worker_threads'); parentPort.postMessage('ENV:' + process.env.MEOW_TEST_VAR);",
  { eval: true, env: { ...process.env, MEOW_TEST_VAR: "meow123" } },
);
w.on("message", (m) => { console.log(m); w.terminate(); });
"#,
        "ENV:meow123",
    );
}

#[test]
fn unref_lets_the_process_exit_with_a_live_worker() {
    // An unref'd worker must not keep the parent event loop alive: the parent
    // prints and exits even though the worker stays listening forever. If unref
    // is broken this hangs and the timeout fails the test.
    run_worker_entry(
        "unref",
        r#"import { Worker } from "node:worker_threads";
const w = new Worker(
  "const { parentPort } = require('node:worker_threads'); parentPort.on('message', () => {}); parentPort.postMessage('ready');",
  { eval: true },
);
w.on("message", () => { w.unref(); console.log("UNREF_EXIT_OK"); });
"#,
        "UNREF_EXIT_OK",
    );
}

#[test]
fn nested_worker_spawns_from_a_worker() {
    // A worker can spawn its own worker (each on its own OS thread) -- SvelteKit's
    // prerender worker spins up miniflare's fetcher this way.
    run_worker_entry(
        "nested",
        r#"import { Worker } from "node:worker_threads";
const outer = new Worker(
  `const { parentPort, Worker } = require('node:worker_threads');
   const inner = new Worker(
     "const { parentPort } = require('node:worker_threads'); parentPort.postMessage('from-inner');",
     { eval: true }
   );
   inner.on('message', (m) => { parentPort.postMessage('outer-got:' + m); inner.terminate(); });`,
  { eval: true },
);
outer.on("message", (m) => { console.log("NESTED:" + m); outer.terminate(); });
"#,
        "NESTED:outer-got:from-inner",
    );
}
