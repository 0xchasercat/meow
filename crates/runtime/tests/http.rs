//! RT-005 behavior tests for `meow:http`.
//!
//! These run the authored `meow:http` module through the real shared loader and
//! the real Hyper-backed op layer: import `meow:http`, bind, serve requests,
//! surface capability denial before bind, and keep serving after handler faults.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::Request;
use hyper::StatusCode;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use meow_graph::GraphDb;
use meow_loader::MeowModuleLoader;
use meow_pkg::{Cache, Lockfile};
use meow_runtime::{
    io_capability_extension, print_sink_extension, CapDenied, CapRequest, CapabilityCheck,
    ModuleSpecifier, PrintSink, Runtime, RuntimeOptions,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-rt005-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[derive(Clone)]
enum Policy {
    Allow,
    DenyNetListen(Arc<RecordedNetListenDeny>),
}

#[derive(Default)]
struct RecordedNetListenDeny {
    calls: AtomicUsize,
    addrs: Mutex<Vec<String>>,
}

struct NetListenDeny {
    shared: Arc<RecordedNetListenDeny>,
}

impl CapabilityCheck for NetListenDeny {
    fn check(&self, req: &CapRequest<'_>) -> Result<(), CapDenied> {
        if let CapRequest::NetListen(addr) = req {
            self.shared.calls.fetch_add(1, Ordering::SeqCst);
            self.shared
                .addrs
                .lock()
                .expect("mutex")
                .push(addr.to_string());
            return Err(CapDenied("deny net listen for test".into()));
        }
        Ok(())
    }
}

struct RuntimeThread {
    logs: Arc<Mutex<Vec<(bool, String)>>>,
    rx: UnboundedReceiver<(bool, String)>,
    join: Option<std::thread::JoinHandle<Result<(), String>>>,
    root: PathBuf,
}

impl RuntimeThread {
    async fn wait_for_listen(&mut self) -> (String, u16) {
        loop {
            let Some((is_err, line)) = self.rx.recv().await else {
                let logs = self.logs.lock().expect("mutex").clone();
                let result = self
                    .join
                    .take()
                    .expect("join handle")
                    .join()
                    .expect("runtime thread join");
                panic!("runtime should emit a listen line, result={result:?}, logs={logs:?}");
            };
            assert!(!is_err, "unexpected stderr before bind: {line:?}");
            if let Some(raw) = line.trim().strip_prefix("LISTEN ") {
                let (host, port) = raw.rsplit_once(':').expect("host:port listen line");
                return (host.to_owned(), port.parse().expect("port number"));
            }
        }
    }

    fn finish(mut self) -> Vec<(bool, String)> {
        let result = self
            .join
            .take()
            .expect("join handle")
            .join()
            .expect("runtime thread join");
        let logs = self.logs.lock().expect("mutex").clone();
        std::fs::remove_dir_all(&self.root).ok();
        result.expect("runtime module completes cleanly");
        logs
    }
}

fn spawn_runtime(source: &str, policy: Policy) -> RuntimeThread {
    let root = unique_dir("runtime");
    let spec = ModuleSpecifier::from_file_path(root.join("main.ts")).expect("main specifier");
    let source = source.to_owned();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let log_store = logs.clone();
    let (tx, rx) = unbounded_channel::<(bool, String)>();
    let join_root = root.clone();

    let join = std::thread::spawn(move || -> Result<(), String> {
        let async_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| err.to_string())?;
        async_rt.block_on(async move {
            let sink = print_sink_extension(PrintSink(Rc::new(move |msg: &str, is_err: bool| {
                let line = msg.to_owned();
                log_store
                    .lock()
                    .expect("mutex")
                    .push((is_err, line.clone()));
                let _ = tx.send((is_err, line));
            })));

            let resolver = meow_loader::Resolver::new(
                Arc::new(Cache::with_root(join_root.join("cache"))),
                Arc::new(Lockfile::new()),
                BTreeMap::new(),
                ModuleSpecifier::from_directory_path(&join_root).expect("project root URL"),
                meow_runtime::native::native_module_registry(),
            );
            let loader: Rc<dyn meow_runtime::deno_core::ModuleLoader> =
                Rc::new(MeowModuleLoader::new(
                    resolver.clone(),
                    Rc::new(std::cell::RefCell::new(GraphDb::new())),
                ));

            let caps = Arc::new(meow_runtime::AllowAll);
            let mut extensions = vec![sink];
            extensions.extend(meow_runtime::hermetic::extensions(
                meow_runtime::hermetic::HermeticConfig::default(),
            ));
            extensions.push(meow_runtime::http_extension());
            extensions.extend(meow_runtime::node::extensions(
                meow_runtime::node::NodeOptions {
                    mode: meow_runtime::node::NodeMode::Enabled,
                    argv: vec![
                        "meow".to_owned(),
                        join_root.join("main.ts").to_string_lossy().into_owned(),
                    ],
                    cwd: join_root.clone(),
                    env: BTreeMap::new(),
                    deno_node_services: None,
                    caps: Some(caps),
                    user_agent: Some("meow-test/rt005".to_owned()),
                },
            ));
            if let Policy::DenyNetListen(shared) = policy {
                extensions.push(io_capability_extension(Rc::new(NetListenDeny { shared })));
            }

            let mut runtime = Runtime::new(RuntimeOptions {
                module_loader: loader,
                extensions,
                max_heap_size: None,
                startup_snapshot: None,
                residual_lazy_js_sources: &[],
                residual_lazy_esm_sources: &[],
            })
            .map_err(|err| err.to_string())?;
            runtime
                .run_main_module_from_source(&spec, source)
                .await
                .map_err(|err| err.to_string())
        })
    });

    RuntimeThread {
        logs,
        rx,
        join: Some(join),
        root,
    }
}

