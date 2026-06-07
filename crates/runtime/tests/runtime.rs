//! RT-001 behavior tests (T1–T5). These assert invariants, not plumbing:
//! a trivial ESM program is observable, the op seam round-trips JS↔Rust, an
//! uncaught exception becomes a typed error (never a panic), top-level await is
//! driven to completion, and the crate introduces no ambient host reads (I-6).

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{extension, op2};
use meow_runtime::{
    print_sink_extension, ModuleSpecifier, PrintSink, Runtime, RuntimeError, RuntimeOptions,
    TrivialModuleLoader,
};

/// A print sink that accumulates `console.log` output (and a separate buffer for
/// `console.error`), plus the extension that installs it.
fn capture() -> (
    Rc<RefCell<String>>,
    Rc<RefCell<String>>,
    deno_core::Extension,
) {
    let out = Rc::new(RefCell::new(String::new()));
    let err = Rc::new(RefCell::new(String::new()));
    let (o, e) = (out.clone(), err.clone());
    let sink = PrintSink(Rc::new(move |msg: &str, is_err: bool| {
        if is_err {
            e.borrow_mut().push_str(msg);
        } else {
            o.borrow_mut().push_str(msg);
        }
    }));
    (out, err, print_sink_extension(sink))
}

fn spec(url: &str) -> ModuleSpecifier {
    ModuleSpecifier::parse(url).expect("valid specifier")
}

fn runtime_with(extensions: Vec<deno_core::Extension>) -> Runtime {
    Runtime::new(RuntimeOptions {
        module_loader: Rc::new(TrivialModuleLoader::new()),
        extensions,
    })
    .expect("runtime initializes")
}

/// Run an ESM source string as the main module. `ModuleCodeString` has no
/// `From<&str>`, so own the source here (the public API takes `Into<ModuleCodeString>`).
async fn run_src(rt: &mut Runtime, url: &str, src: &str) -> Result<(), RuntimeError> {
    rt.run_main_module_from_source(&spec(url), src.to_string())
        .await
}

// T1 · a trivial ESM module runs end-to-end and its output is observable.
#[tokio::test]
async fn trivial_esm_runs_and_is_observable() {
    let (out, _err, sink_ext) = capture();
    let mut rt = runtime_with(vec![sink_ext]);
    run_src(
        &mut rt,
        "file:///main.mjs",
        r#"console.log("hello from meow")"#,
    )
    .await
    .expect("module runs to completion");
    assert_eq!(*out.borrow(), "hello from meow\n");
}

