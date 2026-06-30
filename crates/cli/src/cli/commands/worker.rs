//! Cooperative-isolate `node:worker_threads` host edge (WORKER-001, meow-native).
//!
//! The worker OPS + manager now live in [`meow_runtime::worker`] so they bake
//! into the V8 startup snapshot — `core.ops.op_meow_worker_*` resolve from the
//! snapshot, whereas a runtime-only extension's ops do NOT. This module is the
//! BINARY edge: it owns runtime construction (resolver / Oxc module graph /
//! [`build_worker_runtime`]), which `meow_runtime` deliberately does not depend
//! on. It implements [`WorkerSpawner`] and drives each worker isolate as a
//! `tokio::task::spawn_local` task on the SAME OS thread as the main isolate (a
//! `LocalSet` is installed by the `run` command).
//!
//! Single-threaded by construction: this keeps meow's frozen-clock / seeded-RNG
//! determinism intact and lets the worker reuse the project's resolver + module
//! graph by `Rc`/`Arc` clone (no thread hop, no graph serialization). Only the
//! messages crossing the isolate boundary are structured-cloned
//! (`core.serialize`/`core.deserialize`).
//!
//! Worker isolate construction is DEFERRED to [`drive_worker`] (the spawner's
//! task), not done inside `op_meow_worker_create`, so the main isolate is no
//! longer entered when the worker isolate is built — V8 does not support
//! constructing / entering a second isolate from inside a running op on the
//! first one.

use meow_runtime::worker::{HostEvent, WorkerSideState, WorkerSpawnRequest, WorkerSpawner};
use meow_runtime::ModuleSpecifier;

use super::run::{build_worker_runtime, WorkerSpawnConfig};

/// Binary-edge spawner installed into the main runtime's `OpState` via
/// `meow_runtime::worker::worker_extension(Some(..))`. `op_meow_worker_create`
/// calls [`WorkerSpawner::spawn`] with everything needed to build + drive the
/// worker isolate.
pub(super) struct CliWorkerSpawner {
    pub(super) config: WorkerSpawnConfig,
}

impl WorkerSpawner for CliWorkerSpawner {
    fn spawn(&self, request: WorkerSpawnRequest) {
        tokio::task::spawn_local(drive_worker(self.config.clone(), request));
    }
}

async fn drive_worker(config: WorkerSpawnConfig, req: WorkerSpawnRequest) {
    if req.terminated.get() {
        return;
    }
    // Keep a sender clone for early-failure reporting before the worker side
    // takes ownership of the original.
    let outbox = req.outbox.clone();
    let Some(spec) = worker_module_specifier(&req.specifier) else {
        let _ = outbox.send(HostEvent::Error(format!(
            "worker_threads: invalid worker specifier `{}`",
            req.specifier
        )));
        return;
    };

    // Build the worker isolate now — we are on the LocalSet (the create op has
    // already returned), so the main isolate is no longer entered and a second
    // isolate can be constructed + bootstrapped safely.
    let worker_side = WorkerSideState::new(req.worker_data, req.inbox, req.outbox);
    let mut runtime = match build_worker_runtime(&config, &spec, worker_side) {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = outbox.send(HostEvent::Error(err));
            return;
        }
    };
    if req.terminated.get() {
        return;
    }
    *req.isolate.borrow_mut() = Some(runtime.isolate_handle());

    // Flip the freshly-bootstrapped (main-mode) isolate into worker mode: sets
    // isMainThread = false and wires parentPort + workerData. Runs at top level,
    // so it uses the `globalThis` hook the polyfill installed during bootstrap.
    let init = format!(
        "globalThis.__meowInitWorkerThread({}, {});",
        req.id,
        json_string(spec.as_str())
    );
    if let Err(err) = runtime.execute_script("meow:worker-init", init) {
        let _ = outbox.send(HostEvent::Error(err.to_string()));
        return;
    }

    // Drive the worker module to completion. It stays alive while parentPort has
    // a pending receive; `terminate()` stops it via
    // `IsolateHandle::terminate_execution`.
    if let Err(err) = runtime.run_main_module(&spec).await {
        if !req.terminated.get() {
            let _ = outbox.send(HostEvent::Error(err.to_string()));
        }
    }
    // `outbox` and `runtime` drop here: the host's pending recv resolves to
    // `None` -> an empty frame -> the `Worker` emits `exit`.
}

fn worker_module_specifier(raw: &str) -> Option<ModuleSpecifier> {
    if let Ok(spec) = ModuleSpecifier::parse(raw) {
        return Some(spec);
    }
    ModuleSpecifier::from_file_path(raw).ok()
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}