async fn http_get(hostname: &str, port: u16, path: &str) -> (StatusCode, hyper::HeaderMap, String) {
    let client = Client::builder(hyper_util::rt::TokioExecutor::new()).build(HttpConnector::new());
    let uri: hyper::Uri = format!("http://{hostname}:{port}{path}")
        .parse()
        .expect("request uri");
    let request = Request::builder()
        .uri(uri)
        .body(Empty::<Bytes>::new())
        .expect("request");
    let response = client
        .request(request)
        .await
        .expect("http request succeeds");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("utf8 body");
    (status, headers, text)
}

#[tokio::test]
async fn serve_handles_request() {
    let mut server = spawn_runtime(
        r#"
        import { serve } from "meow:http";

        let server;
        server = serve((req) => {
          const path = new URL(req.url).pathname;
          if (path === "/__shutdown") {
            void Promise.resolve().then(() => server.shutdown());
            return new Response("bye");
          }
          return new Response("hi " + path, {
            status: 200,
            headers: {
              "x-method": req.method,
            },
          });
        }, {
          hostname: "127.0.0.1",
          port: 0,
          onListen(addr) {
            console.log(`LISTEN ${addr.hostname}:${addr.port}`);
          },
        });

        await server.finished;
        console.log("FINISHED");
        "#,
        Policy::Allow,
    );

    let (hostname, port) = server.wait_for_listen().await;
    let (status, headers, body) = http_get(&hostname, port, "/world").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hi /world");
    assert_eq!(
        headers.get("x-method").and_then(|v| v.to_str().ok()),
        Some("GET")
    );

    let (shutdown_status, _, shutdown_body) = http_get(&hostname, port, "/__shutdown").await;
    assert_eq!(shutdown_status, StatusCode::OK);
    assert_eq!(shutdown_body, "bye");

    let logs = server.finish();
    assert!(
        logs.iter()
            .any(|(is_err, line)| !*is_err && line.trim() == "FINISHED"),
        "shutdown drains and resolves finished: {logs:?}"
    );
}

