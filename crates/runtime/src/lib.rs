//! `meow-runtime` — the single V8 embedding chokepoint (RT-001, ADR-1, CANON §6.2).
//!
//! Owns one [`deno_core::JsRuntime`] (isolate + event loop) and the op/extension
//! seam. No other crate touches `deno_core`/`rusty_v8`; consumers reach V8 types
//! through the re-exports here ([`v8`], [`ModuleSpecifier`], [`deno_core`]). This
//! is what makes the `footprint` gate (I-10) a single-edge check.
//!
//! Invariants:
//! - **Never panics on user JS** (CRAFT Part B): every failure reachable from
//!   user input is a typed [`RuntimeError`]; `unwrap`/`expect` are reserved for
//!   internal invariants only.
//! - **No ambient host reads** (I-6): the crate introduces no ambient env, clock,
//!   or randomness reads. The only host boundary is the op seam (`console`/print
//!   by default, plus explicitly-installed `meow:*` host extensions).
//! - **Single-threaded**: [`Runtime`] is not `Send`/`Sync`. Construct and drive
//!   it on one thread inside a current-thread tokio runtime — deno_core requires
//!   an active tokio context at isolate creation and initializes the V8 platform
//!   exactly once per process internally (a guarded `Once`), so repeated
//!   [`Runtime::new`] calls on one thread do not double-init.

mod error;
mod ext;
mod fs_events;
pub mod native;
pub mod typegen;
// === RT-005 ===
/// Host-access edge for `meow types` typegen (the one env-reading spot in this
/// crate outside `hermetic/`; P16 + the floor test allowlist `/host/`).
pub mod host;
// === /RT-005 ===

// === RT-002 ===
/// Async host-I/O layer: tokio-backed ops + the capability seam (ADR-9, I-6).
pub mod io;
// === RT-002 ===

// === RT-004 ===
/// strict-web Stateless-Edge globals (CANON §8.1): the WHATWG web extensions
/// (fetch/URL/crypto.subtle/TextEncoder/…) wired onto the default runtime. See
/// [`web::extensions`]. `console` is NOT installed here (RT-001 owns it).
pub mod web;
// === /RT-004 ===

// === RT-006 ===
/// Determinism & hermeticity harness (CANON §1 #5 / I-6): routes V8's
/// `Date`/`Math.random`, the Web `crypto` entropy, and host env through one
/// governed seam — deterministic by default, real host source on grant. See
/// [`hermetic::extensions`]. The sole sanctioned home of host clock/entropy/env
/// reads (`principles-check.sh` P16 allowlists `src/hermetic/`).
pub mod hermetic;
// === /RT-006 ===
// === RT-007 ===
/// Native Node built-in bootstrap (`process` / `Buffer`) + the default
/// node-compat op wiring (`fs`, `cwd`, strict-web withdrawal policy). See
/// [`node::extensions`].
pub mod node;
// === /RT-007 ===

use deno_core::{JsRuntime, ModuleId, PollEventLoopOptions, RuntimeOptions as DenoRuntimeOptions};

// Re-exports: callers depend on `meow_runtime`, not `deno_core`, directly.
/// The owned V8 edge, re-exported so seam consumers (RT-002, RT-004, …) can build
/// `Extension`s / `#[op2]`s without adding their own `deno_core` dependency.
pub use deno_core;
pub use deno_core::v8;
pub use deno_core::ModuleCodeString;
pub use deno_core::ModuleSpecifier;

pub use error::{JsExceptionReport, RuntimeError};
pub use ext::http::ops::HttpError;
pub use ext::meow_runtime;
pub use ext::{http_extension, print_sink_extension, test_extension, ui_extension, PrintSink};
// === RT-002 ===
pub use io::{
    io_capability_extension, AllowAll, CapDenied, CapRequest, CapabilityCheck, RuntimeIoError,
    TcpStreamResource,
};
// === RT-002 ===

/// One V8 isolate + its event loop. Owns the embedding so no other crate touches
/// `deno_core`. NOT `Send`/`Sync`: drive on a single thread (see crate docs).
pub struct Runtime {
    js_runtime: JsRuntime,
}

