//! Deno-owned Node compatibility runtime wiring (RT-007).
//!
//! Node built-ins, CommonJS, and N-API are delegated to upstream Deno crates.
//! `meow-runtime` only assembles those extensions around services supplied by
//! the binary edge; it does not depend on `meow-loader` and does not own
//! JavaScript `node:*` polyfills.

#[path = "node_bridge.rs"]
pub mod node_bridge;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use deno_core::{op2, Extension, JsRuntime, OpState};
use deno_permissions::{
    PermissionDescriptorParser, Permissions, PermissionsContainer as DenoPermissionsContainer,
    PermissionsOptions, RuntimePermissionDescriptorParser,
};

pub use node_bridge::{
    DenoNodeBridge, DenoNodeExtInitServices, DenoNodeServices, DenoNodeServicesBuilder,
    DenoRtNativeAddonLoaderRc, FastString, FileSystemRc, InNpmPackageChecker, JsErrorBox,
    NodeRequireLoader, NodeRequireLoaderRc, NodeResolverOptions, NpmPackageFolderResolver,
    PackageFolderResolveError, PackageJsonLoadError, PermissionsContainer, Url, UrlOrPathRef,
    Version,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMode {
    Enabled,
    StrictWeb,
}

#[derive(Clone)]
pub struct NodeOptions {
    pub mode: NodeMode,
    pub argv: Vec<String>,
    pub main_module: Option<String>,
    pub cwd: PathBuf,
    // === RUN-001 ===
    pub env: BTreeMap<String, String>,
    // === /RUN-001 ===
    /// Deno Node/N-API services prebuilt by the runtime caller from the package
    /// graph/cache projection. `None` keeps strict runtime construction possible
    /// while CLI wiring moves to the Deno bridge.
    pub deno_node_services: Option<DenoNodeServices>,
    pub caps: Option<std::sync::Arc<dyn crate::io::CapabilityCheck + Send + Sync>>,
    pub user_agent: Option<String>,
    // === SEC-001 ===
    /// Host-access enforcement for this runtime. `None` = trusted (allow_all,
    /// byte-identical to pre-SEC-001 `meow run`). `Some(policy)` = sandboxed
    /// (`meow x` by default, `meow run --sandbox`): the Node stack gets a
    /// restrictive `PermissionsContainer` and meow's seam gets [`SandboxCaps`],
    /// both derived from the one policy so fs/net enforcement stays consistent.
    pub sandbox: Option<crate::io::SandboxPolicy>,
    // === /SEC-001 ===
}

impl NodeOptions {
    pub fn enabled(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            mode: NodeMode::Enabled,
            argv,
            main_module: None,
            cwd,
            // === RUN-001 ===
            env: BTreeMap::new(),
            // === /RUN-001 ===
            deno_node_services: None,
            caps: None,
            user_agent: None,
            sandbox: None,
        }
    }

    pub fn strict_web(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            mode: NodeMode::StrictWeb,
            argv,
            main_module: None,
            cwd,
            // === RUN-001 ===
            env: BTreeMap::new(),
            // === /RUN-001 ===
            deno_node_services: None,
            caps: None,
            user_agent: None,
            sandbox: None,
        }
    }
}

