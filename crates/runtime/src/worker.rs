//! Thread-per-worker `node:worker_threads` ops + manager (WORKER-001).
//!
//! These ops are baked into the V8 snapshot — `build.rs` registers
//! [`worker_extension`] just like the runtime does, so `core.ops.op_meow_worker_*`
//! resolve from the snapshot (runtime-only extension ops are NOT exposed on
//! `core.ops` under a snapshot).
//!
//! ## Threading model
//! Each `new Worker(...)` runs on its OWN OS thread with its OWN
//! `deno_core::JsRuntime` (V8 isolate) and its OWN current-thread tokio runtime.
//! This is forced by rusty_v8: an `OwnedIsolate` is *entered on construction and
//! exited on drop*, and V8 keeps a strict-LIFO stack of entered isolates per
//! thread — so two long-lived isolates cannot be interleaved on one thread. A
//! thread apiece sidesteps that entirely (each thread has its own isolate stack).
//!
//! Determinism is unaffected: the host clones the run's [`crate::hermetic`]
//! config into every worker, so each isolate has the same frozen clock origin +
//! seed. Only Send messages cross the boundary (structured-cloned via
//! `core.serialize`/`core.deserialize`); the immutable resolution graph + cache
//! are shared by `Arc`, so a worker does no re-resolution.
//!
//! This crate stays free of the resolver / module graph / runtime construction:
//! `op_meow_worker_create` hands a [`WorkerSpawnRequest`] to a host-supplied
//! [`WorkerSpawner`] (the binary edge owns building + driving the worker isolate
//! on its own thread).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use deno_core::{extension, op2, Extension, JsBuffer, OpState};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

use crate::v8::IsolateHandle;

/// Framing tags shared with `worker_threads.ts`: `op_meow_worker_host_recv`
/// returns `[tag, ...payload]`; an empty buffer means the worker exited.
const CTRL_MESSAGE: u8 = 0;
const CTRL_ERROR: u8 = 1;

/// A worker -> host event. `Message` carries a `core.serialize`d JS value;
/// `Error` carries a UTF-8 error string (the polyfill wraps it in an `Error`).
/// Both variants are `Send`, so the worker->host channel crosses OS threads.
pub enum HostEvent {
    Message(Vec<u8>),
    Error(String),
}

/// A single-consumer receiver shared with the async recv ops. Constructed and
/// used ENTIRELY on one thread (main for the host outbox, the worker thread for
/// the worker inbox), so the `Rc` never crosses a thread boundary.
pub type SharedReceiver<T> = Rc<AsyncMutex<UnboundedReceiver<T>>>;

/// A set-once, lock-free slot for a worker's V8 isolate handle. The worker
/// thread publishes its handle after construction; the host reads it (from the
/// main thread) to `terminate()` the worker cross-thread. [`IsolateHandle`] is
/// `Send + Sync` by design, so this is safe.
pub type SharedIsolate = Arc<OnceLock<IsolateHandle>>;

/// Host-side record for one live worker, kept in [`WorkerManager`] on the main
/// thread.
struct HostWorker {
    /// host -> worker messages (the worker holds the matching receiver).
    inbox: UnboundedSender<Vec<u8>>,
    /// worker -> host events; read by `op_meow_worker_host_recv` on the main
    /// thread only, so the `Rc` wrapper stays thread-local.
    outbox: SharedReceiver<HostEvent>,
    /// The worker's isolate handle (published by the worker thread once built).
    isolate: SharedIsolate,
    /// Cooperative-stop hint; the hard stop is [`IsolateHandle::terminate_execution`].
    terminated: Arc<AtomicBool>,
}

/// All live workers spawned from a runtime; stored as `Rc<RefCell<WorkerManager>>`
/// in the main runtime's `OpState` by [`worker_extension`]. Main-thread only.
#[derive(Default)]
pub struct WorkerManager {
    next_id: u32,
    workers: HashMap<u32, HostWorker>,
}

/// Worker-side channels + serialized `workerData`. The host installs this into
/// the WORKER runtime's `OpState` (on the worker thread); the guest ops read it.
/// `!Send` by design — it is built and consumed entirely on the worker thread.
pub struct WorkerSideState {
    data: Vec<u8>,
    inbox: SharedReceiver<Vec<u8>>,
    outbox: UnboundedSender<HostEvent>,
}

impl WorkerSideState {
    /// Wrap the raw (Send) channel endpoints handed across the thread boundary
    /// into the thread-local shapes the guest ops expect. Call on the worker
    /// thread.
    pub fn new(
        data: Vec<u8>,
        inbox: UnboundedReceiver<Vec<u8>>,
        outbox: UnboundedSender<HostEvent>,
    ) -> WorkerSideState {
        WorkerSideState {
            data,
            inbox: Rc::new(AsyncMutex::new(inbox)),
            outbox,
        }
    }
}

/// Everything the host needs to build + drive one worker isolate on its own OS
/// thread. Produced by `op_meow_worker_create` and handed to the
/// [`WorkerSpawner`]. Every field is `Send` so the whole request can be moved
/// into a `std::thread::spawn` closure.
pub struct WorkerSpawnRequest {
    pub id: u32,
    pub specifier: String,
    pub worker_data: Vec<u8>,
    /// host -> worker messages (bare receiver; wrapped via [`WorkerSideState::new`]
    /// on the worker thread).
    pub inbox: UnboundedReceiver<Vec<u8>>,
    /// worker -> host events.
    pub outbox: UnboundedSender<HostEvent>,
    /// Slot for the worker to publish its isolate handle into.
    pub isolate: SharedIsolate,
    /// Cooperative-stop hint set by `op_meow_worker_terminate`.
    pub terminated: Arc<AtomicBool>,
}

