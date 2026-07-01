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
use tokio::sync::Notify;

use crate::v8::IsolateHandle;

/// Framing tags shared with `worker_threads.ts`: `op_meow_worker_host_recv`
/// returns `[tag, ...payload]`; an empty buffer means the worker exited.
/// `Online` carries no payload — it fires the Node `'online'` event the moment
/// the worker isolate starts, independent of any message (worker pools wait on
/// `'online'` before dispatching work, so tying it to the first message would
/// deadlock).
const CTRL_MESSAGE: u8 = 0;
const CTRL_ERROR: u8 = 1;
const CTRL_ONLINE: u8 = 2;

/// Opt-in worker lifecycle tracing (`MEOW_WORKER_DEBUG`). The BINARY EDGE is the
/// only place allowed to read host env (I-6); it calls [`set_worker_debug`], and
/// the ops here only ever read this process-global flag — never the environment.
static WORKER_DEBUG: AtomicBool = AtomicBool::new(false);

/// Enable/disable worker tracing. Called once by the binary edge after reading
/// `MEOW_WORKER_DEBUG`.
pub fn set_worker_debug(on: bool) {
    WORKER_DEBUG.store(on, Ordering::Relaxed);
}

/// Whether worker tracing is on (read by both this crate's ops and the binary
/// edge's driver, so both share one env-free source of truth).
pub fn worker_debug() -> bool {
    WORKER_DEBUG.load(Ordering::Relaxed)
}

macro_rules! wtrace {
    ($id:expr, $($arg:tt)*) => {
        if $crate::worker::worker_debug() {
            eprintln!("[meow-worker {}] {}", $id, format!($($arg)*));
        }
    };
}

/// A worker -> host event. `Message` carries a `core.serialize`d JS value;
/// `Error` carries a UTF-8 error string (the polyfill wraps it in an `Error`);
/// `Online` signals the worker isolate has started. All variants are `Send`, so
/// the worker->host channel crosses OS threads.
pub enum HostEvent {
    Online,
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
    /// Wakes a pending `op_meow_worker_host_recv` on `terminate()` so the main
    /// event loop stops waiting on a worker that may be parked (idle) and thus
    /// not about to drop its outbox on its own. Main-thread only.
    terminate_wake: Rc<Notify>,
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
    id: u32,
    data: Vec<u8>,
    inbox: SharedReceiver<Vec<u8>>,
    outbox: UnboundedSender<HostEvent>,
}

impl WorkerSideState {
    /// Wrap the raw (Send) channel endpoints handed across the thread boundary
    /// into the thread-local shapes the guest ops expect. Call on the worker
    /// thread. `id` is carried only for trace correlation with the host side.
    pub fn new(
        id: u32,
        data: Vec<u8>,
        inbox: UnboundedReceiver<Vec<u8>>,
        outbox: UnboundedSender<HostEvent>,
    ) -> WorkerSideState {
        WorkerSideState {
            id,
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
        wtrace!(u32::MAX, "create: no spawner installed -> inert worker");
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
                terminate_wake: Rc::new(Notify::new()),
            },
        );
        id
    };

    wtrace!(id, "create spec={specifier} data={}B", worker_data.len());
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
        wtrace!(id, "host_post -> worker ({}B)", data.len());
        let _ = worker.inbox.send(data.to_vec());
    } else {
        wtrace!(id, "host_post: worker not found (already exited?)");
    }
}

#[op2]
#[buffer]
async fn op_meow_worker_host_recv(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    let (manager, outbox, terminate_wake) = {
        let state = state.borrow();
        let manager = state.borrow::<Rc<RefCell<WorkerManager>>>().clone();
        let (outbox, terminate_wake) = match manager.borrow().workers.get(&id) {
            Some(worker) => (worker.outbox.clone(), worker.terminate_wake.clone()),
            None => return Ok(Vec::new()),
        };
        (manager, outbox, terminate_wake)
    };
    let mut rx = outbox.lock().await;
    // `terminate()` may fire while the worker is parked (idle) and therefore not
    // about to drop its outbox; the notify wakes us so the main event loop is not
    // pinned open by this pending recv. `biased` prioritises prompt termination.
    let event = tokio::select! {
        biased;
        _ = terminate_wake.notified() => None,
        ev = rx.recv() => ev,
    };
    match event {
        Some(HostEvent::Online) => {
            wtrace!(id, "host_recv <- ONLINE");
            Ok(frame(CTRL_ONLINE, &[]))
        }
        Some(HostEvent::Message(bytes)) => {
            wtrace!(id, "host_recv <- message ({}B)", bytes.len());
            Ok(frame(CTRL_MESSAGE, &bytes))
        }
        Some(HostEvent::Error(text)) => {
            wtrace!(id, "host_recv <- error: {text}");
            Ok(frame(CTRL_ERROR, text.as_bytes()))
        }
        None => {
            // Worker exited or was terminated: drop the host record so a
            // long-lived main process does not accumulate dead entries. The
            // cloned `outbox` Rc keeps the receiver alive until this op returns.
            wtrace!(id, "host_recv <- EOF (exit)");
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
        wtrace!(id, "terminate");
        worker.terminated.store(true, Ordering::Relaxed);
        // Hard stop: interrupt V8 on the worker thread if it is executing JS.
        // Safe cross-thread — that is exactly what `IsolateHandle` is for. No-op
        // if the worker has not published its handle yet (it checks `terminated`
        // before running).
        if let Some(handle) = worker.isolate.get() {
            handle.terminate_execution();
        }
        // Dropping `worker` closes the inbox (worker's recv -> EOF, so an idle
        // worker winds down); the notify wakes any pending host recv now so the
        // main event loop can complete without waiting on that chain.
        worker.terminate_wake.notify_one();
    }
}

// ----------------------------- guest ops -----------------------------

#[op2]
fn op_meow_worker_post(state: &mut OpState, #[buffer] data: JsBuffer) {
    if let Some(side) = state.try_borrow::<WorkerSideState>() {
        wtrace!(side.id, "post -> host ({}B)", data.len());
        let _ = side.outbox.send(HostEvent::Message(data.to_vec()));
    }
}

#[op2]
#[buffer]
async fn op_meow_worker_recv(
    state: Rc<RefCell<OpState>>,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    let (id, inbox) = {
        let state = state.borrow();
        match state.try_borrow::<WorkerSideState>() {
            Some(side) => (side.id, side.inbox.clone()),
            None => return Ok(Vec::new()),
        }
    };
    let mut rx = inbox.lock().await;
    let msg = rx.recv().await.unwrap_or_default();
    wtrace!(
        id,
        "recv <- host ({}B{})",
        msg.len(),
        if msg.is_empty() { " EOF" } else { "" }
    );
    Ok(msg)
}

#[op2]
#[buffer]
fn op_meow_worker_data(state: &mut OpState) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    Ok(state
        .try_borrow::<WorkerSideState>()
        .map(|side| side.data.clone())
        .unwrap_or_default())
}
