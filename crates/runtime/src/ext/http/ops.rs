use std::borrow::Cow;
use std::cell::RefCell;
use std::net::SocketAddr;
use std::rc::Rc;

use deno_core::{op2, AsyncRefCell, JsBuffer, OpState, RcRef, Resource, ResourceId};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ServerBuilder;
use hyper_util::server::graceful::GracefulShutdown;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::io::{AllowAll, CapRequest, CapabilityCheck};

#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum HttpError {
    #[class(generic)]
    #[error("network capability is required to bind {addr} — add \"net\" to permissions in meow.config.ts")]
    Denied { addr: String },
    #[class(inherit)]
    #[error("could not bind {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        #[inherit]
        source: std::io::Error,
    },
    #[class(type)]
    #[error("invalid bind address {addr:?}")]
    BadAddr { addr: String },
    #[class(type)]
    #[error("http server resource {rid} is gone")]
    NoServer { rid: u32 },
    #[class(type)]
    #[error("http response slot {slot} is gone")]
    NoSlot { slot: u32 },
    #[class(generic)]
    #[error("http response slot {slot} closed before the handler replied")]
    ResponseClosed { slot: u32 },
    #[class(type)]
    #[error("invalid HTTP status code {status}")]
    BadStatus { status: u16 },
    #[class(type)]
    #[error("invalid response header {name:?}: {value:?}")]
    BadHeader { name: String, value: String },
    #[class(generic)]
    #[error("the meow:http accept loop failed: {message}")]
    Task { message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct NetAddr {
    pub hostname: String,
    pub port: u16,
}

impl From<SocketAddr> for NetAddr {
    fn from(addr: SocketAddr) -> Self {
        NetAddr {
            hostname: addr.ip().to_string(),
            port: addr.port(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ServeHandle {
    pub rid: ResourceId,
    pub addr: NetAddr,
}

#[derive(Debug, Serialize)]
pub struct IncomingRequest {
    pub slot: ResourceId,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

struct QueuedRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    response: oneshot::Sender<WireResponse>,
}

struct WireResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl WireResponse {
    fn into_hyper(self) -> Result<Response<Full<Bytes>>, HttpError> {
        let status = StatusCode::from_u16(self.status).map_err(|_| HttpError::BadStatus {
            status: self.status,
        })?;
        let mut response = Response::builder().status(status);
        for (name, value) in self.headers {
            let header_name =
                hyper::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                    HttpError::BadHeader {
                        name: name.clone(),
                        value: value.clone(),
                    }
                })?;
            let header_value =
                hyper::header::HeaderValue::from_str(&value).map_err(|_| HttpError::BadHeader {
                    name: name.clone(),
                    value: value.clone(),
                })?;
            response = response.header(header_name, header_value);
        }
        response
            .body(Full::new(Bytes::from(self.body)))
            .map_err(|err| HttpError::Task {
                message: err.to_string(),
            })
    }
}

pub struct ServerResource {
    requests: AsyncRefCell<mpsc::Receiver<QueuedRequest>>,
    shutdown: watch::Sender<bool>,
    accept_task: AsyncRefCell<Option<tokio::task::JoinHandle<()>>>,
}

impl ServerResource {
    fn new(
        requests: mpsc::Receiver<QueuedRequest>,
        shutdown: watch::Sender<bool>,
        accept_task: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            requests: AsyncRefCell::new(requests),
            shutdown,
            accept_task: AsyncRefCell::new(Some(accept_task)),
        }
    }

    fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Resource for ServerResource {
    fn name(&self) -> Cow<'_, str> {
        "httpServer".into()
    }

    fn close(self: Rc<Self>) {
        self.request_shutdown();
    }
}

pub struct ResponseSlotResource {
    sender: RefCell<Option<oneshot::Sender<WireResponse>>>,
}

impl ResponseSlotResource {
    fn new(sender: oneshot::Sender<WireResponse>) -> Self {
        Self {
            sender: RefCell::new(Some(sender)),
        }
    }

    fn take(&self) -> Option<oneshot::Sender<WireResponse>> {
        self.sender.borrow_mut().take()
    }
}

impl Resource for ResponseSlotResource {
    fn name(&self) -> Cow<'_, str> {
        "httpResponseSlot".into()
    }
}

#[op2]
#[serde]
pub fn op_http_serve(
    state: &mut OpState,
    #[string] hostname: String,
    #[smi] port: u16,
) -> Result<ServeHandle, HttpError> {
    let requested = format!("{hostname}:{port}");
    let addr = parse_bind_addr(&hostname, port).ok_or(HttpError::BadAddr { addr: requested })?;
    // The network capability is checked BEFORE any socket (I-6 governed entry).
    let caps = state
        .try_borrow::<Rc<dyn CapabilityCheck>>()
        .cloned()
        .unwrap_or_else(|| Rc::new(AllowAll));
    caps.check(&CapRequest::NetListen(&addr))
        .map_err(|_| HttpError::Denied {
            addr: addr.to_string(),
        })?;
    // SYNC bind so `serve()` returns the RESOLVED address immediately (Server.addr,
    // A1): port 0 becomes the OS-assigned port before serve() returns, matching the
    // spec + Deno. Only listen() is synchronous; the accept loop stays async.
    let std_listener = std::net::TcpListener::bind(addr).map_err(|source| HttpError::Bind {
        addr: addr.to_string(),
        source,
    })?;
    std_listener
        .set_nonblocking(true)
        .map_err(|source| HttpError::Bind {
            addr: addr.to_string(),
            source,
        })?;
    let bound = std_listener
        .local_addr()
        .map_err(|source| HttpError::Bind {
            addr: addr.to_string(),
            source,
        })?;
    let listener =
        tokio::net::TcpListener::from_std(std_listener).map_err(|source| HttpError::Bind {
            addr: addr.to_string(),
            source,
        })?;

    let (request_tx, request_rx) = mpsc::channel(64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let accept_task = tokio::spawn(run_server(listener, bound, request_tx, shutdown_rx));

    let rid = state
        .resource_table
        .add(ServerResource::new(request_rx, shutdown_tx, accept_task));
    Ok(ServeHandle {
        rid,
        addr: bound.into(),
    })
}

#[op2]
#[serde]
pub async fn op_http_next(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: u32,
) -> Result<Option<IncomingRequest>, HttpError> {
    let server = state
        .borrow()
        .resource_table
        .get::<ServerResource>(rid)
        .map_err(|_| HttpError::NoServer { rid })?;

    let queued = {
        let requests = RcRef::map(server.clone(), |resource| &resource.requests);
        let mut receiver = requests.borrow_mut().await;
        receiver.recv().await
    };

    let Some(queued) = queued else {
        let maybe_task = {
            let accept_task = RcRef::map(server.clone(), |resource| &resource.accept_task);
            let mut task = accept_task.borrow_mut().await;
            task.take()
        };
        if let Some(task) = maybe_task {
            task.await.map_err(|err| HttpError::Task {
                message: err.to_string(),
            })?;
        }
        let _ = state
            .borrow_mut()
            .resource_table
            .take::<ServerResource>(rid);
        return Ok(None);
    };

    let slot = state
        .borrow_mut()
        .resource_table
        .add(ResponseSlotResource::new(queued.response));
    Ok(Some(IncomingRequest {
        slot,
        method: queued.method,
        url: queued.url,
        headers: queued.headers,
        body: queued.body,
    }))
}

#[op2]
pub async fn op_http_respond(
    state: Rc<RefCell<OpState>>,
    #[smi] slot: u32,
    #[smi] status: u16,
    #[serde] headers: Vec<(String, String)>,
    #[buffer] body: JsBuffer,
) -> Result<(), HttpError> {
    let slot_resource = state
        .borrow_mut()
        .resource_table
        .take::<ResponseSlotResource>(slot)
        .map_err(|_| HttpError::NoSlot { slot })?;
    let sender = slot_resource.take().ok_or(HttpError::NoSlot { slot })?;
    sender
        .send(WireResponse {
            status,
            headers,
            body: body.to_vec(),
        })
        .map_err(|_| HttpError::ResponseClosed { slot })
}

#[op2]
pub async fn op_http_shutdown(
    state: Rc<RefCell<OpState>>,
    #[smi] rid: u32,
) -> Result<(), HttpError> {
    let server = match state.borrow().resource_table.get::<ServerResource>(rid) {
        Ok(server) => server,
        Err(_) => return Ok(()),
    };
    server.request_shutdown();
    Ok(())
}

async fn run_server(
    listener: tokio::net::TcpListener,
    local_addr: SocketAddr,
    request_tx: mpsc::Sender<QueuedRequest>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let builder = ServerBuilder::new(TokioExecutor::new());
    let graceful = GracefulShutdown::new();
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                let _ = changed;
                break;
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(_) => break,
                };
                let per_connection_tx = request_tx.clone();
                let service = service_fn(move |request: Request<Incoming>| {
                    let per_request_tx = per_connection_tx.clone();
                    async move {
                        Ok::<_, std::convert::Infallible>(
                            handle_request(request, local_addr, per_request_tx).await,
                        )
                    }
                });
                let connection = builder
                    .clone()
                    .serve_connection(TokioIo::new(stream), service)
                    .into_owned();
                let watched = graceful.watch(connection);
                connections.spawn(async move {
                    let _ = watched.await;
                });
            }
        }
    }

    drop(request_tx);

    let shutdown = tokio::spawn(graceful.shutdown());
    while let Some(joined) = connections.join_next().await {
        let _ = joined;
    }
    let _ = shutdown.await;
}

