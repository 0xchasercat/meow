//! Live per-run hermetic state — the single home of every host read (RT-006 · A2).
//!
//! `HermeticState` is built once from a [`HermeticConfig`] and stored in
//! `OpState`. It holds the live virtual-clock counter and the seeded RNG instance
//! (so successive draws advance one deterministic stream).
//!
//! THIS FILE IS THE SOLE LOCATION IN THE ENTIRE WORKSPACE where `SystemTime::now`,
//! `Instant::now`, `getrandom`, and `std::env::var(s)` may appear
//! (`principles-check.sh` P16 + the `runtime.rs` floor test allowlist `/hermetic/`).
//! Keep it small and obviously the seam.

use std::collections::BTreeSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};

use super::config::{ClockSource, EnvPolicy, HermeticConfig, RngSource};

/// Typed failure surface for the hermetic ops. Implements `deno_error::JsError`
/// so it crosses into JS as a thrown error (never a Rust panic — CRAFT). Only
/// reachable under [`RngSource::Os`]; the deterministic default cannot fail.
#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum HermeticError {
    /// The OS entropy source was unavailable (granted `Os` rng only).
    #[class(generic)]
    #[error("entropy source unavailable: {0}")]
    Entropy(String),
}

/// The live clock counter. `Real` captures its monotonic base once at
/// construction; wall time is read fresh per call so the granted clock tracks the
/// system clock.
enum ClockState {
    Virtual { origin_ms: f64, advanced_ms: f64 },
    Real { mono_base: Instant },
}

/// The live randomness source. The seeded CSPRNG is boxed so the enum carries no
/// large inline variant (clippy `large_enum_variant`); successive draws advance
/// the one stream.
enum RngState {
    Seeded(Box<ChaCha20Rng>),
    Os,
}

/// Built once from [`HermeticConfig`], stored in `OpState`. The four accessors
/// below are the only ways JS obtains time / entropy / env — the I-6 single
/// governed entry.
pub struct HermeticState {
    clock: ClockState,
    rng: RngState,
    env: EnvPolicy,
}

impl HermeticState {
    /// Seeds `ChaCha20Rng` from the configured seed (Seeded) and captures the
    /// monotonic base once (Real).
    pub fn new(cfg: &HermeticConfig) -> Self {
        let clock = match cfg.clock {
            ClockSource::Virtual { origin_ms } => ClockState::Virtual {
                origin_ms,
                advanced_ms: 0.0,
            },
            ClockSource::Real => ClockState::Real {
                mono_base: Instant::now(),
            },
        };
        let rng = match &cfg.rng {
            RngSource::Seeded { seed } => RngState::Seeded(Box::new(ChaCha20Rng::from_seed(*seed))),
            RngSource::Os => RngState::Os,
        };
        Self {
            clock,
            rng,
            env: cfg.env.clone(),
        }
    }

