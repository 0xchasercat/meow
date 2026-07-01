//! Thread-per-worker `node:worker_threads` host edge (WORKER-001, meow-native).
//!
//! The worker OPS + manager live in [`meow_runtime::worker`] so they bake into
//! the V8 startup snapshot — `core.ops.op_meow_worker_*` resolve from the
//! snapshot, whereas a runtime-only extension's ops do NOT. This module is the
//! BINARY edge: it owns runtime construction (resolver / Oxc module graph /
//! [`build_worker_runtime`]), which `meow_runtime` deliberately does not depend
//! on. It implements [`WorkerSpawner`] and drives each worker isolate on its OWN
//! OS thread.
//!
//! Why a thread apiece (not cooperative isolates on one thread): rusty_v8 enters
//! an `OwnedIsolate` on construction and only exits it on drop, with a strict
//! LIFO stack of entered isolates per thread. Two long-lived isolates therefore
//! cannot be interleaved on one thread — a thread each is the only sound model
//! (and is what Deno/Node do).
//!
//! Performance: the worker reuses the project's immutable resolution graph +
//! content-cache by `Arc` clone (see [`WorkerSpawnConfig`]), so it does NO
//! re-resolution or lockfile re-parse; it loads from the same V8 snapshot; and
//! process-global V8 flags are applied once on the main thread, never re-applied
//! per worker. Only structured-cloned messages cross the boundary.

use std::sync::atomic::Ordering;

use meow_runtime::worker::{HostEvent, WorkerSideState, WorkerSpawnRequest, WorkerSpawner};
use meow_runtime::ModuleSpecifier;

use crate::cli::hiss;

use super::run::{build_worker_runtime, WorkerSpawnConfig};

// Compile-time guarantee that everything moved onto a worker OS thread is `Send`.
// If a future change adds an `!Send` field (e.g. an `Rc`) to the config or the
// spawn request, this fails to compile here with a clear pointer, instead of a
// confusing closure error at the `thread::spawn` call site.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<WorkerSpawnConfig>();
    assert_send::<WorkerSpawnRequest>();
};

/// Native stack for worker OS threads. V8's JS stack is separate, but deep
/// deno_core/V8 native recursion (module instantiation, structured clone of deep
/// graphs) can blow the ~2 MiB default; 16 MiB (virtual, not resident) matches
/// main-thread headroom and avoids a class of native stack overflows up front.
const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

/// Binary-edge spawner installed into the main runtime's `OpState` via
/// `meow_runtime::worker::worker_extension(Some(..))`. `op_meow_worker_create`
/// calls [`WorkerSpawner::spawn`] on the main thread with everything needed to
/// build + drive the worker isolate on its own thread.
pub(super) struct CliWorkerSpawner {
    pub(super) config: WorkerSpawnConfig,
}

impl WorkerSpawner for CliWorkerSpawner {
    fn spawn(&self, request: WorkerSpawnRequest) {
        let config = self.config.clone();
        let builder = std::thread::Builder::new()
            .name(format!("meow-worker-{}", request.id))
            .stack_size(WORKER_STACK_SIZE);
        // Detached: the worker owns its lifetime. Normal exit drops its outbox
        // (host sees EOF -> emits `exit`); `terminate()` stops it via the V8
        // `IsolateHandle`. We never join — that would block the main thread.
        if let Err(err) = builder.spawn(move || run_worker_thread(config, request)) {
            // Thread creation failed (OOM / thread limit). `request` moved into
            // the closure only on success, so its channels are dropped here ->
            // the JS side's pending recv resolves to EOF and the `Worker` emits
            // `exit`. Surface the cause through the branded UI (stderr, and
            // non-interactive-safe).
            hiss(&format!(
                "worker_threads: failed to spawn worker thread: {err}"
            ));
        }
    }
}

/// Worker OS-thread entrypoint: stand up a dedicated single-threaded tokio
/// runtime + V8 isolate for this worker, then drive it to completion.
fn run_worker_thread(config: WorkerSpawnConfig, request: WorkerSpawnRequest) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = request.outbox.send(HostEvent::Error(format!(
                "worker_threads: failed to start worker runtime: {err}"
            )));
            return;
        }
    };
    runtime.block_on(drive_worker(config, request));
}

async fn drive_worker(config: WorkerSpawnConfig, req: WorkerSpawnRequest) {
    if req.terminated.load(Ordering::Relaxed) {
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

    let worker_side = WorkerSideState::new(req.worker_data, req.inbox, req.outbox);
    let mut runtime = match build_worker_runtime(&config, &spec, worker_side) {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = outbox.send(HostEvent::Error(err));
            return;
        }
    };
    if req.terminated.load(Ordering::Relaxed) {
        return;
    }
    // Publish the isolate handle so the host can `terminate()` us cross-thread.
    // Set-once; ignore the (impossible) already-set case.
    let _ = req.isolate.set(runtime.isolate_handle());

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
    // a pending receive; `terminate()` stops it via `terminate_execution`.
    if let Err(err) = runtime.run_main_module(&spec).await {
        if !req.terminated.load(Ordering::Relaxed) {
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
