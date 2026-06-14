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
mod loader;
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
pub use ext::{http_extension, print_sink_extension, ui_extension, PrintSink};
pub use loader::TrivialModuleLoader;
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
    /// Resolves + fetches modules. At P0 this is [`TrivialModuleLoader`];
    /// LOAD-001 replaces it with the real shared resolver. Required (no implicit
    /// default → no ambient fs authority).
    pub module_loader: std::rc::Rc<dyn deno_core::ModuleLoader>,
    /// Subsystem-contributed ops/extensions. This crate's own `meow_runtime`
    /// extension is always prepended internally; callers never pass it.
    pub extensions: Vec<deno_core::Extension>,
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
        let mut extensions = Vec::with_capacity(options.extensions.len() + 1);
        extensions.push(ext::meow_runtime::init());
        extensions.extend(options.extensions);

        let js_runtime = JsRuntime::try_new(DenoRuntimeOptions {
            module_loader: Some(options.module_loader),
            extensions,
            extension_transpiler: Some(std::rc::Rc::new(|specifier, source| {
                maybe_transpile_source(specifier, source)
            })),
            create_params: Some(
                deno_core::v8::CreateParams::default().heap_limits(0, 4 * 1024 * 1024 * 1024_usize),
            ),
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

    /// Take and clear a `process.exit(code)` request recorded by RT-007's node
    /// bootstrap, if the run triggered one.
    pub fn take_process_exit_code(&mut self) -> Option<i32> {
        crate::node::take_process_exit_code(&self.js_runtime)
    }

    /// The canonical deno_core dance: kick off evaluation, drive the loop, then
    /// await the evaluation result — surfacing whichever fails first as a typed
    /// error. An uncaught top-level throw / TLA rejection propagates through the
    /// event loop; a non-JS loop failure becomes [`RuntimeError::EventLoop`].
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

fn maybe_transpile_source(
    specifier: deno_core::ModuleName,
    source: deno_core::ModuleCodeString,
) -> Result<
    (
        deno_core::ModuleCodeString,
        Option<deno_core::SourceMapData>,
    ),
    deno_error::JsErrorBox,
> {
    use deno_ast::{MediaType, ModuleKind, ParseParams, SourceMapOption};

    let specifier_str = specifier.as_str();
    let should_transpile = specifier_str.starts_with("node:")
        || specifier_str.starts_with("ext:")
        || specifier_str.ends_with(".ts")
        || specifier_str.ends_with(".mts")
        || specifier_str.ends_with(".cts")
        || specifier_str.ends_with(".tsx")
        || specifier_str.ends_with(".jsx");

    if should_transpile {
        let parsed_specifier =
            deno_core::ModuleSpecifier::parse(specifier_str).unwrap_or_else(|_| {
                deno_core::ModuleSpecifier::parse(&format!("file:///{}", specifier_str)).unwrap()
            });

        let media_type = if specifier_str.ends_with(".tsx") {
            MediaType::Tsx
        } else if specifier_str.ends_with(".jsx") {
            MediaType::Jsx
        } else {
            MediaType::TypeScript
        };

        let parsed = deno_ast::parse_module(ParseParams {
            specifier: parsed_specifier,
            text: source.as_str().into(),
            media_type,
            capture_tokens: false,
            scope_analysis: false,
            maybe_syntax: None,
        })
        .map_err(deno_error::JsErrorBox::from_err)?;

        let transpiled = parsed
            .transpile(
                &deno_ast::TranspileOptions {
                    imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
                    ..Default::default()
                },
                &deno_ast::TranspileModuleOptions {
                    module_kind: Some(ModuleKind::Esm),
                },
                &deno_ast::EmitOptions {
                    source_map: SourceMapOption::Separate,
                    inline_sources: true,
                    ..Default::default()
                },
            )
            .map_err(deno_error::JsErrorBox::from_err)?
            .into_source();

        Ok((
            transpiled.text.into(),
            transpiled.source_map.map(|s| s.into_bytes().into()),
        ))
    } else {
        Ok((source, None))
    }
}