async fn handle_request(
    request: Request<Incoming>,
    local_addr: SocketAddr,
    request_tx: mpsc::Sender<QueuedRequest>,
) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let collected = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => {
            return response_with(StatusCode::BAD_REQUEST, b"Bad Request");
        }
    };

    let (response_tx, response_rx) = oneshot::channel();
    let queued = QueuedRequest {
        method: parts.method.as_str().to_owned(),
        url: request_url(&parts, local_addr),
        headers: header_pairs(&parts.headers),
        body: collected.to_vec(),
        response: response_tx,
    };

    if request_tx.send(queued).await.is_err() {
        return response_with(StatusCode::SERVICE_UNAVAILABLE, b"Server shutting down");
    }

    match response_rx.await {
        Ok(response) => match response.into_hyper() {
            Ok(response) => response,
            Err(_) => response_with(StatusCode::INTERNAL_SERVER_ERROR, b"Internal Server Error"),
        },
        Err(_) => response_with(StatusCode::INTERNAL_SERVER_ERROR, b"Internal Server Error"),
    }
}

fn parse_bind_addr(hostname: &str, port: u16) -> Option<SocketAddr> {
    let candidate = if hostname.contains(':') && !hostname.starts_with('[') {
        format!("[{hostname}]:{port}")
    } else {
        format!("{hostname}:{port}")
    };
    candidate.parse().ok()
}

fn request_url(parts: &hyper::http::request::Parts, local_addr: SocketAddr) -> String {
    let authority = parts
        .headers
        .get(hyper::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| local_addr.to_string());
    let path = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    format!("http://{authority}{path}")
}

fn header_pairs(headers: &hyper::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let value = value
                .to_str()
                .map(str::to_owned)
                .unwrap_or_else(|_| String::from_utf8_lossy(value.as_bytes()).into_owned());
            (name.as_str().to_owned(), value)
        })
        .collect()
}

fn response_with(status: StatusCode, body: &'static [u8]) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from_static(body)));
    *response.status_mut() = status;
    response
}
