//! Real OS file-system watching backing `Deno.watchFs` (and therefore node's
//! `fs.watch`, which chokidar / watchpack build on). Event-driven via the
//! `notify` crate.
//!
//! This REPLACES the previous synchronous recursive-snapshot polling shim. That
//! shim walked the entire watched tree with `statSync`/`readDirSync` on setup
//! (and every 500ms), so `wp.watch()` over a project root that includes
//! `node_modules` blocked the event loop synchronously and watchpack never
//! emitted `aggregated` -- hanging `next dev` at "Starting..." forever. A
//! `notify` watcher is push-based: `watchFs` returns immediately and change
//! events arrive on a channel.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use deno_core::{op2, AsyncRefCell, OpState, RcRef, Resource, ResourceId};
use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum FsEventsError {
    #[class(generic)]
    #[error("{0}")]
    Notify(String),
}

/// A single filesystem event, shaped like `Deno.FsEvent` so the deno_node
/// `fs.watch` polyfill consumes it unchanged (`{ kind, paths }`).
#[derive(Debug, Clone, serde::Serialize)]
struct FsEvent {
    kind: &'static str,
    paths: Vec<String>,
}

impl From<Event> for FsEvent {
    fn from(event: Event) -> Self {
        let kind = match event.kind {
            EventKind::Create(_) => "create",
            EventKind::Modify(ModifyKind::Name(_)) => "rename",
            EventKind::Modify(_) => "modify",
            EventKind::Remove(_) => "remove",
            EventKind::Access(_) => "access",
            EventKind::Other => "other",
            EventKind::Any => "any",
        };
        FsEvent {
            kind,
            paths: event
                .paths
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        }
    }
}

/// Holds the live `notify` watcher (dropping it stops watching) and the receive
/// end of the event channel. Closing the resource drops the watcher, which drops
/// the channel sender and wakes any pending `op_meow_fs_events_poll` with `None`.
struct FsEventsResource {
    watcher: RefCell<Option<RecommendedWatcher>>,
    receiver: AsyncRefCell<mpsc::UnboundedReceiver<FsEvent>>,
}

impl Resource for FsEventsResource {
    fn name(&self) -> Cow<'_, str> {
        "fsEvents".into()
    }

    fn close(self: Rc<Self>) {
        // Drop the watcher -> drop the notify callback -> drop the channel
        // sender -> a pending recv() resolves to None and the JS iterator ends.
        self.watcher.borrow_mut().take();
    }
}

/// Open a recursive (or shallow) watch over `paths`. Returns a resource id whose
/// events are drained by `op_meow_fs_events_poll`.
#[op2]
#[smi]
pub fn op_meow_fs_events_open(
    state: &mut OpState,
    recursive: bool,
    #[serde] paths: Vec<String>,
) -> Result<ResourceId, FsEventsError> {
    let (tx, rx) = mpsc::unbounded_channel::<FsEvent>();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                // Best-effort: a closed receiver just means the watch ended.
                let _ = tx.send(FsEvent::from(event));
            }
        })
        .map_err(|err| FsEventsError::Notify(err.to_string()))?;

    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    for path in &paths {
        watcher
            .watch(Path::new(path), mode)
            .map_err(|err| FsEventsError::Notify(format!("{path}: {err}")))?;
    }

    Ok(state.resource_table.add(FsEventsResource {
        watcher: RefCell::new(Some(watcher)),
        receiver: AsyncRefCell::new(rx),
    }))
}

/// Await the next filesystem event for `rid`. Resolves to `None` when the watch
/// has been closed (resource gone or watcher dropped).
#[op2]
#[serde]
pub async fn op_meow_fs_events_poll(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: ResourceId,
) -> Result<Option<FsEvent>, FsEventsError> {
    let Ok(resource) = state.borrow().resource_table.get::<FsEventsResource>(rid) else {
        return Ok(None);
    };
    let receiver = RcRef::map(resource, |r| &r.receiver);
    let mut receiver = receiver.borrow_mut().await;
    Ok(receiver.recv().await)
}

/// Stop watching and release the resource.
#[op2(fast)]
pub fn op_meow_fs_events_close(state: &mut OpState, #[smi] rid: ResourceId) {
    if let Ok(resource) = state.resource_table.get::<FsEventsResource>(rid) {
        resource.watcher.borrow_mut().take();
    }
    let _ = state.resource_table.take::<FsEventsResource>(rid);
}
