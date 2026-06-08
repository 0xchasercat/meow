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
//!
//! Timezone/locale (craft-7 M3, CLOSED): the virtual clock fixes the clock *value*,
//! and [`pin_deterministic_intl`] additionally pins `TZ=UTC` + a fixed default
//! locale before the V8 isolate is created, so `Date.prototype.toString`/`getHours`/
//! `getTimezoneOffset` and locale-default `Intl`/`toLocaleString` render identically
//! across host timezones + locales. Explicit-locale APIs (`Intl.NumberFormat("de-DE")`)
//! are unaffected; `--allow-clock` opts into the real host environment (time + TZ +
//! locale). The two-machine determinism claim now covers Date/Intl rendering.

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
        ops::op_hermetic_env_entries,
    ],
    esm_entry_point = "ext:meow_hermetic/hermetic.js",
    esm = [dir "src/hermetic/js", "hermetic.js"],
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

/// Pin the process timezone + default locale so V8 renders `Date`
/// (`toString`/`getHours`/`getTimezoneOffset`) and `Intl` (`toLocaleString`,
/// `Intl.*` with no explicit locale) deterministically under the virtual clock —
/// closing the TZ/locale determinism residual (craft-7 M3). Without it, two
/// machines in different host timezones/locales produce different output even with
/// the clock value fixed (V8/ICU read `TZ` + `LC_ALL`/`LANG`).
///
/// MUST be called BEFORE the V8 isolate is created (ICU reads these at first use).
/// No-op under the real clock — a `--allow-clock` grant deliberately opts into the
/// real host environment (real time, timezone, and locale). Explicit-locale APIs
/// (`Intl.NumberFormat("de-DE")`) are unaffected either way. The harness's one
/// process-global host write, kept in the seam; a write, not an ambient *read*, so
/// determinism holds.
pub fn pin_deterministic_intl(cfg: &HermeticConfig) {
    if matches!(cfg.clock, ClockSource::Virtual { .. }) {
        std::env::set_var("TZ", "UTC");
        std::env::set_var("LC_ALL", "en_US.UTF-8");
        std::env::set_var("LANG", "en_US.UTF-8");
    }
}