/// Construction inputs. Deliberately minimal at P0 — `extensions` is the
/// registration seam (A4).
pub struct RuntimeOptions {
    /// Resolves + fetches modules. Required explicitly so tests and production
    /// exercise the same resolver path and no ambient fs authority is implied.
    pub module_loader: std::rc::Rc<dyn deno_core::ModuleLoader>,
    /// Subsystem-contributed ops/extensions. This crate's own `meow_runtime`
    /// extension is always prepended internally; callers never pass it.
    pub extensions: Vec<deno_core::Extension>,
    /// V8 heap limit in bytes. When `None`, the default is 4 GiB or 75% of
    /// available system memory, whichever is larger. Set via
    /// `--max-old-space-size=<MiB>` on `meow run` / `meow dev`.
    pub max_heap_size: Option<usize>,
    /// Optional pre-built V8 startup snapshot. When present, the runtime
    /// initializes from this blob instead of running all extension JS sources
    /// from scratch. Produces dramatically faster startup.
    ///
    /// The blob is produced by the `meow-snapshot` tool (or a `--create-snapshot`
    /// build path) and embedded at compile time. The `residual_lazy_*` lists
    /// cover any lazy-loaded JS/ESM modules that were NOT reached during
    /// snapshot creation and therefore need fresh source at runtime.
    pub startup_snapshot: Option<&'static [u8]>,
    /// Residual `lazy_loaded_js` sources not captured in the snapshot.
    /// Each entry is `(specifier, source_code)`. Only consulted when
    /// `startup_snapshot` is `Some`.
    pub residual_lazy_js_sources: &'static [(&'static str, &'static str)],
    /// Residual `lazy_loaded_esm` sources not captured in the snapshot.
    /// Each entry is `(specifier, source_code)`. Only consulted when
    /// `startup_snapshot` is `Some`.
    pub residual_lazy_esm_sources: &'static [(&'static str, &'static str)],
    /// Raw V8 flags forwarded to `v8::V8::set_flags_from_command_line` before
    /// the isolate is created (e.g. `--allow-natives-syntax`, `--trace-opt`).
    /// Comma- or whitespace-separated. Owned so the caller doesn't have to
    /// leak or static-promote the input.
    pub v8_flags: Option<String>,
}
/// Default V8 heap size: 4 GiB or 75% of available system memory,
/// like Next.js dev while staying reasonable for small scripts.
fn default_heap_size() -> usize {
    let four_gib = 4 * 1024 * 1024 * 1024_usize;
    system_memory()
        .map(|total| (total * 75) / 100)
        .unwrap_or(four_gib)
        .max(four_gib)
}
/// Returns total system memory in bytes, or `None` if unavailable.
#[cfg(target_os = "macos")]
fn system_memory() -> Option<usize> {
    use std::ffi::CString;
    let Ok(name) = CString::new("hw.memsize") else {
        return None;
    };
    let mut size: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut size as *mut _ as *mut _,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Some(size as usize)
    } else {
        None
    }
}
#[cfg(target_os = "linux")]
fn system_memory() -> Option<usize> {
    let info = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if info > 0 && page_size > 0 {
        Some((info as usize) * (page_size as usize))
    } else {
        None
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_memory() -> Option<usize> {
    None
}

#[cfg(unix)]
fn maximize_fd_limit() {
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
            limit.rlim_cur = limit.rlim_max.min(10240);
            let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
        }
    }
}

#[cfg(not(unix))]
fn maximize_fd_limit() {}

/// Apply raw V8 engine flags before any isolate is created.
/// Calls `v8::V8::set_flags_from_command_line` which is a process-global
/// init point — must be invoked before any `JsRuntime` is constructed.
fn apply_v8_flags(raw: &str) {
    // Split on both commas and whitespace so users can use either separator:
    //   --v8-flags=--allow-natives-syntax,--trace-opt
    //   --v8-flags="--allow-natives-syntax --trace-opt"
    let joined = raw.replace(',', " ");
    let mut args: Vec<String> = vec!["meow".to_string()];
    args.extend(joined.split_whitespace().map(|s| s.to_string()));
    v8::V8::set_flags_from_command_line(args);
}

