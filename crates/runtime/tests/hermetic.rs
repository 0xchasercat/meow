//! RT-006 behavior tests: the runtime is hermetic by default (I-6). `Date` /
//! `Math.random` are deterministic and reproducible across runs; ungranted host
//! env is invisible; the `Date` wrapper preserves `Date` semantics; and a grant
//! actually flips the source (the gate is not a no-op).
//!
//! Tests assert observable JS behavior, not plumbing (CRAFT). The host-touching
//! state itself (clock/rng/env) is unit-tested in `src/hermetic/state.rs`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use meow_runtime::hermetic::{extensions, HermeticConfig, RngSource};
use meow_runtime::web::{extensions as web_extensions, NetCaps, WebOptions};
use meow_runtime::{
    print_sink_extension, AllowAll, ModuleSpecifier, PrintSink, Runtime, RuntimeError,
    RuntimeOptions, TrivialModuleLoader,
};

/// Capture sink + the captured buffer (console output; deterministic, no OS).
fn capture() -> (Rc<RefCell<String>>, deno_core::Extension) {
    let out = Rc::new(RefCell::new(String::new()));
    let o = out.clone();
    let sink = PrintSink(Rc::new(move |msg: &str, _is_err: bool| {
        o.borrow_mut().push_str(msg);
    }));
    (out, print_sink_extension(sink))
}

/// A runtime with the hermetic shadows installed under `cfg` + a capture sink.
/// No Web globals (RT-006 does not depend on RT-004); the `crypto`/`performance`
/// rebinds in `hermetic.js` are guarded and simply skip when absent.
fn hermetic_runtime(cfg: HermeticConfig) -> (Rc<RefCell<String>>, Runtime) {
    let (out, sink_ext) = capture();
    let mut exts = extensions(cfg);
    exts.push(sink_ext);
    let rt = Runtime::new(RuntimeOptions {
            module_loader: Rc::new(TrivialModuleLoader::new()),
            extensions: exts,
            max_heap_size: None,
        })
    .expect("runtime initializes with hermetic shadows");
    (out, rt)
}

async fn run_src(rt: &mut Runtime, url: &str, src: &str) -> Result<(), RuntimeError> {
    let spec = ModuleSpecifier::parse(url).expect("valid specifier");
    rt.run_main_module_from_source(&spec, src.to_string()).await
}

/// Probe every ambient nondeterminism source the default config governs.
const PROBE: &str = r#"
    const out = [];
    out.push("now=" + Date.now());
    out.push("iso=" + new Date().toISOString());
    out.push("epoch=" + new Date(0).toISOString());
    for (let i = 0; i < 4; i++) out.push("r=" + Math.random());
    console.log(out.join("\n"));
"#;

// I-6 happy path: identical observable output across two independent runs under
// the default (deterministic) config.
#[tokio::test]
async fn hermetic_reproducible_default() {
    let (out1, mut rt1) = hermetic_runtime(HermeticConfig::default());
    run_src(&mut rt1, "file:///a.js", PROBE)
        .await
        .expect("run 1");

    let (out2, mut rt2) = hermetic_runtime(HermeticConfig::default());
    run_src(&mut rt2, "file:///b.js", PROBE)
        .await
        .expect("run 2");

    let a = out1.borrow().clone();
    let b = out2.borrow().clone();
    assert_eq!(a, b, "deterministic-by-default output must be identical");
    // And it is the FIXED virtual epoch, not whatever the host clock said.
    assert!(
        a.contains("now=1780790400000"),
        "default clock is the virtual epoch, got: {a}"
    );
    assert!(
        a.contains("epoch=1970-01-01T00:00:00.000Z"),
        "arg-form Date is pure/unchanged, got: {a}"
    );
}

// A real grant flips the clock source: under Real the wall clock is live, so it
// is NOT the frozen virtual epoch (proves the gate is not a no-op).
#[tokio::test]
async fn hermetic_real_clock_grant_flips_source() {
    let (out, mut rt) = hermetic_runtime(HermeticConfig::default().with_real_clock());
    run_src(&mut rt, "file:///c.js", "console.log(Date.now());")
        .await
        .expect("run");
    let printed: f64 = out.borrow().trim().parse().expect("Date.now() is a number");
    assert_ne!(
        printed, 1_780_790_400_000.0,
        "the --allow-clock grant must use the live clock, not the virtual epoch"
    );
    assert!(
        printed > 1_735_689_600_000.0,
        "live clock should be a recent timestamp (> 2025), got {printed}"
    );
}

