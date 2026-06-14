//! RT-004 behavior tests: the strict-web Stateless-Edge globals (CANON §8.1) are
//! installed on the default runtime and spec-conformant, and the one host-touching
//! global (`fetch`) is routed through the RT-002 capability seam — a DENY policy
//! makes it reject (never panic) and the seam observes the connect target. Also
//! guards the boundary: no DOM / `window` / `localStorage`.
//!
//! Tests assert behavior from JS via `meow run`-style module evaluation, not
//! plumbing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use meow_runtime::web::{extensions, NetCaps, WebOptions};
use meow_runtime::{
    print_sink_extension, ModuleSpecifier, PrintSink, Runtime, RuntimeError, RuntimeOptions,
    TrivialModuleLoader,
};

// --- helpers ---------------------------------------------------------------

/// A capture sink + the captured buffer (console.log output, deterministic — no OS).
fn capture() -> (Rc<RefCell<String>>, deno_core::Extension) {
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    let sink = PrintSink(Rc::new(move |msg: &str, _is_err: bool| {
        o.borrow_mut().push_str(msg);
    }));
    (out, print_sink_extension(sink))
}

/// Build a default runtime with the Web globals installed and a capture sink.
fn web_runtime(caps: NetCaps) -> (Rc<RefCell<String>>, Runtime) {
    let (out, sink_ext) = capture();
    let mut exts = extensions(WebOptions {
        caps,
        user_agent: "meow/test".to_string(),
    });
    exts.push(sink_ext);
    let rt = Runtime::new(RuntimeOptions {
        module_loader: Rc::new(TrivialModuleLoader::new()),
        extensions: exts,
    })
    .expect("runtime initializes with web globals");
    (out, rt)
}

fn allow_all() -> NetCaps {
    Arc::new(meow_runtime::AllowAll)
}

async fn run_src(rt: &mut Runtime, url: &str, src: &str) -> Result<(), RuntimeError> {
    let spec = ModuleSpecifier::parse(url).expect("valid specifier");
    rt.run_main_module_from_source(&spec, src.to_string()).await
}

// --- light globals ---------------------------------------------------------

#[tokio::test]
async fn web_url() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///url.js",
        r#"
        const u = new URL("https://h.example/p?a=1&a=2#x");
        console.log(u.pathname + "|" + JSON.stringify(u.searchParams.getAll("a")));
        const m = new URLPattern({ pathname: "/u/:id" }).exec("https://h/u/42");
        console.log(m.pathname.groups.id);
        "#,
    )
    .await
    .expect("url module runs");
    assert_eq!(out.borrow().trim(), "/p|[\"1\",\"2\"]\n42");
}

#[tokio::test]
async fn web_text_codec() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///text.js",
        r#"console.log(new TextDecoder().decode(new TextEncoder().encode("héllo")));"#,
    )
    .await
    .expect("text module runs");
    assert_eq!(out.borrow().trim(), "héllo");
}

#[tokio::test]
async fn web_crypto_subtle() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///crypto.js",
        r#"
        const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
        const hex = Array.from(new Uint8Array(d)).map(b => b.toString(16).padStart(2, "0")).join("");
        console.log(hex);
        "#,
    )
    .await
    .expect("crypto module runs");
    assert_eq!(
        out.borrow().trim(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

// `Blob` is a deno_web (light) global, available even when `web-fetch` is off.
#[tokio::test]
async fn web_blob() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///blob.js",
        r#"console.log(await new Blob(["xy"]).text());"#,
    )
    .await
    .expect("blob module runs");
    assert_eq!(out.borrow().trim(), "xy");
}

#[tokio::test]
async fn web_set_timeout() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///timer.js",
        r#"await new Promise((r) => setTimeout(r, 1)); console.log("resolved");"#,
    )
    .await
    .expect("timer module runs");
    assert_eq!(out.borrow().trim(), "resolved");
}

