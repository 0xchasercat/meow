//! `fetch` capability gate + the permission glue deno_fetch requires (RT-004 · A3).
//!
//! deno_fetch (this pin) reads a concrete [`deno_permissions::PermissionsContainer`]
//! out of `OpState` and resolves hosts through a pluggable [`deno_fetch::dns::Resolve`]
//! DNS resolver. There is no per-request permission *trait* to implement anymore
//! (the upstream model became concrete since the spec's `[INFERENCE]` sketch). We
//! therefore wire meow's capability seam at the resolver: every hostname `fetch`
//! resolves is checked against the RT-002 [`CapabilityCheck`] BEFORE any socket is
//! opened (the resolver runs ahead of the connector). A denied host fails
//! resolution → the `fetch` promise rejects with a `TypeError`, never a panic.
//!
//! Honest boundaries (Operator notes, I-11):
//! - The bytes flow through deno_fetch's own hyper/rustls client — NOT RT-002's
//!   `op_tcp_connect` transport. The seam is the *authorization* gate, not the
//!   transport.
//! - deno_fetch's own [`PermissionsContainer`] is set to `allow_all` so it never
//!   second-guesses meow's gate; meow's [`CapabilityCheck`] is the single network
//!   authority. At P1 that seam defaults to `AllowAll` (seam, not enforcement —
//!   SEC-001/P6); `fetch` is NOT sandboxed.
//! - hyper-util's connector resolves IP-literal hosts (e.g. `127.0.0.1`) without
//!   consulting a DNS resolver, so the gate fires for named hosts; tiered,
//!   complete enforcement (incl. literals) is SEC-001/P6.

use std::borrow::Cow;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::Path;

use deno_permissions::{
    AllowRunDescriptorParseResult, DenyRunDescriptor, EnvDescriptor, EnvDescriptorParseError,
    FfiDescriptor, ImportDescriptor, NetDescriptor, NetDescriptorParseError, PathQueryDescriptor,
    PathResolveError, PermissionDescriptorParser, ReadDescriptor, RunDescriptorParseError,
    RunQueryDescriptor, SpecialFilePathQueryDescriptor, SysDescriptor, SysDescriptorParseError,
    WriteDescriptor,
};
use hyper_util::client::legacy::connect::dns::Name;

use crate::io::CapRequest;

use super::NetCaps;

/// deno_fetch DNS resolver that consults meow's capability seam before resolving
/// (hence before connecting). On allow it performs ordinary name resolution; on
/// deny it returns a permission error so `fetch` rejects without opening a socket.
pub struct CapResolver {
    caps: NetCaps,
}

impl CapResolver {
    pub fn new(caps: NetCaps) -> Self {
        Self { caps }
    }
}

impl std::fmt::Debug for CapResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CapResolver")
    }
}

impl deno_fetch::dns::Resolve for CapResolver {
    fn resolve(&self, name: Name) -> deno_fetch::dns::Resolving {
        let host = name.as_str().to_string();
        let caps = self.caps.clone();
        Box::pin(async move {
            // Authorization gate FIRST — before any socket (I-6 governed entry).
            caps.check(&CapRequest::NetConnect(&host))
                .map_err(|denied| io::Error::new(io::ErrorKind::PermissionDenied, denied.0))?;
            // getaddrinfo is blocking; keep it off the event-loop thread.
            let resolved = tokio::task::spawn_blocking(move || {
                (host.as_str(), 0u16)
                    .to_socket_addrs()
                    .map(|it| it.collect::<Vec<SocketAddr>>())
            })
            .await
            .map_err(io::Error::other)??;
            Ok(resolved.into_iter())
        })
    }
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
