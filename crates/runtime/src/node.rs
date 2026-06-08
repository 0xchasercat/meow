//! Native Node built-in runtime wiring (RT-007).
//!
//! This module owns the runtime-side part of the first node-compat slice:
//! - the `meow_node` bootstrap that installs `process` / `Buffer` globals when
//!   node-compat is enabled,
//! - the per-run process metadata (`argv`, `cwd`, versions), and
//! - the strict-web withdrawal policy for host-backed Node built-ins.
//!
//! The actual module sources (`node:fs`, `node:path`, ...) live in `native.rs`
//! and resolve through the shared loader. This file only wires the op/state seam.

use std::path::PathBuf;
use std::rc::Rc;

use deno_core::{op2, JsRuntime, OpState};
use serde::Serialize;

use crate::io::{io_capability_extension, meow_io, CapDenied, CapRequest, CapabilityCheck};

const NODE_COMPAT_VERSION: &str = "22.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMode {
    Enabled,
    StrictWeb,
}

impl NodeMode {
    fn globals_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug, Clone)]
pub struct NodeOptions {
    pub mode: NodeMode,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
}

impl NodeOptions {
    pub fn enabled(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            mode: NodeMode::Enabled,
            argv,
            cwd,
        }
    }

    pub fn strict_web(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            mode: NodeMode::StrictWeb,
            argv,
            cwd,
        }
    }
}

#[derive(Debug, Clone)]
struct NodeRuntimeState {
    mode: NodeMode,
    argv: Vec<String>,
    cwd: String,
}

impl NodeRuntimeState {
    fn new(opts: NodeOptions) -> Self {
        Self {
            mode: opts.mode,
            argv: opts.argv,
            cwd: opts.cwd.to_string_lossy().into_owned(),
        }
    }

    fn fallback() -> Self {
        Self {
            mode: NodeMode::StrictWeb,
            argv: Vec::new(),
            cwd: ".".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct NodeVersions {
    node: String,
    v8: String,
    meow: String,
}

#[derive(Debug, Clone, Serialize)]
struct NodeProcessInfo {
    enabled: bool,
    argv: Vec<String>,
    cwd: String,
    platform: String,
    arch: String,
    version: String,
    versions: NodeVersions,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessExitCode(pub i32);

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
#[error("process.exit({code})")]
struct ProcessExitError {
    code: i32,
}

fn node_state(state: &OpState) -> NodeRuntimeState {
    state
        .try_borrow::<NodeRuntimeState>()
        .cloned()
        .unwrap_or_else(NodeRuntimeState::fallback)
}

fn node_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "android") {
        "android"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else if cfg!(target_os = "netbsd") {
        "netbsd"
    } else if cfg!(target_os = "aix") {
        "aix"
    } else if cfg!(target_os = "solaris") {
        "sunos"
    } else {
        "unknown"
    }
}

fn node_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    }
}

fn process_info(state: &OpState) -> NodeProcessInfo {
    let st = node_state(state);
    let versions = NodeVersions {
        node: NODE_COMPAT_VERSION.to_owned(),
        v8: deno_core::v8::V8::get_version().to_owned(),
        meow: env!("CARGO_PKG_VERSION").to_owned(),
    };
    NodeProcessInfo {
        enabled: st.mode.globals_enabled(),
        argv: st.argv,
        cwd: st.cwd,
        platform: node_platform().to_owned(),
        arch: node_arch().to_owned(),
        version: format!("v{}", NODE_COMPAT_VERSION),
        versions,
    }
}

#[op2]
#[serde]
fn op_node_process_info(state: &mut OpState) -> NodeProcessInfo {
    process_info(state)
}

#[op2(fast)]
fn op_node_process_exit(state: &mut OpState, code: i32) -> Result<(), ProcessExitError> {
    state.put(ProcessExitCode(code));
    Err(ProcessExitError { code })
}

deno_core::extension!(
    meow_node,
    ops = [op_node_process_info, op_node_process_exit],
    esm_entry_point = "ext:meow_node/bootstrap.js",
    esm = [dir "src/js/node", "bootstrap.js"],
);

pub fn node_state_extension(opts: NodeOptions) -> deno_core::Extension {
    let state = NodeRuntimeState::new(opts);
    deno_core::Extension {
        name: "meow_node_state",
        op_state_fn: Some(Box::new(move |op_state: &mut OpState| {
            op_state.put(state.clone());
        })),
        ..Default::default()
    }
}

pub fn extensions(opts: NodeOptions) -> Vec<deno_core::Extension> {
    let mode = opts.mode;
    let mut exts = vec![
        meow_io::init(),
        meow_node::init(),
        node_state_extension(opts),
    ];
    if matches!(mode, NodeMode::StrictWeb) {
        exts.push(io_capability_extension(Rc::new(StrictWebIoCaps)));
    }
    exts
}

pub fn take_process_exit_code(js_runtime: &JsRuntime) -> Option<i32> {
    js_runtime
        .op_state()
        .borrow_mut()
        .try_take::<ProcessExitCode>()
        .map(|code| code.0)
}

struct StrictWebIoCaps;

impl CapabilityCheck for StrictWebIoCaps {
    fn check(&self, req: &CapRequest<'_>) -> Result<(), CapDenied> {
        match req {
            CapRequest::ReadFile(path)
            | CapRequest::WriteFile(path)
            | CapRequest::ReadDir(path)
            | CapRequest::Stat(path)
            | CapRequest::Mkdir(path)
            | CapRequest::Access(path)
            | CapRequest::Remove(path) => Err(CapDenied(format!(
                "ERR_STRICT_WEB_WITHDRAWN: strict-web mode withdraws node:fs access to {}",
                path.display()
            ))),
            CapRequest::CurrentDir => Err(CapDenied(
                "ERR_STRICT_WEB_WITHDRAWN: strict-web mode withdraws process.cwd()".to_owned(),
            )),
            CapRequest::NetConnect(_) | CapRequest::NetListen(_) => Ok(()),
        }
    }
}