#[tokio::test]
async fn web_no_dom() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///dom.js",
        r#"console.log(typeof window, typeof document, typeof localStorage, typeof navigator);"#,
    )
    .await
    .expect("dom probe runs");
    assert_eq!(
        out.borrow().trim(),
        "undefined undefined undefined undefined"
    );
}

// `File` ships transitively via deno_web but is NOT a committed §8.1 global
// (which commits only `Blob`/`FormData`): the bootstrap must not bind it, so it
// stays `undefined` while `Blob` (committed) is present (I-11 superset honesty).
#[tokio::test]
async fn web_file_is_not_a_global() {
    let (out, mut rt) = web_runtime(allow_all());
    run_src(
        &mut rt,
        "file:///file.js",
        r#"console.log(typeof File, typeof Blob);"#,
    )
    .await
    .expect("file probe runs");
    assert_eq!(out.borrow().trim(), "undefined function");
}

// The curated ambient decl `meow sync` writes (and threads into the shadow
// tsconfig) tracks the `web-fetch` feature: a no-fetch build must not type a
// global that throws `ReferenceError` at runtime. Asserted in both build configs
// so `cargo test -p meow-runtime [--no-default-features]` both hold (finding 2).
#[test]
fn strict_web_dts_tracks_web_fetch_feature() {
    let dts = meow_runtime::web::STRICT_WEB_DTS;
    // Base globals are present regardless of feature.
    assert!(dts.contains("declare var Blob"), "Blob is always committed");
    assert!(dts.contains("declare var URL"), "URL is always committed");
    // `File` is never a committed global in either build.
    assert!(
        !dts.contains("declare var File"),
        "File must not be a committed global"
    );

    let fetch_decls = [
        "declare function fetch",
        "declare var Headers",
        "declare var Request",
        "declare var Response",
        "declare var FormData",
    ];
    for d in fetch_decls {
        assert!(
            dts.contains(d),
            "default build must type the fetch group: {d}"
        );
    }
}

mod fetch {
    use super::*;
    use meow_runtime::{CapDenied, CapRequest, CapabilityCheck};
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn web_response_headers_request() {
        let (out, mut rt) = web_runtime(allow_all());
        run_src(
            &mut rt,
            "file:///resp.js",
            r#"
            console.log(await new Response("hi").text());
            console.log(new Headers({ a: "b" }).get("a"));
            console.log(new Request("https://h/").method);
            "#,
        )
        .await
        .expect("response module runs");
        assert_eq!(out.borrow().trim(), "hi\nb\nGET");
    }

    #[tokio::test]
    async fn web_formdata() {
        let (out, mut rt) = web_runtime(allow_all());
        run_src(
            &mut rt,
            "file:///fd.js",
            r#"
            const fd = new FormData();
            fd.append("k", "v");
            const got = await new Request("https://h/", { method: "POST", body: fd }).formData();
            console.log(got.get("k"));
            "#,
        )
        .await
        .expect("formdata module runs");
        assert_eq!(out.borrow().trim(), "v");
    }