pub fn extensions(opts: NodeOptions) -> Vec<Extension> {
    let NodeOptions {
        mode,
        argv,
        main_module,
        cwd,
        env,
        deno_node_services,
        caps,
        user_agent,
        sandbox,
    } = opts;

    // StrictWeb return removed to enable CJS ops in StrictWeb mode

    // === SEC-001 === derive both enforcement seams from the one policy.
    // Trusted (sandbox = None): keep the pre-SEC-001 allow_all everywhere so
    // `meow run` is byte-identical. Sandboxed: meow's seam gets SandboxCaps and
    // the Node stack (deno_fs/deno_net/deno_process) gets a restrictive
    // PermissionsContainer built from the same policy.
    let caps = match &sandbox {
        Some(policy) => crate::io::sandbox_caps(policy.clone()),
        None => caps.unwrap_or_else(|| Arc::new(crate::io::AllowAll)),
    };
    let user_agent = user_agent.unwrap_or_else(|| format!("meow/{}", env!("CARGO_PKG_VERSION")));

    // One container governs the whole Node stack; `node_permissions_ext` reuses a
    // clone of it (no second, divergent container), so put-order can't matter.
    let parser: Arc<dyn PermissionDescriptorParser> = Arc::new(
        RuntimePermissionDescriptorParser::new(node_bridge::real_node_sys()),
    );
    let perms_container = match &sandbox {
        None => DenoPermissionsContainer::allow_all(parser.clone()),
        Some(policy) => {
            // Fail CLOSED on the (practically impossible) construction error:
            // deny-all breaks the run visibly rather than silently un-sandboxing.
            let perms = Permissions::from_options(&*parser, &sandbox_permissions_options(policy))
                .unwrap_or_else(|_| Permissions::none_without_prompt());
            DenoPermissionsContainer::new(parser.clone(), perms)
        }
    };
    // === /SEC-001 ===
    crate::web::ensure_crypto_provider();

    let blob_store = std::sync::Arc::new(deno_web::BlobStore::default());
    let broadcast_channel = deno_web::InMemoryBroadcastChannel::default();
    let fetch_options = deno_fetch::Options {
        user_agent: user_agent.clone(),
        ..Default::default()
    };

    let (services, fs, native_addon_loader) = match deno_node_services {
        Some(s) => (Some(s.node_ext_init), s.fs.clone(), s.native_addon_loader),
        None => {
            let fs = node_bridge::real_file_system();
            (None, fs, None)
        }
    };

    let mut exts = vec![
        process_exit_state_extension(&env),
        runtime::init(),
        deno_webidl::deno_webidl::init(),
        deno_web::deno_web::init(blob_store, None, false, broadcast_channel),
        deno_crypto::deno_crypto::init(None),
        deno_fetch::deno_fetch::init(fetch_options),
        deno_net::deno_net::init(None, None),
        deno_io::deno_io::init(Some(deno_io::Stdio::default())),
        deno_fs::deno_fs::init(fs.clone()),
        deno_telemetry::deno_telemetry::init(),
        deno_os::deno_os::init(Some(deno_os::ExitCode::default())),
        deno_process::deno_process::init(None),
    ];

    if services.is_some() {
        exts.push(deno_node::deno_node::init(services, fs));
    } else {
        exts.push(deno_node::deno_node::init::<
            node_bridge::DenoNodeBridgeAdapter,
            node_bridge::DenoNodeBridgeAdapter,
            node_bridge::DenoNodeSys,
        >(None, fs));
    }
    exts.push(deno_node_crypto::deno_node_crypto::init());

    exts.push(deno_napi::deno_napi::init(native_addon_loader));

    // Reuse the single container built above (trusted allow_all or the sandbox's
    // restrictive one) rather than a second, always-allow_all instance.
    let node_permissions = perms_container.clone();

    let node_permissions_ext = Extension {
        name: "meow_deno_node_permissions",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            if state.try_borrow::<DenoPermissionsContainer>().is_none() {
                state.put::<DenoPermissionsContainer>(node_permissions.clone());
            }
            if state.try_borrow::<node_bridge::DenoNodeSys>().is_none() {
                state.put::<node_bridge::DenoNodeSys>(sys_traits::impls::RealSys);
            }
        })),
        ..Default::default()
    };

    // === SEC-001 === also seed meow's own Rc capability seam (meow:http serve
    // NetListen + any meow-native io ops) with the sandbox. Folded into this
    // EXISTING, snapshot-ordered extension rather than a new one: deno_core
    // validates the whole extension-name order against the snapshot, so inserting
    // a fresh extension would break snapshot loading. The `SandboxPolicy` is
    // `Send`; the `!Send` `Rc` is built inside the op_state_fn on the runtime
    // thread. `None` (trusted) leaves the default AllowAll seam untouched.
    let io_seam_policy = sandbox.clone();
    exts.push(crate::web::meow_web_fetch::init());
    exts.push(Extension {
        name: "meow_web_fetch_perms",
        op_state_fn: Some(Box::new(move |state| {
            state.put::<DenoPermissionsContainer>(perms_container.clone());
            state.put::<crate::web::NetCaps>(caps.clone());
            if let Some(policy) = &io_seam_policy {
                state.put::<std::rc::Rc<dyn crate::io::CapabilityCheck>>(std::rc::Rc::new(
                    crate::io::SandboxCaps::new(policy.clone()),
                ));
            }
        })),
        ..Default::default()
    });
    exts.push(crate::web::meow_web::init());
    exts.push(node_bootstrap_state_extension(argv, main_module, cwd, env));
    exts.push(node_globals::init());
    let mut withdraw_ext = meow_strict_web_withdraw::init();
    withdraw_ext.op_state_fn = Some(Box::new(move |state: &mut OpState| {
        if matches!(mode, NodeMode::StrictWeb) {
            state.put(StrictWebMarker);
        }
    }));
    exts.push(withdraw_ext);
    exts.push(node_permissions_ext);

    exts
}

