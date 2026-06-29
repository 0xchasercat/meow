//! The hermetic ops (RT-006 · A3): the governed edges JS uses for time,
//! entropy, and env. Each borrows the shared [`HermeticState`] out of `OpState`.
//!
//! `OpState` holds `Rc<RefCell<HermeticState>>`, seeded by
//! [`extensions`](super::extensions) / [`hermetic_config_extension`](super::hermetic_config_extension).
//! If a caller wires `meow_hermetic` without a config extension, [`hermetic_state`]
//! installs the fully-deterministic default on first touch — so the ops are
//! deterministic by default and NEVER panic on a missing seed.

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{op2, OpState};

use super::config::HermeticConfig;
use super::state::{HermeticError, HermeticState};

/// Shared, mutable handle stored in `OpState` (the seeded RNG + virtual counter
/// mutate across draws). Type alias keeps the op signatures legible.
pub type SharedHermeticState = Rc<RefCell<HermeticState>>;

/// Get the shared state, installing the deterministic default on first touch so
/// the ops are deterministic-by-default and never panic on a missing seed.
fn hermetic_state(state: &mut OpState) -> SharedHermeticState {
    if let Some(st) = state.try_borrow::<SharedHermeticState>() {
        return st.clone();
    }
    let st: SharedHermeticState =
        Rc::new(RefCell::new(HermeticState::new(&HermeticConfig::default())));
    state.put(st.clone());
    st
}

/// Wall-clock ms for `Date.now()` / `new Date()`.
#[op2(fast)]
pub fn op_hermetic_now_ms(state: &mut OpState) -> f64 {
    hermetic_state(state).borrow().now_ms()
}

/// Monotonic ms for `performance.now()`.
#[op2(fast)]
pub fn op_hermetic_mono_ms(state: &mut OpState) -> f64 {
    hermetic_state(state).borrow().mono_ms()
}

/// Report whether the active config uses the virtual clock and/or seeded RNG.
/// Called once at runtime init by `globalThis.__meowApplyHermeticShadows` to
/// decide whether to shadow `Date`/`performance` (virtual clock) and
/// `Math.random`/`crypto` (seeded RNG). When a real source is granted
/// (`--trust` / `--allow-clock` / `--allow-random`), the corresponding
/// shadows are skipped so V8's native intrinsics run unhindered — no FFI
/// tax in hot loops.
#[op2]
#[serde]
pub fn op_hermetic_status(state: &mut OpState) -> (bool, bool) {
    let st = hermetic_state(state);
    let st = st.borrow();
    (st.is_virtual_clock(), st.is_seeded_rng())
}

/// Fill `buf` with bytes from the active randomness source. Backs
/// `crypto.getRandomValues` and the one-time `Math.random` seed draw.
#[op2(fast)]
pub fn op_hermetic_random_fill(
    state: &mut OpState,
    #[buffer] buf: &mut [u8],
) -> Result<(), HermeticError> {
    hermetic_state(state).borrow_mut().fill_random(buf)
}

/// Read an env var through the policy. Deny -> `null`; Allow -> the real value iff
/// `name` is in the scoped allowlist.
#[op2]
#[string]
pub fn op_hermetic_env_get(state: &mut OpState, #[string] name: &str) -> Option<String> {
    hermetic_state(state).borrow().env_get(name)
}

/// Snapshot the visible env through the policy. Deny -> empty list; Allow -> the
/// scoped visible subset, key-sorted.
#[op2]
#[serde]
pub fn op_hermetic_env_entries(state: &mut OpState) -> Vec<(String, String)> {
    hermetic_state(state).borrow().env_entries()
}
