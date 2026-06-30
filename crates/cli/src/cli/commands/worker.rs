//! Cooperative-isolate `node:worker_threads` host (WORKER-001, meow-native).
//!
//! Each `new Worker(specifier)` builds a fresh [`meow_runtime::Runtime`] (its own
//! V8 isolate) and drives it as a `tokio::task::spawn_local` task on the SAME OS
//! thread as the main isolate (a `LocalSet` is installed by the `run` command).
//! This keeps the runtime single-threaded — preserving meow's frozen-clock /
//! seeded-RNG determinism and letting the worker reuse the resolver + Oxc module
//! graph by `Rc`/`Arc` clone (no thread hop, no graph serialization). Messages
//! between isolates ARE structured-cloned (`core.serialize`/`core.deserialize`),
//! because JS values cannot cross an isolate boundary by reference.
//!
//! The JS surface is `vendor/deno_node/polyfills/worker_threads.ts`, which calls
//! the ops below. Worker isolate construction is DEFERRED to the driver's first
//! poll (not done inside `op_meow_worker_create`) so the main isolate is no longer
//! entered when the worker isolate is built — V8 does not support constructing /
//! entering a second isolate from inside a running op on the main one.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use deno_core::{extension, op2, Extension, JsBuffer, OpState};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

use meow_runtime::v8::IsolateHandle;
use meow_runtime::ModuleSpecifier;

use super::run::{build_worker_runtime, WorkerSpawnConfig};

/// Frame tag bytes shared with the polyfill (`op_meow_worker_host_recv` returns
/// `[tag, ...payload]`; an empty frame means the worker exited).
const CTRL_MESSAGE: u8 = 0;
const CTRL_ERROR: u8 = 1;

/// A worker → host event. `Message` carries a `core.serialize`d JS value; `Error`
/// carries a UTF-8 error string (the polyfill wraps it in an `Error`).
enum HostEvent {
    Message(Vec<u8>),
    Error(String),
}

type SharedReceiver<T> = Rc<AsyncMutex<UnboundedReceiver<T>>>;

/// Host-side record for one live worker, kept in [`WorkerManager`].
struct HostWorker {
    inbox: UnboundedSender<Vec<u8>>,
    outbox: SharedReceiver<HostEvent>,
    isolate: Rc<RefCell<Option<IsolateHandle>>>,
    terminated: Rc<Cell<bool>>,
}

/// All live workers spawned from the main isolate. Stored as
/// `Rc<RefCell<WorkerManager>>` in the main runtime's `OpState`.
#[derive(Default)]
pub(super) struct WorkerManager {
    next_id: u32,
    workers: HashMap<u32, HostWorker>,
}

/// Worker-side channels + serialized `workerData`, installed into the WORKER
/// runtime's `OpState` by [`build_worker_runtime`].
pub(super) struct WorkerSideState {
    data: Vec<u8>,
    inbox: SharedReceiver<Vec<u8>>,
    outbox: UnboundedSender<HostEvent>,
}

/// Extension for the MAIN runtime: host ops + the manager and spawn config.
pub(super) fn host_extension(config: WorkerSpawnConfig) -> Extension {
    let mut ext = meow_worker_host::init();
    ext.op_state_fn = Some(Box::new(move |state| {
        state.put::<Rc<RefCell<WorkerManager>>>(Rc::new(RefCell::new(WorkerManager::default())));
        state.put::<WorkerSpawnConfig>(config.clone());
    }));
    ext
}

/// Extension for each WORKER runtime (ops only; the driver installs
/// [`WorkerSideState`] into the runtime's `OpState` after construction).
pub(super) fn guest_extension() -> Extension {
    meow_worker_guest::init()
}

extension!(
    meow_worker_host,
    ops = [
        op_meow_worker_create,
        op_meow_worker_host_post,
        op_meow_worker_host_recv,
        op_meow_worker_terminate,
    ],
);

extension!(
    meow_worker_guest,
    ops = [
        op_meow_worker_post,
        op_meow_worker_recv,
        op_meow_worker_data
    ],
);

// ───────────────────────────── host ops ─────────────────────────────

#[op2]
#[smi]
fn op_meow_worker_create(
    state: &mut OpState,
    #[string] specifier: String,
    #[buffer] worker_data: JsBuffer,
) -> u32 {
    let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
    let config = state.borrow::<WorkerSpawnConfig>().clone();

    let (host_inbox, worker_inbox) = unbounded_channel::<Vec<u8>>();
    let (worker_outbox, host_outbox) = unbounded_channel::<HostEvent>();
    let isolate: Rc<RefCell<Option<IsolateHandle>>> = Rc::new(RefCell::new(None));
    let terminated = Rc::new(Cell::new(false));

    let id = {
        let mut mgr = manager.borrow_mut();
        let id = mgr.next_id;
        mgr.next_id = mgr.next_id.wrapping_add(1);
        mgr.workers.insert(
            id,
            HostWorker {
                inbox: host_inbox,
                outbox: Rc::new(AsyncMutex::new(host_outbox)),
                isolate: isolate.clone(),
                terminated: terminated.clone(),
            },
        );
        id
    };

    let worker_side = WorkerSideState {
        data: worker_data.to_vec(),
        inbox: Rc::new(AsyncMutex::new(worker_inbox)),
        outbox: worker_outbox,
    };

    tokio::task::spawn_local(drive_worker(
        config,
        id,
        specifier,
        worker_side,
        isolate,
        terminated,
    ));
    id
}

