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

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{op2, JsRuntime, OpState};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

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

#[derive(Clone)]
pub struct NodeOptions {
    pub mode: NodeMode,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    // === RUN-001 ===
    pub env: BTreeMap<String, String>,
    // === /RUN-001 ===
    pub cjs_resolver: Option<Arc<dyn CjsResolver>>,
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
            cjs_resolver: None,
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
            cjs_resolver: None,
        }
    }
}

#[derive(Clone)]
struct NodeRuntimeState {
    mode: NodeMode,
    argv: Vec<String>,
    cwd: String,
    // === RUN-001 ===
    env: Vec<(String, String)>,
    // === /RUN-001 ===
    cjs_resolver: Option<Arc<dyn CjsResolver>>,
}

impl NodeRuntimeState {
    fn new(opts: NodeOptions) -> Self {
        Self {
            mode: opts.mode,
            argv: opts.argv,
            cwd: opts.cwd.to_string_lossy().into_owned(),
            // === RUN-001 ===
            env: opts.env.into_iter().collect(),
            // === /RUN-001 ===
            cjs_resolver: opts.cjs_resolver,
        }
    }

    fn fallback() -> Self {
        Self {
            mode: NodeMode::StrictWeb,
            argv: Vec::new(),
            cwd: ".".to_owned(),
            // === RUN-001 ===
            env: Vec::new(),
            // === /RUN-001 ===
            cjs_resolver: None,
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
    // === RUN-001 ===
    env: Vec<(String, String)>,
    // === /RUN-001 ===
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChildProcessOptions {
    cwd: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    shell: bool,
    #[serde(default)]
    stdio: Option<String>,
    #[serde(default)]
    input: Option<Vec<u8>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChildProcessOutput {
    status: Option<i32>,
    signal: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
struct DnsLookupAddress {
    address: String,
    family: u8,
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
enum ChildProcessError {
    #[error("strict-web mode withdraws node:child_process")]
    StrictWeb,
    #[error("child process command cannot be empty")]
    EmptyCommand,
    #[error("child process failed: {0}")]
    Io(String),
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
enum CryptoError {
    #[error("node:crypto unsupported digest algorithm: {0}")]
    UnsupportedAlgorithm(String),
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
enum DnsLookupError {
    #[error("ERR_STRICT_WEB_WITHDRAWN: strict-web mode withdraws node:dns")]
    StrictWeb,
    #[error("ERR_INVALID_ARG_VALUE: dns.lookup() family must be 0, 4, or 6")]
    InvalidFamily,
    #[error("ENOTFOUND: getaddrinfo ENOTFOUND {0}")]
    NotFound(String),
    #[error("ENOTFOUND: getaddrinfo ENOTFOUND {hostname}: {source}")]
    Lookup {
        hostname: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CjsLoadedModule {
    pub url: String,
    pub filename: String,
    pub dirname: String,
    pub source: String,
    pub kind: String,
}

pub trait CjsResolver: Send + Sync {
    fn resolve_and_load_cjs(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Result<CjsLoadedModule, String>;
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(generic)]
enum CjsLoadError {
    #[error("meow: CommonJS resolver is not installed")]
    ResolverUnavailable,
    #[error("meow: {0}")]
    Resolve(String),
}

fn canonical_digest_algorithm(algorithm: &str) -> Option<&'static str> {
    let trimmed = algorithm.trim();
    if trimmed.eq_ignore_ascii_case("md5") {
        Some("md5")
    } else if trimmed.eq_ignore_ascii_case("sha1") || trimmed.eq_ignore_ascii_case("sha-1") {
        Some("sha1")
    } else if trimmed.eq_ignore_ascii_case("sha256") || trimmed.eq_ignore_ascii_case("sha-256") {
        Some("sha256")
    } else if trimmed.eq_ignore_ascii_case("sha384") || trimmed.eq_ignore_ascii_case("sha-384") {
        Some("sha384")
    } else if trimmed.eq_ignore_ascii_case("sha512") || trimmed.eq_ignore_ascii_case("sha-512") {
        Some("sha512")
    } else {
        None
    }
}

fn hash_digest<D: Digest>(data: &[u8]) -> Vec<u8> {
    let mut hasher = D::new();
    Digest::update(&mut hasher, data);
    hasher.finalize().to_vec()
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

fn ensure_node_process_allowed(state: &OpState) -> Result<NodeRuntimeState, ChildProcessError> {
    let st = node_state(state);
    if !st.mode.globals_enabled() {
        return Err(ChildProcessError::StrictWeb);
    }
    Ok(st)
}

fn ensure_node_dns_allowed(state: &OpState) -> Result<(), DnsLookupError> {
    if !node_state(state).mode.globals_enabled() {
        return Err(DnsLookupError::StrictWeb);
    }
    Ok(())
}

fn socket_addr_family(addr: &SocketAddr) -> u8 {
    if addr.is_ipv4() {
        4
    } else {
        6
    }
}

fn dns_lookup_address(addr: SocketAddr) -> DnsLookupAddress {
    DnsLookupAddress {
        address: addr.ip().to_string(),
        family: socket_addr_family(&addr),
    }
}

fn family_matches(addr: &SocketAddr, family: i32) -> bool {
    family == 0 || i32::from(socket_addr_family(addr)) == family
}

fn build_command(
    st: &NodeRuntimeState,
    command: &str,
    args: Vec<String>,
    options: ChildProcessOptions,
) -> Result<Command, ChildProcessError> {
    if command.is_empty() {
        return Err(ChildProcessError::EmptyCommand);
    }
    let mut cmd = if options.shell {
        shell_command(command, args)
    } else {
        let mut cmd = Command::new(command);
        cmd.args(args);
        cmd
    };
    cmd.current_dir(options.cwd.unwrap_or_else(|| st.cwd.clone()));
    for (key, value) in &st.env {
        cmd.env(key, value);
    }
    for (key, value) in options.env {
        cmd.env(key, value);
    }
    if options.stdio.as_deref() == Some("inherit") {
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
    }
    Ok(cmd)
}

fn shell_command(command: &str, args: Vec<String>) -> Command {
    let script = if args.is_empty() {
        command.to_owned()
    } else {
        format!("{command} {}", args.join(" "))
    };
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/D", "/S", "/C", &script]);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &script]);
        cmd
    }
}

fn run_child_process(
    st: NodeRuntimeState,
    command: String,
    args: Vec<String>,
    options: ChildProcessOptions,
) -> Result<ChildProcessOutput, ChildProcessError> {
    let inherit = options.stdio.as_deref() == Some("inherit");
    let input = options.input.clone();
    let mut cmd = build_command(&st, &command, args, options)?;
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    if inherit {
        let status = cmd
            .status()
            .map_err(|err| ChildProcessError::Io(err.to_string()))?;
        return Ok(ChildProcessOutput {
            status: status.code(),
            signal: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
    }
    if let Some(input) = input {
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| ChildProcessError::Io(err.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(&input)
                .map_err(|err| ChildProcessError::Io(err.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|err| ChildProcessError::Io(err.to_string()))?;
        return Ok(ChildProcessOutput {
            status: output.status.code(),
            signal: None,
            stdout: output.stdout,
            stderr: output.stderr,
        });
    }
    let output = cmd
        .output()
        .map_err(|err| ChildProcessError::Io(err.to_string()))?;
    Ok(ChildProcessOutput {
        status: output.status.code(),
        signal: None,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[op2]
#[serde]
fn op_node_child_process_sync(
    state: &mut OpState,
    #[string] command: String,
    #[serde] args: Vec<String>,
    #[serde] options: ChildProcessOptions,
) -> Result<ChildProcessOutput, ChildProcessError> {
    let st = ensure_node_process_allowed(state)?;
    run_child_process(st, command, args, options)
}

#[op2]
#[serde]
async fn op_node_child_process(
    state: Rc<RefCell<OpState>>,
    #[string] command: String,
    #[serde] args: Vec<String>,
    #[serde] options: ChildProcessOptions,
) -> Result<ChildProcessOutput, ChildProcessError> {
    let st = {
        let state = state.borrow();
        ensure_node_process_allowed(&state)?
    };
    tokio::task::spawn_blocking(move || run_child_process(st, command, args, options))
        .await
        .map_err(|err| ChildProcessError::Io(err.to_string()))?
}

#[op2]
#[serde]
async fn op_node_dns_lookup(
    state: Rc<RefCell<OpState>>,
    #[string] hostname: String,
    family: i32,
    all: bool,
) -> Result<Vec<DnsLookupAddress>, DnsLookupError> {
    {
        let state = state.borrow();
        ensure_node_dns_allowed(&state)?;
    }
    if family != 0 && family != 4 && family != 6 {
        return Err(DnsLookupError::InvalidFamily);
    }
    let addrs = tokio::net::lookup_host((hostname.as_str(), 0))
        .await
        .map_err(|source| DnsLookupError::Lookup {
            hostname: hostname.clone(),
            source,
        })?;
    let mut out = Vec::new();
    for addr in addrs {
        if family_matches(&addr, family) {
            out.push(dns_lookup_address(addr));
            if !all {
                break;
            }
        }
    }
    if out.is_empty() {
        return Err(DnsLookupError::NotFound(hostname));
    }
    Ok(out)
}

#[op2]
#[serde]
fn op_node_crypto_hash(
    #[string] algorithm: String,
    #[serde] data: Vec<u8>,
) -> Result<Vec<u8>, CryptoError> {
    let canonical = canonical_digest_algorithm(&algorithm)
        .ok_or_else(|| CryptoError::UnsupportedAlgorithm(algorithm))?;
    Ok(match canonical {
        "md5" => hash_digest::<md5::Md5>(&data),
        "sha1" => hash_digest::<Sha1>(&data),
        "sha256" => hash_digest::<Sha256>(&data),
        "sha384" => hash_digest::<Sha384>(&data),
        "sha512" => hash_digest::<Sha512>(&data),
        _ => unreachable!("canonical digest set is closed"),
    })
}

#[op2]
#[serde]
fn op_node_crypto_hmac(
    #[string] algorithm: String,
    #[serde] key: Vec<u8>,
    #[serde] data: Vec<u8>,
) -> Result<Vec<u8>, CryptoError> {
    let canonical = canonical_digest_algorithm(&algorithm)
        .ok_or_else(|| CryptoError::UnsupportedAlgorithm(algorithm))?;
    macro_rules! hmac_digest {
        ($digest:ty) => {{
            let mut mac =
                Hmac::<$digest>::new_from_slice(&key).expect("HMAC accepts any key length");
            Mac::update(&mut mac, &data);
            mac.finalize().into_bytes().to_vec()
        }};
    }

    Ok(match canonical {
        "md5" => hmac_digest!(md5::Md5),
        "sha1" => hmac_digest!(Sha1),
        "sha256" => hmac_digest!(Sha256),
        "sha384" => hmac_digest!(Sha384),
        "sha512" => hmac_digest!(Sha512),
        _ => unreachable!("canonical digest set is closed"),
    })
}
#[op2]
#[serde]
fn op_cjs_resolve_and_load(
    state: &mut OpState,
    #[string] specifier: String,
    #[string] referrer: String,
) -> Result<CjsLoadedModule, CjsLoadError> {
    let st = node_state(state);
    let resolver = st.cjs_resolver.ok_or(CjsLoadError::ResolverUnavailable)?;
    resolver
        .resolve_and_load_cjs(&specifier, &referrer)
        .map_err(CjsLoadError::Resolve)
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
        // === RUN-001 ===
        env: st.env,
        // === /RUN-001 ===
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
    ops = [
        op_node_process_info,
        op_node_process_exit,
        op_node_child_process_sync,
        op_node_child_process,
        op_node_crypto_hash,
        op_node_crypto_hmac,
        op_node_dns_lookup,
        op_cjs_resolve_and_load
    ],
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