    /// Bind a loopback HTTP/1.1 server that answers every request with `pong`.
    /// Returns the bound port; the accept loop runs on the same current-thread
    /// runtime as the test (cooperatively scheduled).
    async fn pong_server() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let _ = sock
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\npong",
                        )
                        .await;
                    let _ = sock.flush().await;
                });
            }
        });
        port
    }

    /// A capability check that DENIES every request and records the connect
    /// targets it was asked about. `Send + Sync` (the fetch resolver runs on a
    /// spawned task), no poisoning lock (parking_lot).
    struct RecordingDeny {
        net_checks: AtomicUsize,
        targets: Mutex<Vec<String>>,
    }

    impl CapabilityCheck for RecordingDeny {
        fn check(&self, req: &CapRequest<'_>) -> Result<(), CapDenied> {
            if let CapRequest::NetConnect(target) = req {
                self.net_checks.fetch_add(1, Ordering::SeqCst);
                self.targets.lock().push((*target).to_string());
                return Err(CapDenied(format!("net denied: {target}")));
            }
            Ok(())
        }
    }

    // `fetch` works end-to-end against a real server with the default AllowAll cap.
    #[tokio::test]
    async fn web_fetch_allowed() {
        let port = pong_server().await;
        let (out, mut rt) = web_runtime(allow_all());
        let src = format!(
            r#"
            const r = await fetch("http://127.0.0.1:{port}/");
            console.log(r.status + ":" + (await r.text()));
            "#
        );
        run_src(&mut rt, "file:///fetch_ok.js", &src)
            .await
            .expect("fetch module runs");
        assert_eq!(out.borrow().trim(), "200:pong");
    }

    // The I-6 seam invariant: a DENY capability policy makes `fetch` reject with a
    // catchable error (never a Rust panic), and the seam observed the connect
    // target before any socket was opened (the gate is on the path).
    #[tokio::test]
    async fn web_fetch_denied() {
        let port = pong_server().await;
        let recorder = Arc::new(RecordingDeny {
            net_checks: AtomicUsize::new(0),
            targets: Mutex::new(Vec::new()),
        });
        let caps: NetCaps = recorder.clone();
        let (out, mut rt) = web_runtime(caps);
        // A named host so the connector consults the DNS resolver (where the gate
        // lives); IP literals bypass DNS resolution.
        let src = format!(
            r#"
            let result = "no-throw";
            try {{
                await fetch("http://localhost:{port}/");
            }} catch (e) {{
                result = "rejected:" + (e && e.constructor ? e.constructor.name : typeof e);
            }}
            console.log(result);
            "#
        );
        run_src(&mut rt, "file:///fetch_deny.js", &src)
            .await
            .expect("fetch module runs without panicking");

        let logged = out.borrow().clone();
        assert!(
            logged.starts_with("rejected:"),
            "fetch should reject under a DENY policy, got: {logged:?}"
        );
        assert!(
            recorder.net_checks.load(Ordering::SeqCst) >= 1,
            "the capability seam must be consulted on the fetch path"
        );
        // The seam must see the FULL connect target — host:port, not just the host
        // (parity with op_tcp_connect): a policy can distinguish :80 from :8443.
        let expected = format!("localhost:{port}");
        assert!(
            recorder.targets.lock().contains(&expected),
            "the seam saw the host:port connect target {expected:?}, got: {:?}",
            recorder.targets.lock()
        );
    }

    /// Bind a loopback server that accepts but NEVER responds, so an in-flight
    /// request stays pending until the client aborts it. The handle is dropped at
    /// test end (the runtime tears the spawned task down).
    async fn hang_server() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock); // hold the connection open; never write a response
            }
        });
        port
    }

    // `AbortController` integrates with `fetch`: aborting the signal makes an
    // in-flight request reject with an `AbortError` (never a Rust panic). The
    // `abort()` runs synchronously right after `fetch()`, before the op yields, so
    // the abort is observed regardless of connection timing (deterministic).
    #[tokio::test]
    async fn web_fetch_abort() {
        let port = hang_server().await;
        let (out, mut rt) = web_runtime(allow_all());
        let src = format!(
            r#"
            const ac = new AbortController();
            const p = fetch("http://127.0.0.1:{port}/", {{ signal: ac.signal }});
            ac.abort();
            let result = "no-throw";
            try {{
                await p;
            }} catch (e) {{
                result = "rejected:" + (e && e.name ? e.name : typeof e);
            }}
            console.log(result);
            "#
        );
        run_src(&mut rt, "file:///fetch_abort.js", &src)
            .await
            .expect("fetch module runs without panicking");
        assert_eq!(
            out.borrow().trim(),
            "rejected:AbortError",
            "aborting the signal rejects the fetch with an AbortError"
        );
    }
}