// console.error routes to the error sink (is_err = true).
#[tokio::test]
async fn console_error_routes_to_stderr_sink() {
    let (out, err, sink_ext) = capture();
    let mut rt = runtime_with(vec![sink_ext]);
    run_src(&mut rt, "file:///main.mjs", r#"console.error("oops")"#)
        .await
        .expect("module runs");
    assert_eq!(*err.borrow(), "oops\n");
    assert!(out.borrow().is_empty());
}

// A test-only op contributed via RuntimeOptions.extensions (proves the seam).
#[op2]
#[string]
fn op_echo(#[string] input: String) -> String {
    input
}

extension!(test_echo, ops = [op_echo]);

// T2 · op round-trip JS↔Rust through the extension seam, no re-embedding of V8.
#[tokio::test]
async fn op_round_trips_through_extension_seam() {
    let (out, _err, sink_ext) = capture();
    let mut rt = runtime_with(vec![sink_ext, test_echo::init()]);
    run_src(
        &mut rt,
        "file:///main.mjs",
        r#"console.log(Deno.core.ops.op_echo("ping-pong"))"#,
    )
    .await
    .expect("module runs");
    assert_eq!(*out.borrow(), "ping-pong\n");
}

// T3 · an uncaught synchronous throw becomes a typed error, not a panic.
#[tokio::test]
async fn uncaught_throw_becomes_typed_error() {
    let mut rt = runtime_with(vec![]);
    let result = run_src(&mut rt, "file:///boom.mjs", "throw new Error(\"boom\")").await;
    match result {
        Err(RuntimeError::Uncaught { report, specifier }) => {
            assert!(
                report.message.contains("boom"),
                "message: {}",
                report.message
            );
            assert!(specifier.contains("boom.mjs"));
            assert!(
                report.file_name.is_some() && report.line_number.is_some(),
                "expected a populated source position, got {report:?}"
            );
        }
        other => panic!("expected RuntimeError::Uncaught, got {other:?}"),
    }
}

// T3 (cont.) · a rejected top-level await also surfaces as Uncaught.
#[tokio::test]
async fn rejected_top_level_await_becomes_typed_error() {
    let mut rt = runtime_with(vec![]);
    let result = run_src(
        &mut rt,
        "file:///reject.mjs",
        "await Promise.reject(new Error(\"boom-await\"))",
    )
    .await;
    match result {
        Err(RuntimeError::Uncaught { report, .. }) => {
            assert!(
                report.message.contains("boom-await"),
                "message: {}",
                report.message
            );
        }
        other => panic!("expected RuntimeError::Uncaught, got {other:?}"),
    }
}

// T3 (cont.) · a syntax error is a module error (load-time), not an Uncaught.
#[tokio::test]
async fn syntax_error_is_a_module_error() {
    let mut rt = runtime_with(vec![]);
    let result = run_src(&mut rt, "file:///bad.mjs", "const = ;").await;
    assert!(
        matches!(result, Err(RuntimeError::Module { .. })),
        "expected RuntimeError::Module, got {result:?}"
    );
}

// T4 · top-level await resolves before the run returns (the loop is driven).
#[tokio::test]
async fn top_level_await_resolves_before_return() {
    let (out, _err, sink_ext) = capture();
    let mut rt = runtime_with(vec![sink_ext]);
    run_src(
        &mut rt,
        "file:///tla.mjs",
        r#"
            await Promise.resolve();
            await new Promise((r) => queueMicrotask(r));
            console.log("after-await");
        "#,
    )
    .await
    .expect("module runs");
    // If the loop were not driven to completion the print would not have landed.
    assert_eq!(*out.borrow(), "after-await\n");
}

// The from-string and from-file paths agree: a file: entry runs too.
#[tokio::test]
async fn file_specifier_runs_from_disk() {
    let (out, _err, sink_ext) = capture();
    let dir = std::env::temp_dir().join(format!("meow-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("entry.mjs");
    std::fs::write(&path, "console.log(\"from-disk\")").unwrap();
    let url = ModuleSpecifier::from_file_path(&path).unwrap();

    let mut rt = runtime_with(vec![sink_ext]);
    rt.run_main_module(&url).await.expect("file module runs");
    assert_eq!(*out.borrow(), "from-disk\n");
    std::fs::remove_dir_all(&dir).ok();
}

// A .ts entry fails honestly (TypeScript needs RT-003), surfaced as a module error.
#[tokio::test]
async fn typescript_entry_fails_honestly() {
    let dir = std::env::temp_dir().join(format!("meow-rt-ts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("entry.ts");
    std::fs::write(&path, "const x: number = 1; console.log(x);").unwrap();
    let url = ModuleSpecifier::from_file_path(&path).unwrap();

    let mut rt = runtime_with(vec![]);
    let result = rt.run_main_module(&url).await;
    match result {
        Err(RuntimeError::Module { source, .. }) => {
            assert!(
                source.to_string().contains("RT-003"),
                "expected the honest TS message, got {source}"
            );
        }
        other => panic!("expected RuntimeError::Module for .ts, got {other:?}"),
    }
    std::fs::remove_dir_all(&dir).ok();
}

// T5 · determinism posture — the crate's src introduces no ambient host reads
// (I-6 floor tripwire; the adversarial form is RT-006).
#[test]
fn no_ambient_host_reads_in_src() {
    // Needles are split via `concat!` so this test file itself does not contain
    // the contiguous banned tokens (the harness `principles-check.sh` greps *.rs).
    const BANNED: &[&str] = &[
        concat!("std::env", "::var"),
        concat!("SystemTime", "::now"),
        concat!("Instant", "::now"),
        concat!("rand", "::"),
    ];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    visit(&src, &mut |path, contents| {
        for pat in BANNED {
            if contents.contains(pat) {
                offenders.push(format!("{}: {pat}", path.display()));
            }
        }
    });
    assert!(
        offenders.is_empty(),
        "ambient host reads found: {offenders:?}"
    );
}

fn visit(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path, &str)) {
    for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, f);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let contents = std::fs::read_to_string(&path).expect("read src file");
            f(&path, &contents);
        }
    }
}
