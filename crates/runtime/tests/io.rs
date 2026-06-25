//! RT-002 behavior tests. These assert invariants, not plumbing: an async I/O
//! op resolves under the event loop from JS, concurrent ops resolve, the
//! capability seam sits on every op path (a DENY refuses with a typed error,
//! never a panic), TCP connect succeeds to a live listener and fails typed to a
//! closed port, and bad input rejects without panicking (CRAFT no-panic).

mod real_loader;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use meow_runtime::{
    io_capability_extension, print_sink_extension, CapDenied, CapRequest, CapabilityCheck,
    ModuleSpecifier, PrintSink, Runtime, RuntimeError, RuntimeOptions,
};

// --- helpers ---------------------------------------------------------------

fn capture() -> (Rc<RefCell<String>>, deno_core::Extension) {
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    let sink = PrintSink(Rc::new(move |msg: &str, _is_err: bool| {
        o.borrow_mut().push_str(msg);
    }));
    (out, print_sink_extension(sink))
}

fn runtime_with(extensions: Vec<deno_core::Extension>) -> Runtime {
    Runtime::new(RuntimeOptions {
        module_loader: real_loader::loader_for(&real_loader::unique_dir("runtime")),
        extensions,
        max_heap_size: None,
        startup_snapshot: None,
        residual_lazy_js_sources: &[],
        residual_lazy_esm_sources: &[],
    })
    .expect("runtime initializes")
}

// Opt-in raw host-I/O ops. `Runtime::new` no longer installs these by default
// (host I/O is not exposed to user JS); callers that need them pass this through
// `RuntimeOptions.extensions`.
fn io_ext() -> deno_core::Extension {
    meow_runtime::io::meow_io::init()
}

async fn run_src(rt: &mut Runtime, url: &str, src: &str) -> Result<(), RuntimeError> {
    let spec = ModuleSpecifier::parse(url).expect("valid specifier");
    rt.run_main_module_from_source(&spec, src.to_string()).await
}

/// A unique temp file seeded with `contents`. No clock read (I-6): a process-id +
/// monotonic counter names the file. Returns its absolute path as a string.
fn temp_file(contents: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("meow-rt002-{}-{}.txt", std::process::id(), n));
    std::fs::write(&path, contents).expect("write temp file");
    path.to_string_lossy().into_owned()
}

/// JS escaping for a Windows-friendly path literal (backslashes → escaped).
fn js_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\"))
}

// --- tests -----------------------------------------------------------------

// An async read op resolves under the event loop and JS sees the file bytes.
#[tokio::test]
async fn io_async_read_resolves_from_js() {
    let contents = "meow-async-io-payload";
    let path = temp_file(contents);
    let (out, sink) = capture();
    let mut rt = runtime_with(vec![sink, io_ext()]);
    run_src(
        &mut rt,
        "file:///read.mjs",
        &format!(
            r#"
            const bytes = await Deno.core.ops.op_read_file({path});
            let s = "";
            for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
            console.log(s);
            "#,
            path = js_str(&path)
        ),
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), format!("{contents}\n"));
}

// Concurrent reads + concurrent TCP connects all resolve under one loop.
#[tokio::test]
async fn io_concurrent_ops_resolve() {
    let a = temp_file("alpha");
    let b = temp_file("bravo");
    let c = temp_file("charlie");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("local addr").to_string();
    let accepted = Arc::new(AtomicUsize::new(0));
    let acc = accepted.clone();
    // Accept exactly two connections; the spawned task is polled by the same
    // runtime that drives the event loop. Keep the sockets alive until done.
    let accept_task = tokio::spawn(async move {
        let mut held = Vec::new();
        for _ in 0..2 {
            let (sock, _) = listener.accept().await.expect("accept");
            held.push(sock);
            acc.fetch_add(1, Ordering::SeqCst);
        }
        held
    });

    let (out, sink) = capture();
    let mut rt = runtime_with(vec![sink, io_ext()]);
    run_src(
        &mut rt,
        "file:///concurrent.mjs",
        &format!(
            r#"
            const reads = await Promise.all([
              Deno.core.ops.op_read_file({a}),
              Deno.core.ops.op_read_file({b}),
              Deno.core.ops.op_read_file({c}),
            ]);
            const conns = await Promise.all([
              Deno.core.ops.op_tcp_connect({addr}),
              Deno.core.ops.op_tcp_connect({addr}),
            ]);
            const allNums = conns.every((r) => typeof r === "number" && r >= 0);
            const distinct = conns[0] !== conns[1];
            console.log(reads.length + ":" + conns.length + ":" + allNums + ":" + distinct);
            "#,
            a = js_str(&a),
            b = js_str(&b),
            c = js_str(&c),
            addr = js_str(&addr),
        ),
    )
    .await
    .expect("module runs");

    // Drive the accept task to completion now that both connects resolved.
    tokio::time::timeout(Duration::from_secs(5), accept_task)
        .await
        .expect("accept did not hang")
        .expect("accept task ok");

    assert_eq!(*out.borrow(), "3:2:true:true\n");
    assert_eq!(accepted.load(Ordering::SeqCst), 2, "both connects accepted");
}