/// Host seam: builds + drives a worker isolate on its own OS thread. Implemented
/// at the binary edge (which owns the resolver / module graph / runtime
/// construction); installed into the main runtime's `OpState` via
/// [`worker_extension`]. `spawn` runs on the main thread and must not block — it
/// spawns the worker's OS thread and returns.
pub trait WorkerSpawner {
    fn spawn(&self, request: WorkerSpawnRequest);
}

/// Build the `meow_worker` extension. Register it in BOTH the snapshot (`build.rs`,
/// `spawner = None`) and the runtime (`spawner = Some(..)`) so the ops bind from
/// the snapshot. The worker runtime also registers it (`None`) for the guest ops.
pub fn worker_extension(spawner: Option<Rc<dyn WorkerSpawner>>) -> Extension {
    let mut ext = meow_worker::init();
    ext.op_state_fn = Some(Box::new(move |state| {
        state.put::<Rc<RefCell<WorkerManager>>>(Rc::new(RefCell::new(WorkerManager::default())));
        if let Some(spawner) = spawner.clone() {
            state.put::<Rc<dyn WorkerSpawner>>(spawner);
        }
    }));
    ext
}

extension!(
    meow_worker,
    ops = [
        op_meow_worker_create,
        op_meow_worker_host_post,
        op_meow_worker_host_recv,
        op_meow_worker_terminate,
        op_meow_worker_post,
        op_meow_worker_recv,
        op_meow_worker_data,
    ],
);

fn frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 1);
    out.push(tag);
    out.extend_from_slice(payload);
    out
}

// ----------------------------- host ops -----------------------------

#[op2]
#[smi]
fn op_meow_worker_create(
    state: &mut OpState,
    #[string] specifier: String,
    #[buffer] worker_data: JsBuffer,
) -> u32 {
    let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
    let Some(spawner) = state.try_borrow::<Rc<dyn WorkerSpawner>>().cloned() else {
        // No spawner installed (e.g. snapshot warmup, or a context that does not
        // support workers). u32::MAX is an inert id the host never tracks.
        return u32::MAX;
    };

    let (host_inbox, worker_inbox) = unbounded_channel::<Vec<u8>>();
    let (worker_outbox, host_outbox) = unbounded_channel::<HostEvent>();
    let isolate: SharedIsolate = Arc::new(OnceLock::new());
    let terminated = Arc::new(AtomicBool::new(false));

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

    spawner.spawn(WorkerSpawnRequest {
        id,
        specifier,
        worker_data: worker_data.to_vec(),
        inbox: worker_inbox,
        outbox: worker_outbox,
        isolate,
        terminated,
    });
    id
}

#[op2]
fn op_meow_worker_host_post(state: &mut OpState, #[smi] id: u32, #[buffer] data: JsBuffer) {
    let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
    let mgr = manager.borrow();
    if let Some(worker) = mgr.workers.get(&id) {
        // Unbounded send never blocks: it just enqueues + wakes the worker's
        // event loop on its own thread.
        let _ = worker.inbox.send(data.to_vec());
    }
}

#[op2]
#[buffer]
async fn op_meow_worker_host_recv(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    let (manager, outbox) = {
        let state = state.borrow();
        let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
        let outbox = match manager.borrow().workers.get(&id) {
            Some(worker) => worker.outbox.clone(),
            None => return Ok(Vec::new()),
        };
        (manager, outbox)
    };
    let mut rx = outbox.lock().await;
    match rx.recv().await {
        Some(HostEvent::Message(bytes)) => Ok(frame(CTRL_MESSAGE, &bytes)),
        Some(HostEvent::Error(text)) => Ok(frame(CTRL_ERROR, text.as_bytes())),
        None => {
            // Worker exited (its outbox sender dropped): drop the host record so
            // a long-lived main process does not accumulate dead entries. The
            // cloned `outbox` Rc keeps the receiver alive until this op returns.
            manager.borrow_mut().workers.remove(&id);
            Ok(Vec::new())
        }
    }
}

#[op2(fast)]
fn op_meow_worker_terminate(state: &mut OpState, #[smi] id: u32) {
    let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
    let mut mgr = manager.borrow_mut();
    if let Some(worker) = mgr.workers.remove(&id) {
        worker.terminated.store(true, Ordering::Relaxed);
        // Hard stop: interrupt V8 on the worker thread. Safe cross-thread — that
        // is exactly what `IsolateHandle` is for. No-op if the worker has not
        // published its handle yet (it checks `terminated` before running).
        if let Some(handle) = worker.isolate.get() {
            handle.terminate_execution();
        }
    }
}

// ----------------------------- guest ops -----------------------------

#[op2]
fn op_meow_worker_post(state: &mut OpState, #[buffer] data: JsBuffer) {
    if let Some(side) = state.try_borrow::<WorkerSideState>() {
        let _ = side.outbox.send(HostEvent::Message(data.to_vec()));
    }
}

#[op2]
#[buffer]
async fn op_meow_worker_recv(state: Rc<RefCell<OpState>>) -> Result<Vec<u8>, deno_error::JsErrorBox> {
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