impl Runtime {
    /// Creates the isolate, prepending this crate's `meow_runtime` extension
    /// (ops + `console` bootstrap) to `options.extensions`. No host reads.
    ///
    /// The default extension set is intentionally **safe-by-default**: only the
    /// `meow_runtime` console/print bootstrap is installed. The raw host-I/O ops
    /// ([`io::meow_io`]) are NOT installed here, so a default runtime (what
    /// `meow run` uses) cannot reach `op_read_file` / `op_tcp_connect` from user
    /// JS — there is no ambient host FS/network authority (I-6, I-8). Callers
    /// that need host I/O opt in explicitly by passing [`io::meow_io::init`]
    /// (together with [`io::io_capability_extension`] for a non-default policy)
    /// through `options.extensions`. Those raw ops are unmediated internal
    /// plumbing; they only go live behind a mediated, capability-enforced API in
    /// a later spec (meow:fs + SEC/P6).
    pub fn new(options: RuntimeOptions) -> Result<Runtime, RuntimeError> {
        maximize_fd_limit();
        if let Some(raw) = options.v8_flags.as_deref() {
            apply_v8_flags(raw);
        }
        let mut extensions = Vec::with_capacity(options.extensions.len() + 1);
        extensions.push(ext::meow_runtime::init());
        extensions.extend(options.extensions);
        let heap_limit = options.max_heap_size.unwrap_or_else(default_heap_size);
        let js_runtime = JsRuntime::try_new(DenoRuntimeOptions {
            module_loader: Some(options.module_loader),
            extensions,
            extension_transpiler: Some(std::rc::Rc::new(|specifier, source| {
                maybe_transpile_source(specifier, source)
            })),
            create_params: Some(deno_core::v8::CreateParams::default().heap_limits(0, heap_limit)),
            startup_snapshot: options.startup_snapshot,
            residual_lazy_js_sources: options.residual_lazy_js_sources,
            residual_lazy_esm_sources: options.residual_lazy_esm_sources,
            ..Default::default()
        })
        .map_err(|err| RuntimeError::Init(err.to_string()))?;

        Ok(Runtime { js_runtime })
    }
    /// Non-module script eval; returns the completion value. For bootstrap
    /// snippets and tests. `name` is the synthetic source name in stack traces.
    pub fn execute_script(
        &mut self,
        name: &'static str,
        src: impl Into<ModuleCodeString>,
    ) -> Result<v8::Global<v8::Value>, RuntimeError> {
        self.js_runtime
            .execute_script(name, src.into())
            .map_err(|err| error::uncaught_from_js(name, &err))
    }

    /// Read test results stored by the JS test runner via the op seam.
    /// Returns `None` if no test results were stored (no test file ran).
    pub fn take_test_results(&mut self) -> Option<String> {
        let op_state = self.js_runtime.op_state();
        let state = op_state.borrow();
        state
            .try_borrow::<crate::ext::test::TestResults>()
            .and_then(|results| results.0.borrow_mut().take())
    }
    /// Loads `spec` as the main ESM module via the configured loader, evaluates
    /// it, and drives the event loop to completion — resolving top-level await
    /// and draining the microtask + macrotask queues. Uncaught exceptions become
    /// [`RuntimeError::Uncaught`] (no panic).
    pub async fn run_main_module(&mut self, spec: &ModuleSpecifier) -> Result<(), RuntimeError> {
        let id = self
            .js_runtime
            .load_main_es_module(spec)
            .await
            .map_err(|source| RuntimeError::Module {
                specifier: spec.to_string(),
                source: Box::new(source),
            })?;
        self.evaluate_to_completion(spec, id).await
    }

