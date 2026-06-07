//! `fetch` capability gate + the permission glue deno_fetch requires (RT-004 · A3).
//!
//! `fetch` is the one host-touching global, so it routes through meow's single
//! network seam — RT-002's [`CapabilityCheck`], reusing its existing
//! [`CapRequest::NetConnect`] variant. The gate fires at the `fetch` *entry*: the
//! committed `fetch` global is a thin JS wrapper (see `js/bootstrap.js`) that, for
//! `http(s)` requests, parses the URL and calls [`op_meow_fetch_check`] with the
//! connect target BEFORE the request op runs — i.e. before any socket. A denial
//! rejects the `fetch` promise with a `TypeError`, never a Rust panic.
//!
//! The target carries the full **host:port** (e.g. `example.com:8443`), matching
//! RT-002's `op_tcp_connect` — a policy can distinguish `:80` from `:8443`, not
//! just the bare host.
//!
//! Honest boundaries (Operator notes, I-11):
//! - The bytes flow through deno_fetch's own hyper/rustls client — NOT RT-002's
//!   `op_tcp_connect` transport. The seam is the *authorization* gate, not the
//!   transport.
//! - deno_fetch's own [`PermissionsContainer`] is set to `allow_all` so it never
//!   second-guesses meow's gate; meow's [`CapabilityCheck`] is the single network
//!   authority. At P1 that seam defaults to `AllowAll` (seam, not enforcement —
//!   SEC-001/P6); `fetch` is NOT sandboxed.
//! - IP-literal hosts (e.g. `127.0.0.1`, `[::1]`) bypass the gate, unchanged from
//!   the prior pre-DNS gate: hyper-util resolves literals without DNS, so meow's
//!   pre-connection seam never saw them. Tiered, complete enforcement (incl.
//!   literals, and the raw-op path the committed wrapper fronts) is SEC-001/P6.

use std::borrow::Cow;
use std::io;
use std::net::IpAddr;
use std::path::Path;

use deno_core::{op2, OpState};
use deno_permissions::{
    AllowRunDescriptorParseResult, DenyRunDescriptor, EnvDescriptor, EnvDescriptorParseError,
    FfiDescriptor, ImportDescriptor, NetDescriptor, NetDescriptorParseError, PathQueryDescriptor,
    PathResolveError, PermissionDescriptorParser, ReadDescriptor, RunDescriptorParseError,
    RunQueryDescriptor, SpecialFilePathQueryDescriptor, SysDescriptor, SysDescriptorParseError,
    WriteDescriptor,
};

use crate::io::{CapDenied, CapRequest};

use super::NetCaps;

/// Typed failure when the capability seam refuses a `fetch` connect target.
/// Implements `deno_error::JsError` so it crosses into JS as a thrown `TypeError`
/// (the WHATWG fetch network-error class) — a rejected promise, never a panic.
#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum FetchAuthError {
    /// The network capability denied the connect target (host:port).
    #[class(type)]
    #[error("{0}")]
    Denied(#[from] CapDenied),
}

/// Authorize a `fetch` connect target through meow's network seam. `host` is the
/// URL hostname (possibly `[..]`-bracketed for IPv6) and `port` the resolved
/// destination port (URL port or the scheme default). IP-literal hosts bypass the
/// gate (see module docs); every named host is checked as the full `host:port`
/// target, allocating only on the checked path.
pub(crate) fn authorize_fetch(
    state: &OpState,
    host: &str,
    port: u16,
) -> Result<(), FetchAuthError> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if bare.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let target = format!("{host}:{port}");
    state
        .borrow::<NetCaps>()
        .check(&CapRequest::NetConnect(&target))
        .map_err(FetchAuthError::Denied)
}

/// Sync op the committed `fetch` wrapper calls before the request op runs. Throws
/// (rejecting the `fetch` promise) iff the seam denies the host:port target.
#[op2(fast)]
pub fn op_meow_fetch_check(
    state: &mut OpState,
    #[string] host: &str,
    port: u16,
) -> Result<(), FetchAuthError> {
    authorize_fetch(state, host, port)
}

/// A no-op [`PermissionDescriptorParser`]. deno_fetch's `PermissionsContainer`
/// constructor requires a descriptor parser, but with `allow_all` net permission
/// the parser is never consulted on the `fetch` path (the allow-all check
/// short-circuits before parsing), and meow exposes no `Deno` namespace that would
/// reach the other descriptors. Every method therefore returns a typed error
/// rather than panicking — defensive, not load-bearing.
#[derive(Debug)]
pub struct NoopDescriptorParser;

impl NoopDescriptorParser {
    fn unsupported() -> PathResolveError {
        PathResolveError::CwdResolve(io::Error::new(
            io::ErrorKind::Unsupported,
            "meow: permission descriptors are gated by meow's capability seam, not deno_permissions",
        ))
    }
}

impl PermissionDescriptorParser for NoopDescriptorParser {
    fn parse_read_descriptor(&self, _t: &str) -> Result<ReadDescriptor, PathResolveError> {
        Err(Self::unsupported())
    }
    fn parse_write_descriptor(&self, _t: &str) -> Result<WriteDescriptor, PathResolveError> {
        Err(Self::unsupported())
    }
    fn parse_net_descriptor(&self, t: &str) -> Result<NetDescriptor, NetDescriptorParseError> {
        Err(NetDescriptorParseError::Url(t.to_string()))
    }
    fn parse_import_descriptor(
        &self,
        t: &str,
    ) -> Result<ImportDescriptor, NetDescriptorParseError> {
        Err(NetDescriptorParseError::Url(t.to_string()))
    }
    fn parse_env_descriptor(&self, _t: &str) -> Result<EnvDescriptor, EnvDescriptorParseError> {
        Err(EnvDescriptorParseError)
    }
    fn parse_sys_descriptor(&self, _t: &str) -> Result<SysDescriptor, SysDescriptorParseError> {
        Err(SysDescriptorParseError::Empty)
    }
    fn parse_allow_run_descriptor(
        &self,
        _t: &str,
    ) -> Result<AllowRunDescriptorParseResult, RunDescriptorParseError> {
        Err(RunDescriptorParseError::EmptyRunQuery)
    }
    fn parse_deny_run_descriptor(&self, _t: &str) -> Result<DenyRunDescriptor, PathResolveError> {
        Err(Self::unsupported())
    }
    fn parse_ffi_descriptor(&self, _t: &str) -> Result<FfiDescriptor, PathResolveError> {
        Err(Self::unsupported())
    }
    fn parse_path_query<'a>(
        &self,
        _path: Cow<'a, Path>,
    ) -> Result<PathQueryDescriptor<'a>, PathResolveError> {
        Err(Self::unsupported())
    }
    fn parse_special_file_descriptor<'a>(
        &self,
        _path: PathQueryDescriptor<'a>,
    ) -> Result<SpecialFilePathQueryDescriptor<'a>, PathResolveError> {
        Err(Self::unsupported())
    }
    fn parse_net_query(&self, t: &str) -> Result<NetDescriptor, NetDescriptorParseError> {
        Err(NetDescriptorParseError::Url(t.to_string()))
    }
    fn parse_run_query<'a>(
        &self,
        _requested: &'a str,
    ) -> Result<RunQueryDescriptor<'a>, RunDescriptorParseError> {
        Err(RunDescriptorParseError::EmptyRunQuery)
    }
}