// === SEC-001 ===
/// Translate a [`SandboxPolicy`](crate::io::SandboxPolicy) into deno_permissions
/// flags. Deno semantics: `Some(vec![])` = grant ALL, `Some(paths)` = scope to
/// those paths, `None` = deny. Reads/env/sys/import are allowed (env is really
/// governed by the hermetic layer + the baked env map); writes are confined to
/// the policy roots; network, subprocess (`run`) and native FFI are denied — the
/// three ways sandboxed code would otherwise escape.
fn sandbox_permissions_options(policy: &crate::io::SandboxPolicy) -> PermissionsOptions {
    let write = policy
        .write_roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    PermissionsOptions {
        allow_read: Some(Vec::new()),
        allow_write: Some(write),
        allow_net: if policy.allow_net {
            Some(Vec::new())
        } else {
            None
        },
        allow_env: Some(Vec::new()),
        allow_sys: Some(Vec::new()),
        allow_run: None,
        allow_ffi: None,
        allow_import: Some(Vec::new()),
        ..Default::default()
    }
}
// === /SEC-001 ===

fn process_exit_state_extension(env: &BTreeMap<String, String>) -> Extension {
    let child_pipe = child_pipe_from_env(env);
    Extension {
        name: "meow_process_exit_state",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            if state.try_borrow::<crate::ext::ProcessExitCode>().is_none() {
                let exit_code = state
                    .try_borrow::<deno_os::ExitCode>()
                    .cloned()
                    .unwrap_or_default();
                state.put(crate::ext::ProcessExitCode::new(exit_code));
            }
            state.put::<deno_node::ops::handle_wrap::AsyncId>(
                deno_node::ops::handle_wrap::AsyncId::default(),
            );
            if let Some(child_pipe) = child_pipe {
                state.put(child_pipe);
            }
        })),
        ..Default::default()
    }
}