    /// Like [`run_main_module`](Self::run_main_module) but the source is supplied
    /// directly (no loader fetch) — the "evaluate an ESM module from a string"
    /// path. Relative imports inside the source still go through the loader.
    pub async fn run_main_module_from_source(
        &mut self,
        spec: &ModuleSpecifier,
        src: impl Into<ModuleCodeString>,
    ) -> Result<(), RuntimeError> {
        let id = self
            .js_runtime
            .load_main_es_module_from_code(spec, src.into())
            .await
            .map_err(|source| RuntimeError::Module {
                specifier: spec.to_string(),
                source: Box::new(source),
            })?;
        self.evaluate_to_completion(spec, id).await
    }

    /// Pumps the event loop until there is no more pending work. Public so
    /// RT-002 can interleave async I/O.
    pub async fn run_event_loop(&mut self) -> Result<(), RuntimeError> {
        self.js_runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|err| RuntimeError::EventLoop(Box::new(err)))
    }

    /// The runtime's shared op-state. Used to install per-worker channel state
    /// for the cooperative-isolate `node:worker_threads` host (the worker driver
    /// inserts the worker side's message queues + serialized `workerData`).
    pub fn op_state(&self) -> std::rc::Rc<std::cell::RefCell<deno_core::OpState>> {
        self.js_runtime.op_state()
    }

    /// A thread-safe handle to this runtime's V8 isolate. The cooperative worker
    /// host calls `terminate_execution()` on a worker's handle to stop it without
    /// borrowing the worker's single-owner `JsRuntime` (which its own driver task
    /// holds across `run_event_loop().await`).
    pub fn isolate_handle(&mut self) -> deno_core::v8::IsolateHandle {
        self.js_runtime.v8_isolate().thread_safe_handle()
    }

    /// Take and clear a `process.exit(code)` request recorded by RT-007's node
    /// bootstrap, if the run triggered one.
    pub fn take_process_exit_code(&mut self) -> Option<i32> {
        crate::node::take_process_exit_code(&self.js_runtime)
    }
    /// Refresh the Node bootstrap state after loading from a snapshot.
    /// The snapshot bakes in placeholder argv/cwd/env; this updates them
    /// to the real values for this invocation.
    pub fn refresh_node_bootstrap(
        &mut self,
        argv: Vec<String>,
        main_module: Option<String>,
        cwd: std::path::PathBuf,
        env: std::collections::BTreeMap<String, String>,
    ) -> Result<(), RuntimeError> {
        crate::node::refresh_bootstrap_state(&mut self.js_runtime, argv, main_module, cwd, env)
    }

    /// Apply hermetic global shadows (`Date` / `Math.random` / `performance` /
    /// `crypto`) if the active [`hermetic::HermeticConfig`] requires them.
    ///
    /// This MUST be called after [`Runtime::new`] (whether from a snapshot or
    /// fresh) for the hermetic extension to take effect. The shadow logic lives
    /// in `globalThis.__meowApplyHermeticShadows` (defined by `hermetic.js` at
    /// module-eval time, so it survives snapshot restore) and is invoked here
    /// at runtime so it reads the *runtime* config via `op_hermetic_status`,
    /// not the snapshot-creation config.
    ///
    /// Under `--trust` / `--allow-clock` / `--allow-random`, the corresponding
    /// shadows are skipped and V8's native intrinsics run unhindered -- no FFI
    /// tax in hot loops. If the hermetic extension is not installed, this is a
    /// no-op (the global is absent).
    pub fn apply_hermetic_shadows(&mut self) -> Result<(), RuntimeError> {
        // Fast path: if both the clock and RNG are real (e.g. --trust or
        // node-compat mode), no shadows are needed. Skip the execute_script
        // entirely — saves ~1-2ms of JS compile+eval on every run.
        let op_state = self.js_runtime.op_state();
        let needed = {
            let state = op_state.borrow();
            crate::hermetic::shadows_needed(&state)
        };
        if !needed {
            return Ok(());
        }
        self.execute_script(
            "hermetic_apply",
            String::from("if (typeof globalThis.__meowApplyHermeticShadows === 'function') globalThis.__meowApplyHermeticShadows();"),
        )?;
        Ok(())
    }

    /// The canonical deno_core dance: kick off evaluation, pump the event
    /// loop until the module resolves, then continue pumping for any
    /// lingering server/async work. An uncaught top-level throw / TLA
    /// rejection propagates through the event loop; a non-JS loop failure
    /// becomes [`RuntimeError::EventLoop`].
    async fn evaluate_to_completion(
        &mut self,
        spec: &ModuleSpecifier,
        id: ModuleId,
    ) -> Result<(), RuntimeError> {
        let specifier = spec.as_str();
        let mut eval = Box::pin(self.js_runtime.mod_evaluate(id));
        // Phase 1: pump until module evaluation finishes
        loop {
            tokio::select! {
                result = &mut eval => {
                    result.map_err(|err| error::classify_eval_error(specifier, err))?;
                    break;
                }
                result = self.js_runtime.run_event_loop(PollEventLoopOptions::default()) => {
                    result.map_err(|err| error::classify_eval_error(specifier, err))?;
                }
            }
        }
        // Phase 2: keep pumping for server/async handles
        self.js_runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|err| error::classify_eval_error(specifier, err))
    }
}

