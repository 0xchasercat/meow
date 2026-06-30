//! Cooperative-isolate `node:worker_threads` ops + manager (WORKER-001).
//!
//! These ops are baked into the V8 snapshot — `build.rs` registers
//! [`worker_extension`] just like the runtime does, so `core.ops.op_meow_worker_*`
//! resolve from the snapshot (runtime-only extension ops are NOT exposed on
//! `core.ops` under a snapshot).
//!
//! This crate stays free of the resolver / module graph / runtime construction:
//! `op_meow_worker_create` hands a [`WorkerSpawnRequest`] to a host-supplied
//! [`WorkerSpawner`] (the binary edge owns building + driving the worker isolate
//! on the thread's `LocalSet`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

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
pub enum HostEvent {
    Message(Vec<u8>),
    Error(String),
}

/// A single-consumer receiver shared with the async recv ops.
pub type SharedReceiver<T> = Rc<AsyncMutex<UnboundedReceiver<T>>>;

/// Host-side record for one live worker, kept in [`WorkerManager`].
struct HostWorker {
    inbox: UnboundedSender<Vec<u8>>,
    outbox: SharedReceiver<HostEvent>,
    isolate: Rc<RefCell<Option<IsolateHandle>>>,
    terminated: Rc<Cell<bool>>,
}

/// All live workers spawned from a runtime; stored as `Rc<RefCell<WorkerManager>>`
/// in the main runtime's `OpState` by [`worker_extension`].
#[derive(Default)]
pub struct WorkerManager {
    next_id: u32,
    workers: HashMap<u32, HostWorker>,
}

/// Worker-side channels + serialized `workerData`. The host installs this into the
/// WORKER runtime's `OpState`; the guest ops read it.
pub struct WorkerSideState {
    data: Vec<u8>,
    inbox: SharedReceiver<Vec<u8>>,
    outbox: UnboundedSender<HostEvent>,
}

impl WorkerSideState {
    pub fn new(
        data: Vec<u8>,
        inbox: SharedReceiver<Vec<u8>>,
        outbox: UnboundedSender<HostEvent>,
    ) -> WorkerSideState {
        WorkerSideState {
            data,
            inbox,
            outbox,
        }
    }
}

/// Everything the host needs to build + drive one worker isolate. Produced by
/// `op_meow_worker_create` and handed to the [`WorkerSpawner`].
pub struct WorkerSpawnRequest {
    pub id: u32,
    pub specifier: String,
    pub worker_data: Vec<u8>,
    pub inbox: SharedReceiver<Vec<u8>>,
    pub outbox: UnboundedSender<HostEvent>,
    pub isolate: Rc<RefCell<Option<IsolateHandle>>>,
    pub terminated: Rc<Cell<bool>>,
}

/// Host seam: builds + drives a worker isolate. Implemented at the binary edge
/// (which owns the resolver / module graph / runtime construction); installed
/// into the main runtime's `OpState` via [`worker_extension`].
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

    spawner.spawn(WorkerSpawnRequest {
        id,
        specifier,
        worker_data: worker_data.to_vec(),
        inbox: Rc::new(AsyncMutex::new(worker_inbox)),
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

#[op2(fast)]
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

// ----------------------------- guest ops -----------------------------

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