// Ungranted host env is invisible; a scoped grant resolves only the named var.
// The op is reachable from JS directly (no public global yet — process.env is P5).
#[tokio::test]
async fn hermetic_env_invisible_until_granted() {
    let name = "MEOW_RT006_TEST_VAR";
    std::env::set_var(name, "secret-value");

    // Deny (default): the var is invisible.
    let (out, mut rt) = hermetic_runtime(HermeticConfig::default());
    run_src(
        &mut rt,
        "file:///deny.js",
        &format!("console.log(String(Deno.core.ops.op_hermetic_env_get({name:?})));"),
    )
    .await
    .expect("run deny");
    assert_eq!(out.borrow().trim(), "null", "ungranted env is invisible");

    // Granted by name: the real value resolves.
    let cfg = HermeticConfig::default().with_env_allow([name.to_string()]);
    let (out, mut rt) = hermetic_runtime(cfg);
    run_src(
        &mut rt,
        "file:///allow.js",
        &format!("console.log(String(Deno.core.ops.op_hermetic_env_get({name:?})));"),
    )
    .await
    .expect("run allow");
    assert_eq!(out.borrow().trim(), "secret-value", "granted env resolves");

    // A different granted name does not leak this one (scoped, not all-or-nothing).
    let cfg = HermeticConfig::default().with_env_allow(["SOMETHING_ELSE".to_string()]);
    let (out, mut rt) = hermetic_runtime(cfg);
    run_src(
        &mut rt,
        "file:///scoped.js",
        &format!("console.log(String(Deno.core.ops.op_hermetic_env_get({name:?})));"),
    )
    .await
    .expect("run scoped");
    assert_eq!(
        out.borrow().trim(),
        "null",
        "a grant for another name must not leak this var"
    );

    std::env::remove_var(name);
}

// The Date wrapper gates only the clock-reading paths; every other Date behavior
// (arg construction, instanceof, statics) is unchanged.
#[tokio::test]
async fn hermetic_date_wrapper_fidelity() {
    let (out, mut rt) = hermetic_runtime(HermeticConfig::default());
    run_src(
        &mut rt,
        "file:///d.js",
        r#"
        const d = new Date(2020, 0, 2, 3, 4, 5);
        const checks = [
            d.getFullYear() === 2020,                 // arg construction preserved
            d instanceof Date,                        // instanceof still holds
            new Date(0) instanceof Date,
            Date.parse("1970-01-01T00:00:00Z") === 0, // static parse unchanged
            Date.UTC(1970, 0, 1) === 0,               // static UTC unchanged
            Date.now() === 1780790400000,             // only "now" is gated
            typeof new Date().toISOString() === "string",
        ];
        console.log(checks.every(Boolean) ? "ok" : "FAIL:" + JSON.stringify(checks));
        "#,
    )
    .await
    .expect("run");
    assert_eq!(out.borrow().trim(), "ok");
}

// Different RNG sources diverge: the seeded default and a different fixed seed
// produce different Math.random sequences (the seed actually drives the stream).
#[tokio::test]
async fn hermetic_seed_drives_math_random() {
    let probe = "console.log(Math.random());";

    let (a, mut rt_a) = hermetic_runtime(HermeticConfig::default());
    run_src(&mut rt_a, "file:///s1.js", probe).await.expect("a");

    let cfg = HermeticConfig {
        rng: RngSource::Seeded { seed: [7u8; 32] },
        ..HermeticConfig::default()
    };
    let (b, mut rt_b) = hermetic_runtime(cfg);
    run_src(&mut rt_b, "file:///s2.js", probe).await.expect("b");

    assert_ne!(
        a.borrow().trim(),
        b.borrow().trim(),
        "a different seed must produce a different Math.random stream"
    );
}

/// A runtime with RT-004's Web globals (so `crypto` exists) THEN the hermetic
/// shadows appended after — exactly the `meow run` order, so `hermetic.js` rebinds
/// the real `crypto.getRandomValues`.
fn web_hermetic_runtime(cfg: HermeticConfig) -> (Rc<RefCell<String>>, Runtime) {
    let (out, sink_ext) = capture();
    let caps: NetCaps = Arc::new(AllowAll);
    let mut exts = web_extensions(WebOptions {
        caps,
        user_agent: "meow/test".to_string(),
    });
    exts.extend(extensions(cfg));
    exts.push(sink_ext);
    let rt = Runtime::new(RuntimeOptions {
            module_loader: Rc::new(TrivialModuleLoader::new()),
            extensions: exts,
            max_heap_size: None,
        })
    .expect("runtime initializes with web + hermetic");
    (out, rt)
}

// `crypto.getRandomValues` is routed through the hermetic op: deterministic under
// the seeded default (identical across two runs, and it actually fills bytes),
// and real OS entropy under the `--allow-random` grant (so it diverges).
#[tokio::test]
async fn hermetic_crypto_get_random_values_deterministic_default_real_on_grant() {
    let probe = r#"
        const a = new Uint8Array(8);
        crypto.getRandomValues(a);
        console.log(Array.from(a).join(","));
    "#;

    let (o1, mut r1) = web_hermetic_runtime(HermeticConfig::default());
    run_src(&mut r1, "file:///cg1.js", probe).await.expect("1");
    let (o2, mut r2) = web_hermetic_runtime(HermeticConfig::default());
    run_src(&mut r2, "file:///cg2.js", probe).await.expect("2");
    let seeded = o1.borrow().trim().to_string();
    assert_eq!(
        seeded,
        o2.borrow().trim(),
        "seeded crypto.getRandomValues must be reproducible"
    );
    assert_ne!(
        seeded, "0,0,0,0,0,0,0,0",
        "getRandomValues must actually fill the buffer (not a no-op)"
    );

    let (o3, mut r3) = web_hermetic_runtime(HermeticConfig::default().with_os_rng());
    run_src(&mut r3, "file:///cg3.js", probe).await.expect("3");
    assert_ne!(
        o3.borrow().trim(),
        seeded,
        "the --allow-random grant must use OS entropy, not the seeded stream"
    );
}