    /// Wall-clock ms for `Date.now()` / `new Date()`. Virtual → `origin + advanced`
    /// (frozen at origin until the timer queue advances it). Real → `SystemTime`
    /// epoch ms (read fresh).
    pub fn now_ms(&self) -> f64 {
        match self.clock {
            ClockState::Virtual {
                origin_ms,
                advanced_ms,
            } => origin_ms + advanced_ms,
            ClockState::Real { .. } => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs_f64() * 1_000.0)
                // A pre-epoch system clock is the only failure; report 0 rather
                // than panic (never panics — CRAFT).
                .unwrap_or(0.0),
        }
    }

    /// Monotonic ms for `performance.now()`. Virtual → the advanced counter
    /// (zero until the timer queue advances it). Real → elapsed since the base.
    pub fn mono_ms(&self) -> f64 {
        match self.clock {
            ClockState::Virtual { advanced_ms, .. } => advanced_ms,
            ClockState::Real { mono_base } => mono_base.elapsed().as_secs_f64() * 1_000.0,
        }
    }

    /// Advance the virtual clock (the timer queue calls this when a timer fires;
    /// no-op under `Real`). The I-6 hook RT-004 wires.
    pub fn advance_virtual_ms(&mut self, delta_ms: f64) {
        if let ClockState::Virtual { advanced_ms, .. } = &mut self.clock {
            *advanced_ms += delta_ms;
        }
    }

    /// Fill `buf`. Seeded → next bytes of the ChaCha stream (deterministic).
    /// Os → `getrandom` (typed error on failure, never a panic).
    pub fn fill_random(&mut self, buf: &mut [u8]) -> Result<(), HermeticError> {
        match &mut self.rng {
            RngState::Seeded(rng) => {
                rng.fill_bytes(buf);
                Ok(())
            }
            RngState::Os => getrandom::fill(buf).map_err(|e| HermeticError::Entropy(e.to_string())),
        }
    }

    /// Env lookup. Deny → `None`. Allow(set) → `Some(value)` iff `name ∈ set` and
    /// the host has it; else `None` (scoped, not all-or-nothing).
    pub fn env_get(&self, name: &str) -> Option<String> {
        match &self.env {
            EnvPolicy::Deny => None,
            EnvPolicy::Allow(set) => {
                if set.contains(name) {
                    std::env::var(name).ok()
                } else {
                    None
                }
            }
        }
    }
}

/// Snapshot the host's current env-variable names (the widest `--allow-env`
/// grant). One of the sanctioned host reads, confined to this file.
pub fn host_env_names() -> BTreeSet<String> {
    std::env::vars().map(|(name, _)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hermetic::config::MEOW_DEFAULT_SEED;

    // The default seeded stream is reproducible: same seed → same bytes; a
    // different seed diverges. Proves the seed actually drives the RNG (I-6).
    #[test]
    fn seeded_stream_is_reproducible_and_seed_driven() {
        let cfg = HermeticConfig::default();
        let mut a = HermeticState::new(&cfg);
        let mut b = HermeticState::new(&cfg);
        let (mut ba, mut bb) = ([0u8; 32], [0u8; 32]);
        a.fill_random(&mut ba).unwrap();
        b.fill_random(&mut bb).unwrap();
        assert_eq!(ba, bb, "same seed must yield the same stream");

        let mut seed = MEOW_DEFAULT_SEED;
        seed[0] = 1;
        let mut c = HermeticState::new(&HermeticConfig {
            rng: RngSource::Seeded { seed },
            ..HermeticConfig::default()
        });
        let mut bc = [0u8; 32];
        c.fill_random(&mut bc).unwrap();
        assert_ne!(ba, bc, "a different seed must diverge");
    }

    // The virtual clock is frozen at origin until advanced, then tracks the
    // accumulated delta (the RT-004 timer hook).
    #[test]
    fn virtual_clock_frozen_until_advanced() {
        let mut st = HermeticState::new(&HermeticConfig::default());
        let origin = st.now_ms();
        assert_eq!(st.now_ms(), origin, "frozen at origin");
        assert_eq!(st.mono_ms(), 0.0, "monotonic starts at zero");
        st.advance_virtual_ms(1_500.0);
        assert_eq!(st.now_ms(), origin + 1_500.0);
        assert_eq!(st.mono_ms(), 1_500.0);
    }

    // Env is invisible under Deny; scoped under Allow.
    #[test]
    fn env_policy_scopes_reads() {
        std::env::set_var("MEOW_RT006_UNIT", "yes");
        let deny = HermeticState::new(&HermeticConfig::default());
        assert_eq!(deny.env_get("MEOW_RT006_UNIT"), None);

        let allow = HermeticState::new(
            &HermeticConfig::default().with_env_allow(["MEOW_RT006_UNIT".to_string()]),
        );
        assert_eq!(allow.env_get("MEOW_RT006_UNIT"), Some("yes".to_string()));
        // A name outside the allowlist stays invisible.
        assert_eq!(allow.env_get("PATH"), None);
        std::env::remove_var("MEOW_RT006_UNIT");
    }
}
