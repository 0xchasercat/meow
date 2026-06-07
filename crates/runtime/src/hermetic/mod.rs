//! Determinism & hermeticity harness (RT-006 · I-6).
//!
//! Makes the runtime **hermetic by default**: V8's `Date`/`Math.random`, the Web
//! `crypto` entropy source (RT-004), and host env are routed through one governed
//! seam where, by default, the clock is a fixed virtual clock, randomness is a
//! seeded stream, and host env is invisible — a grant swaps in the real source.
//! This extends RT-002's seam pattern (a policy object in `OpState`, every host
//! touch through it) from fs/net to the §15.2 categories `system time ·
//! randomness · environment access`. It is also the substrate the test runner
//! (P6) reuses — one mechanism, not a second clock for tests (I-1).
//!
//! Honest boundary (I-11): this closes the *ambient* nondeterminism path; it is
//! NOT a sandbox against *adversarial* code (a program retaining a pre-shadow
//! reference, reaching an unshadowed intrinsic, or using `eval`/FFI can still read
//! the host). That soundness is tier-3 (CANON §24.3, SEC P7). The correct words
//! are "hermetic by default" + "defense-in-depth," never "sandboxed"/"blocked".

mod config;
mod ops;
mod state;

pub use config::{
    ClockSource, EnvPolicy, HermeticConfig, RngSource, MEOW_DEFAULT_SEED, MEOW_VIRTUAL_EPOCH_MS,
};
pub use state::{HermeticError, HermeticState};

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::OpState;

deno_core::extension!(
    meow_hermetic,
    ops = [
        ops::op_hermetic_now_ms,
        ops::op_hermetic_mono_ms,
        ops::op_hermetic_random_fill,
        ops::op_hermetic_env_get,
    ],
    esm_entry_point = "ext:meow_hermetic/hermetic.js",
    esm = [dir "src/hermetic/js", "hermetic.js"],
    // Seed a deterministic state by default so the ops always find one (mirrors
    // `meow_io`'s default `AllowAll`). A grant overrides it via
    // `hermetic_config_extension`, placed AFTER this in the extension list.
    state = |state: &mut OpState| {
        state.put(Rc::new(RefCell::new(HermeticState::new(&HermeticConfig::default()))));
    },
);

/// Build an [`Extension`](deno_core::Extension) that installs a configured
/// [`HermeticState`] into `OpState`, overriding the `meow_hermetic` default.
///
/// Pass it through `RuntimeOptions.extensions` *after* `meow_hermetic::init()` (and
/// after RT-004's Web globals, so the `hermetic.js` shadows rebind the real
/// `Date`/`crypto`). Mirrors [`io_capability_extension`](crate::io_capability_extension):
/// the seam is wired the same way for resource access (fs/net) and source
/// selection (clock/rng/env).
pub fn hermetic_config_extension(cfg: HermeticConfig) -> deno_core::Extension {
    deno_core::Extension {
        name: "meow_hermetic_config",
        op_state_fn: Some(Box::new(move |state: &mut OpState| {
            state.put(Rc::new(RefCell::new(HermeticState::new(&cfg))));
        })),
        ..Default::default()
    }
}

/// Build the ordered hermetic extension list to append to
/// `RuntimeOptions.extensions`: the `meow_hermetic` ops + shadows, then the
/// config override carrying `cfg`. Append this AFTER RT-004's Web globals so the
/// `Date`/`crypto`/`performance` shadows rebind the real ones. The default `cfg`
/// is fully deterministic; grants flip individual sources.
pub fn extensions(cfg: HermeticConfig) -> Vec<deno_core::Extension> {
    vec![meow_hermetic::init(), hermetic_config_extension(cfg)]
}