#[derive(Clone)]
struct NodeBootstrapState {
    argv: Vec<String>,
    main_module: Option<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeBootstrapInfo {
    args: Vec<String>,
    argv: Vec<String>,
    cwd: String,
    main_module: Option<String>,
    exec_path: Option<String>,
    env: BTreeMap<String, String>,
    pid: u32,
    ppid: u32,
}

#[cfg(unix)]
fn parent_process_id() -> u32 {
    unsafe { libc::getppid() as u32 }
}

#[cfg(windows)]
fn parent_process_id() -> u32 {
    0
}

#[cfg(not(any(unix, windows)))]
fn parent_process_id() -> u32 {
    0
}

#[op2]
#[serde]
fn op_meow_node_bootstrap_info(state: &mut OpState) -> NodeBootstrapInfo {
    let bootstrap = state.borrow::<NodeBootstrapState>();
    NodeBootstrapInfo {
        args: bootstrap.argv.iter().skip(1).cloned().collect(),
        argv: bootstrap.argv.clone(),
        cwd: bootstrap.cwd.to_string_lossy().into_owned(),
        main_module: bootstrap.main_module.clone(),
        exec_path: bootstrap
            .env
            .get("NODE")
            .or_else(|| bootstrap.env.get("npm_node_execpath"))
            .cloned(),
        env: bootstrap.env.clone(),
        pid: std::process::id(),
        ppid: parent_process_id(),
    }
}

fn node_bootstrap_state_extension(
    argv: Vec<String>,
    main_module: Option<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
) -> Extension {
    let mut ext = meow_node_bootstrap::init();
    ext.op_state_fn = Some(Box::new(move |state: &mut OpState| {
        state.put(NodeBootstrapState {
            argv: argv.clone(),
            main_module: main_module.clone(),
            cwd: cwd.clone(),
            env: env.clone(),
        });
    }));
    ext
}

deno_core::extension!(meow_node_bootstrap, ops = [op_meow_node_bootstrap_info],);

fn child_pipe_from_env(env: &BTreeMap<String, String>) -> Option<deno_node::ChildPipeFd> {
    let fd = env.get("NODE_CHANNEL_FD")?.parse().ok()?;
    let serialization = env
        .get("NODE_CHANNEL_SERIALIZATION_MODE")
        .and_then(|raw| deno_node::ops::ipc::ChildIpcSerialization::from_str(raw).ok())
        .unwrap_or(deno_node::ops::ipc::ChildIpcSerialization::Json);
    Some(deno_node::ChildPipeFd(fd, serialization))
}

pub fn take_process_exit_code(js_runtime: &JsRuntime) -> Option<i32> {
    let op_state = js_runtime.op_state();
    let state = op_state.borrow();
    state
        .try_borrow::<crate::ext::ProcessExitCode>()
        .and_then(|exit_code| exit_code.take())
}
/// Refresh the Node bootstrap state (argv, cwd, env) in the runtime's OpState
/// and update the JS `process` global to match.
/// Must be called after loading from a snapshot, since the snapshot bakes in
/// the bootstrap state from snapshot-creation time.
pub fn refresh_bootstrap_state(
    js_runtime: &mut deno_core::JsRuntime,
    argv: Vec<String>,
    main_module: Option<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
) -> Result<(), crate::RuntimeError> {
    // Seed the real per-invocation bootstrap state so op_meow_node_bootstrap_info
    // returns fresh values, then run the genuine Node process bootstrap against it.
    {
        let op_state = js_runtime.op_state();
        let mut state = op_state.borrow_mut();
        let child_pipe = child_pipe_from_env(&env);
        state.put(NodeBootstrapState {
            argv,
            main_module,
            cwd,
            env,
        });
        if let Some(child_pipe) = child_pipe {
            state.put(child_pipe);
        }
    }
    // The snapshot's module bodies ran at snapshot-build time with placeholder
    // argv/cwd/env and only WARMED the Node bootstrap (warmup:true), leaving
    // __bootstrapNodeProcess installed. Re-run it now with the real state: this sets
    // process.argv/execPath/cwd, wires child IPC (process.send), and registers
    // streamBaseState.
    js_runtime
        .execute_script(
            "ext:meow_runtime/runtime_bootstrap.js",
            "globalThis.__meowRuntimeBootstrap && globalThis.__meowRuntimeBootstrap();",
        )
        .map_err(|err| {
            crate::RuntimeError::Init(format!("runtime node bootstrap failed: {err}"))
        })?;
    Ok(())
}
deno_core::extension!(
    runtime,
    esm_entry_point = "ext:runtime/98_global_scope_shared.js",
    esm = [dir "src/js", "98_global_scope_shared.js"],
);

deno_core::extension!(
    node_globals,
    esm_entry_point = "ext:node_globals/node_globals.js",
    esm = [dir "src/js", "node_globals.js"],
);

// === RT-004 / RT-007 ===
// StrictWeb host-access withdrawal: node:fs / node:process operations throw
// ERR_STRICT_WEB_WITHDRAWN at use time and the ambient `process` global is
// removed. Pushed ONLY in NodeMode::StrictWeb, after node_globals, so it tears
// down the Node host surface the rest of the stack just wired up.
struct StrictWebMarker;

#[op2(fast)]
fn op_is_strict_web(state: &mut OpState) -> bool {
    state.has::<StrictWebMarker>()
}

deno_core::extension!(
    meow_strict_web_withdraw,
    ops = [op_is_strict_web],
    esm_entry_point = "ext:meow_strict_web_withdraw/strict_web_withdraw.js",
    esm = [dir "src/js", "strict_web_withdraw.js"],
);
