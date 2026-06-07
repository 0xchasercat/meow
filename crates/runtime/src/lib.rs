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
//!   or randomness reads. The only host boundary is the op seam (at P0, `op_print`
//!   -> stdout/stderr).
//! - **Single-threaded**: [`Runtime`] is not `Send`/`Sync`. Construct and drive
//!   it on one thread inside a current-thread tokio runtime — deno_core requires
//!   an active tokio context at isolate creation and initializes the V8 platform
//!   exactly once per process internally (a guarded `Once`), so repeated
//!   [`Runtime::new`] calls on one thread do not double-init.

mod error;
mod ext;
mod loader;

use deno_core::{JsRuntime, ModuleId, PollEventLoopOptions, RuntimeOptions as DenoRuntimeOptions};

// Re-exports: callers depend on `meow_runtime`, not `deno_core`, directly.
/// The owned V8 edge, re-exported so seam consumers (RT-002, RT-004, …) can build
/// `Extension`s / `#[op2]`s without adding their own `deno_core` dependency.
pub use deno_core;
pub use deno_core::v8;
pub use deno_core::ModuleCodeString;
pub use deno_core::ModuleSpecifier;

pub use error::{JsExceptionReport, RuntimeError};
pub use ext::{print_sink_extension, PrintSink};
pub use loader::TrivialModuleLoader;

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
    pub fn new(options: RuntimeOptions) -> Result<Runtime, RuntimeError> {
        let mut extensions = Vec::with_capacity(options.extensions.len() + 1);
        extensions.push(ext::meow_runtime::init());
        extensions.extend(options.extensions);

        let js_runtime = JsRuntime::try_new(DenoRuntimeOptions {
            module_loader: Some(options.module_loader),
            extensions,
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

    /// The canonical deno_core dance: kick off evaluation, drive the loop, then
    /// await the evaluation result — surfacing whichever fails first as a typed
    /// error. An uncaught top-level throw / TLA rejection propagates through the
    /// event loop; a non-JS loop failure becomes [`RuntimeError::EventLoop`].
    async fn evaluate_to_completion(
        &mut self,
        spec: &ModuleSpecifier,
        id: ModuleId,
    ) -> Result<(), RuntimeError> {
        let specifier = spec.as_str();
        let eval = self.js_runtime.mod_evaluate(id);
        self.js_runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(|err| error::classify_eval_error(specifier, err))?;
        eval.await
            .map_err(|err| error::classify_eval_error(specifier, err))?;
        Ok(())
    }
}