#[op2]
fn op_meow_worker_host_post(state: &mut OpState, #[smi] id: u32, #[buffer] data: JsBuffer) {
    let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
    let mgr = manager.borrow();
    if let Some(worker) = mgr.workers.get(&id) {
        let _ = worker.inbox.send(data.to_vec());
    }
}

#[op2]
#[buffer]
async fn op_meow_worker_host_recv(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    let outbox = {
        let state = state.borrow();
        let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
        let mgr = manager.borrow();
        match mgr.workers.get(&id) {
            Some(worker) => worker.outbox.clone(),
            None => return Ok(Vec::new()),
        }
    };
    let mut rx = outbox.lock().await;
    Ok(match rx.recv().await {
        Some(HostEvent::Message(bytes)) => frame(CTRL_MESSAGE, &bytes),
        Some(HostEvent::Error(text)) => frame(CTRL_ERROR, text.as_bytes()),
        None => Vec::new(),
    })
}

#[op2]
fn op_meow_worker_terminate(state: &mut OpState, #[smi] id: u32) {
    let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
    let mut mgr = manager.borrow_mut();
    if let Some(worker) = mgr.workers.remove(&id) {
        worker.terminated.set(true);
        if let Some(handle) = worker.isolate.borrow().as_ref() {
            handle.terminate_execution();
        }
    }
}

// ──────────────────────────── guest ops ────────────────────────────

#[op2]
fn op_meow_worker_post(state: &mut OpState, #[buffer] data: JsBuffer) {
    if let Some(side) = state.try_borrow::<WorkerSideState>() {
        let _ = side.outbox.send(HostEvent::Message(data.to_vec()));
    }
}

#[op2]
#[buffer]
async fn op_meow_worker_recv(
    state: Rc<RefCell<OpState>>,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    let inbox = {
        let state = state.borrow();
        match state.try_borrow::<WorkerSideState>() {
            Some(side) => side.inbox.clone(),
            None => return Ok(Vec::new()),
        }
    };
    let mut rx = inbox.lock().await;
    Ok(rx.recv().await.unwrap_or_default())
}

#[op2]
#[buffer]
fn op_meow_worker_data(state: &mut OpState) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    Ok(state
        .try_borrow::<WorkerSideState>()
        .map(|side| side.data.clone())
        .unwrap_or_default())
}

fn frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.push(tag);
    out.extend_from_slice(payload);
    out
}

// ───────────────────────────── driver ─────────────────────────────

async fn drive_worker(
    config: WorkerSpawnConfig,
    id: u32,
    specifier: String,
    worker_side: WorkerSideState,
    isolate: Rc<RefCell<Option<IsolateHandle>>>,
    terminated: Rc<Cell<bool>>,
) {
    if terminated.get() {
        return;
    }
    let outbox = worker_side.outbox.clone();
    let Some(spec) = worker_module_specifier(&specifier) else {
        let _ = outbox.send(HostEvent::Error(format!(
            "worker_threads: invalid worker specifier `{specifier}`"
        )));
        return;
    };

    // Build the worker isolate now — the main isolate is no longer entered here,
    // so constructing + bootstrapping a second isolate is safe.
    let mut runtime = match build_worker_runtime(&config, &spec, worker_side) {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = outbox.send(HostEvent::Error(err));
            return;
        }
    };
    if terminated.get() {
        return;
    }
    *isolate.borrow_mut() = Some(runtime.isolate_handle());

    // Flip the freshly-bootstrapped (main-mode) isolate into worker mode: sets
    // isMainThread = false and wires parentPort + workerData. Runs at top level,
    // so it uses the globalThis hook the polyfill installed during bootstrap.
    let init = format!(
        "globalThis.__meowInitWorkerThread({id}, {});",
        json_string(spec.as_str())
    );
    if let Err(err) = runtime.execute_script("meow:worker-init", init) {
        let _ = outbox.send(HostEvent::Error(err.to_string()));
        return;
    }

    // Drive the worker module to completion. It stays alive while parentPort has a
    // pending receive; `terminate()` stops it via `IsolateHandle::terminate_execution`.
    if let Err(err) = runtime.run_main_module(&spec).await {
        if !terminated.get() {
            let _ = outbox.send(HostEvent::Error(err.to_string()));
        }
    }
    // `outbox` and `runtime` drop here: the host's pending recv resolves to
    // `None` → an empty frame → the `Worker` emits `exit`.
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