/// A DENY policy that records how many ReadFile checks it saw, then refuses.
struct RecordingDeny {
    reads: Cell<usize>,
}

impl CapabilityCheck for RecordingDeny {
    fn check(&self, req: &CapRequest<'_>) -> Result<(), CapDenied> {
        if let CapRequest::ReadFile(_) = req {
            self.reads.set(self.reads.get() + 1);
        }
        Err(CapDenied("test policy denies all".into()))
    }
}

// Every host op traverses the capability seam: a DENY refuses with a typed
// error (a rejected promise JS can catch), and the recorder saw exactly one
// ReadFile check — no panic.
#[tokio::test]
async fn io_seam_denies_with_typed_error() {
    let path = temp_file("should-not-be-read");
    let recorder = Rc::new(RecordingDeny {
        reads: Cell::new(0),
    });
    let (out, sink) = capture();
    let mut rt = runtime_with(vec![
        sink,
        io_ext(),
        io_capability_extension(recorder.clone()),
    ]);
    run_src(
        &mut rt,
        "file:///deny.mjs",
        &format!(
            r#"
            let denied = false;
            try {{
              await Deno.core.ops.op_read_file({path});
            }} catch (e) {{
              denied = true;
              console.log(e.message);
            }}
            if (!denied) throw new Error("expected the read to be denied");
            "#,
            path = js_str(&path)
        ),
    )
    .await
    .expect("module runs (JS catches the typed rejection)");

    assert_eq!(recorder.reads.get(), 1, "the read traversed the seam once");
    assert!(
        out.borrow().contains("permission denied"),
        "typed CapDenied surfaced to JS, got: {:?}",
        out.borrow()
    );
}

// TCP connect resolves to a numeric rid for a live listener and rejects with a
// typed error for a closed port — neither panics.
#[tokio::test]
async fn io_tcp_connect_ok_and_refused() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let live = listener.local_addr().expect("addr").to_string();

    // A free port we immediately release: connecting to it is refused.
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let closed = probe.local_addr().expect("addr").to_string();
    drop(probe);

    let accepted = Arc::new(AtomicUsize::new(0));
    let acc = accepted.clone();
    let accept_task = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.expect("accept");
        acc.fetch_add(1, Ordering::SeqCst);
        sock
    });

    let (out, sink) = capture();
    let mut rt = runtime_with(vec![sink, io_ext()]);
    run_src(
        &mut rt,
        "file:///tcp.mjs",
        &format!(
            r#"
            const rid = await Deno.core.ops.op_tcp_connect({live});
            let refused = false;
            try {{
              await Deno.core.ops.op_tcp_connect({closed});
            }} catch (e) {{
              refused = true;
            }}
            console.log((typeof rid === "number" && rid >= 0) + ":" + refused);
            "#,
            live = js_str(&live),
            closed = js_str(&closed),
        ),
    )
    .await
    .expect("module runs");

    tokio::time::timeout(Duration::from_secs(5), accept_task)
        .await
        .expect("accept did not hang")
        .expect("accept ok");

    assert_eq!(*out.borrow(), "true:true\n");
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

// Bad input rejects with a typed error and never panics (CRAFT no-panic-on-input).
#[tokio::test]
async fn io_no_panic_on_bad_input() {
    let (out, sink) = capture();
    let mut rt = runtime_with(vec![sink, io_ext()]);
    run_src(
        &mut rt,
        "file:///bad.mjs",
        r#"
        let missing = false, badAddr = false;
        try { await Deno.core.ops.op_read_file("/no/such/meow/file/xyz"); }
        catch { missing = true; }
        try { await Deno.core.ops.op_tcp_connect("not-a-socket-addr"); }
        catch { badAddr = true; }
        if (!missing || !badAddr) throw new Error("expected typed errors");
        console.log("ok");
        "#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "ok\n");
}

// A DEFAULT runtime (no opt-in I/O extension) does NOT expose host I/O: the raw
// ops are absent from `Deno.core.ops`, so user JS cannot reach the host FS or
// network (I-6, I-8). This is the safe-by-default contract `meow run` relies on.
#[tokio::test]
async fn io_ops_absent_by_default() {
    let (out, sink) = capture();
    // Note: only the print sink — NO `io_ext()`.
    let mut rt = runtime_with(vec![sink]);
    run_src(
        &mut rt,
        "file:///default.mjs",
        r#"
        const readMissing = typeof Deno.core.ops.op_read_file === "undefined";
        const tcpMissing = typeof Deno.core.ops.op_tcp_connect === "undefined";
        let threw = false;
        try {
          Deno.core.ops.op_read_file("/etc/hosts");
        } catch {
          threw = true;
        }
        console.log(readMissing + ":" + tcpMissing + ":" + threw);
        "#,
    )
    .await
    .expect("module runs");
    // Both ops are absent, and attempting to call op_read_file throws (uncallable).
    assert_eq!(*out.borrow(), "true:true:true\n");
}