pub fn maybe_transpile_source(
    specifier: deno_core::ModuleName,
    source: deno_core::ModuleCodeString,
) -> Result<
    (
        deno_core::ModuleCodeString,
        Option<deno_core::SourceMapData>,
    ),
    deno_error::JsErrorBox,
> {
    use oxc_allocator::Allocator;
    use oxc_codegen::Codegen;
    use oxc_parser::{ParseOptions, Parser};
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;
    use oxc_transformer::{TransformOptions, Transformer};

    let specifier_str = specifier.as_str();
    let should_transpile = specifier_str.starts_with("node:")
        || specifier_str.starts_with("ext:")
        || specifier_str.ends_with(".ts")
        || specifier_str.ends_with(".mts")
        || specifier_str.ends_with(".cts")
        || specifier_str.ends_with(".tsx")
        || specifier_str.ends_with(".jsx");

    if !should_transpile {
        return Ok((source, None));
    }

    let source_text = source.as_str();
    let source_path = source_path_for_oxc(specifier_str);
    let source_type = SourceType::from_path(&source_path)
        .unwrap_or_else(|_| source_type_from_specifier(specifier_str));
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source_text, source_type)
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        let message = parsed
            .diagnostics
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(deno_error::JsErrorBox::generic(format!(
            "Oxc parse failed for {specifier_str}: {message}"
        )));
    }

    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        let message = semantic
            .diagnostics
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(deno_error::JsErrorBox::generic(format!(
            "Oxc semantic analysis failed for {specifier_str}: {message}"
        )));
    }

    let transform_options = TransformOptions {
        typescript: oxc_transformer::TypeScriptOptions {
            only_remove_type_imports: true,
            ..oxc_transformer::TypeScriptOptions::default()
        },
        ..TransformOptions::default()
    };
    let transformed = Transformer::new(&allocator, &source_path, &transform_options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if !transformed.diagnostics.is_empty() {
        let message = transformed
            .diagnostics
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(deno_error::JsErrorBox::generic(format!(
            "Oxc transform failed for {specifier_str}: {message}"
        )));
    }

    let code = Codegen::new().build(&program).code;
    Ok((code.into(), None))
}

fn source_path_for_oxc(specifier: &str) -> std::path::PathBuf {
    if let Ok(url) = deno_core::ModuleSpecifier::parse(specifier) {
        if let Ok(path) = url.to_file_path() {
            return path;
        }
        let path = url.path();
        if let Some(name) = path.rsplit('/').next().filter(|name| !name.is_empty()) {
            return std::path::PathBuf::from(name);
        }
    }
    std::path::PathBuf::from(specifier.rsplit('/').next().unwrap_or(specifier))
}

fn source_type_from_specifier(specifier: &str) -> oxc_span::SourceType {
    if specifier.ends_with(".tsx") {
        oxc_span::SourceType::tsx()
    } else if specifier.ends_with(".jsx") {
        oxc_span::SourceType::jsx()
    } else if specifier.ends_with(".mjs") {
        oxc_span::SourceType::mjs()
    } else if specifier.ends_with(".cjs") {
        oxc_span::SourceType::cjs()
    } else {
        oxc_span::SourceType::ts()
    }
}
