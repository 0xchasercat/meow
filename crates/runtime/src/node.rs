//! Deno-owned Node compatibility runtime wiring (RT-007).
//!
//! Node built-ins, CommonJS, and N-API are delegated to upstream Deno crates.
//! `meow-runtime` only assembles those extensions around services supplied by
//! the binary edge; it does not depend on `meow-loader` and does not own
//! JavaScript `node:*` polyfills.

#[path = "node_bridge.rs"]
pub mod node_bridge;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use deno_core::{op2, Extension, JsRuntime, OpState};
use deno_permissions::{
    PermissionsContainer as DenoPermissionsContainer, RuntimePermissionDescriptorParser,
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
}

impl NodeOptions {
    pub fn enabled(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            mode: NodeMode::Enabled,
            argv,
            cwd,
            // === RUN-001 ===
            env: BTreeMap::new(),
            // === /RUN-001 ===
            deno_node_services: None,
            caps: None,
            user_agent: None,
        }
    }

    pub fn strict_web(argv: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            mode: NodeMode::StrictWeb,
            argv,
            cwd,
            // === RUN-001 ===
            env: BTreeMap::new(),
            // === /RUN-001 ===
            deno_node_services: None,
            caps: None,
            user_agent: None,
        }
    }
}

pub fn extensions(opts: NodeOptions) -> Vec<Extension> {
    let NodeOptions {
        mode,
        argv,
        cwd,
        env,
        deno_node_services,
        caps,
        user_agent,
    } = opts;

    // StrictWeb return removed to enable CJS ops in StrictWeb mode

    let caps = caps.unwrap_or_else(|| Arc::new(crate::io::AllowAll));
    let user_agent = user_agent.unwrap_or_else(|| format!("meow/{}", env!("CARGO_PKG_VERSION")));

    let parser = Arc::new(RuntimePermissionDescriptorParser::new(
        node_bridge::real_node_sys(),
    ));
    let perms_container = DenoPermissionsContainer::allow_all(parser);
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
        process_exit_state_extension(),
        runtime::init(),
        deno_webidl::deno_webidl::init(),
        deno_web::deno_web::init(blob_store, None, false, broadcast_channel),
        deno_crypto::deno_crypto::init(None),
        deno_fetch::deno_fetch::init(fetch_options),
        deno_net::deno_net::init(None, None),
        deno_io::deno_io::init(Some(deno_io::Stdio::default())),
        deno_fs::deno_fs::init(fs.clone()),
        deno_telemetry::deno_telemetry::init(),
        deno_os::deno_os::init(None),
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

    let parser = Arc::new(RuntimePermissionDescriptorParser::new(
        node_bridge::real_node_sys(),
    ));
    let node_permissions = DenoPermissionsContainer::allow_all(parser);

    let node_permissions_ext = Extension {
        name: "meow_deno_node_permissions",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            if state.try_borrow::<DenoPermissionsContainer>().is_none() {
                state.put::<DenoPermissionsContainer>(node_permissions.clone());
            }
        })),
        ..Default::default()
    };

    exts.push(crate::web::meow_web_fetch::init());
    exts.push(Extension {
        name: "meow_web_fetch_perms",
        op_state_fn: Some(Box::new(move |state| {
            state.put::<DenoPermissionsContainer>(perms_container.clone());
            state.put::<crate::web::NetCaps>(caps.clone());
        })),
        ..Default::default()
    });
    exts.push(crate::web::meow_web::init());
    exts.push(node_bootstrap_state_extension(argv, cwd, env));
    exts.push(node_globals::init());
    if matches!(mode, NodeMode::StrictWeb) {
        exts.push(meow_strict_web_withdraw::init());
    }
    exts.push(node_permissions_ext);

    exts
}

fn process_exit_state_extension() -> Extension {
    let cell = crate::ext::ProcessExitCell(Rc::new(RefCell::new(None)));
    let child_pipe = child_pipe_from_env();
    Extension {
        name: "meow_process_exit_state",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            state.put(cell.clone());
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
    env: BTreeMap<String, String>,
}

#[op2]
#[serde]
fn op_meow_node_bootstrap_info(state: &mut OpState) -> NodeBootstrapInfo {
    let bootstrap = state.borrow::<NodeBootstrapState>();
    let main_module = bootstrap.argv.get(1).and_then(|arg| {
        let path = PathBuf::from(arg);
        let path = if path.is_absolute() {
            path
        } else {
            bootstrap.cwd.join(path)
        };
        deno_core::ModuleSpecifier::from_file_path(path)
            .ok()
            .map(|specifier| specifier.to_string())
    });

    NodeBootstrapInfo {
        args: bootstrap.argv.iter().skip(2).cloned().collect(),
        argv: bootstrap.argv.clone(),
        cwd: bootstrap.cwd.to_string_lossy().into_owned(),
        main_module,
        env: bootstrap.env.clone(),
    }
}

fn node_bootstrap_state_extension(
    argv: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
) -> Extension {
    let mut ext = meow_node_bootstrap::init();
    ext.op_state_fn = Some(Box::new(move |state: &mut OpState| {
        state.put(NodeBootstrapState {
            argv: argv.clone(),
            cwd: cwd.clone(),
            env: env.clone(),
        });
    }));
    ext
}

deno_core::extension!(meow_node_bootstrap, ops = [op_meow_node_bootstrap_info],);

fn child_pipe_from_env() -> Option<deno_node::ChildPipeFd> {
    let fd = std::env::var("NODE_CHANNEL_FD").ok()?.parse().ok()?;
    let serialization = std::env::var("NODE_CHANNEL_SERIALIZATION_MODE")
        .ok()
        .and_then(|raw| deno_node::ops::ipc::ChildIpcSerialization::from_str(&raw).ok())
        .unwrap_or(deno_node::ops::ipc::ChildIpcSerialization::Json);
    Some(deno_node::ChildPipeFd(fd, serialization))
}

pub fn take_process_exit_code(js_runtime: &JsRuntime) -> Option<i32> {
    let op_state = js_runtime.op_state();
    let state = op_state.borrow();
    state
        .try_borrow::<crate::ext::ProcessExitCell>()
        .and_then(|cell| cell.0.borrow_mut().take())
}
/// Refresh the Node bootstrap state (argv, cwd, env) in the runtime's OpState
/// and update the JS `process` global to match.
/// Must be called after loading from a snapshot, since the snapshot bakes in
/// the bootstrap state from snapshot-creation time.
pub fn refresh_bootstrap_state(
    js_runtime: &mut deno_core::JsRuntime,
    argv: Vec<String>,
    cwd: PathBuf,
    env: BTreeMap<String, String>,
) {
    // Update the OpState so ops like op_meow_node_bootstrap_info return fresh values.
    {
        let op_state = js_runtime.op_state();
        let mut state = op_state.borrow_mut();
        state.put(NodeBootstrapState {
            argv: argv.clone(),
            cwd: cwd.clone(),
            env: env.clone(),
        });
    }
    // Note: we do NOT patch process.argv or process.cwd here.
    // The snapshot captures these at snapshot-creation time (argv=["meow", "snapshot-placeholder"], cwd="/").
    // Patching via execute_script doesn't reliably reach the ESM module scope.
    // The argv limitation is acceptable; cwd returns "/" which is a valid fallback.
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
deno_core::extension!(
    meow_strict_web_withdraw,
    esm_entry_point = "ext:meow_strict_web_withdraw/strict_web_withdraw.js",
    esm = [dir "src/js", "strict_web_withdraw.js"],
);