#[tokio::test]
async fn node_http_create_server_handles_basic_get() {
    let mut server = spawn_runtime(
        r#"
        import http from "http";

        let server;
        server = http.createServer((req, res) => {
          req.on("end", () => {
            if (req.url === "/__shutdown") {
              res.writeHead(200, { "x-shutdown": "yes" });
              res.end("bye", () => server.close());
              return;
            }
            res.setHeader("x-method", req.method);
            res.write("hi ");
            res.end(req.url);
          });
        });

        server.listen(0, "127.0.0.1", () => {
          const addr = server.address();
          console.log(`LISTEN ${addr.address}:${addr.port}`);
        });

        await new Promise((resolve) => server.on("close", resolve));
        console.log("FINISHED");
        "#,
        Policy::Allow,
    );

    let (hostname, port) = server.wait_for_listen().await;
    let (status, headers, body) = http_get(&hostname, port, "/world").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hi /world");
    assert_eq!(
        headers.get("x-method").and_then(|v| v.to_str().ok()),
        Some("GET")
    );

    let (shutdown_status, shutdown_headers, shutdown_body) =
        http_get(&hostname, port, "/__shutdown").await;
    assert_eq!(shutdown_status, StatusCode::OK);
    assert_eq!(shutdown_body, "bye");
    assert_eq!(
        shutdown_headers
            .get("x-shutdown")
            .and_then(|v| v.to_str().ok()),
        Some("yes")
    );

    let logs = server.finish();
    assert!(
        logs.iter()
            .any(|(is_err, line)| !*is_err && line.trim() == "FINISHED"),
        "node:http close emits close: {logs:?}"
    );
}

#[tokio::test]
async fn serve_capability_gated() {
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let denied_port = probe.local_addr().expect("probe addr").port();
    drop(probe);

    let recorded = Arc::new(RecordedNetListenDeny::default());
    let server = spawn_runtime(
        &format!(
            r#"
            import {{ serve }} from "meow:http";

            const server = serve(() => new Response("blocked"), {{
              hostname: "127.0.0.1",
              port: {denied_port},
            }});

            try {{
              await server.finished;
              throw new Error("expected bind failure");
            }} catch (error) {{
              console.log(String(error instanceof Error ? error.message : error));
            }}
            "#,
        ),
        Policy::DenyNetListen(recorded.clone()),
    );

    let logs = server.finish();
    assert_eq!(
        recorded.calls.load(Ordering::SeqCst),
        1,
        "one NetListen check"
    );
    assert_eq!(
        recorded.addrs.lock().expect("mutex").as_slice(),
        &[format!("127.0.0.1:{denied_port}")],
        "the seam saw the exact bind target"
    );
    assert!(
        logs.iter().any(|(is_err, line)| {
            !*is_err
                && line.contains("network capability is required to bind")
                && line.contains("meow.config.ts")
        }),
        "bind denial surfaced to JS: {logs:?}"
    );

    tokio::net::TcpListener::bind(("127.0.0.1", denied_port))
        .await
        .expect("port stayed free because bind was denied before socket creation");
}

#[tokio::test]
async fn serve_handler_error_is_500() {
    let mut server = spawn_runtime(
        r#"
        import { serve } from "meow:http";

        let server;
        server = serve((req) => {
          const path = new URL(req.url).pathname;
          if (path === "/throw") {
            throw new Error("boom");
          }
          if (path === "/bad") {
            return 123;
          }
          if (path === "/__shutdown") {
            void Promise.resolve().then(() => server.shutdown());
            return new Response("bye");
          }
          return new Response("ok");
        }, {
          hostname: "127.0.0.1",
          port: 0,
          onListen(addr) {
            console.log(`LISTEN ${addr.hostname}:${addr.port}`);
          },
        });

        await server.finished;
        "#,
        Policy::Allow,
    );

    let (hostname, port) = server.wait_for_listen().await;

    let (throw_status, _, throw_body) = http_get(&hostname, port, "/throw").await;
    assert_eq!(throw_status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(throw_body, "Internal Server Error");

    let (bad_status, _, bad_body) = http_get(&hostname, port, "/bad").await;
    assert_eq!(bad_status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(bad_body, "Internal Server Error");

    let (ok_status, _, ok_body) = http_get(&hostname, port, "/ok").await;
    assert_eq!(ok_status, StatusCode::OK);
    assert_eq!(ok_body, "ok");

    let _ = http_get(&hostname, port, "/__shutdown").await;
    let logs = server.finish();
    let stderr = logs
        .iter()
        .filter(|(is_err, _)| *is_err)
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("");
    assert!(
        stderr.contains("meow:http handler failed"),
        "handler errors are reported: {stderr:?}"
    );
    assert!(stderr.contains("boom"), "throw path reported: {stderr:?}");
    assert!(
        stderr.contains("must return a Response"),
        "non-Response path reported: {stderr:?}"
    );
}
