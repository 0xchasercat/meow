//! The clock/rng/env capability slice of a run (RT-006 · A1).
//!
//! Modelled as **source selection** (deterministic substitute ↔ real host
//! source), deliberately NOT RT-002's binary `CapabilityCheck` allow/deny:
//! denying a clock or RNG outright breaks ordinary programs, so the I-6-correct
//! shape is a deterministic stand-in by default, real on grant. P6 `SEC-001`
//! composes this slice with [`CapabilityCheck`](crate::io::CapabilityCheck) into
//! the one unified grant model; they are two slices of one capability set.
//!
//! [`Default`] is fully deterministic — this is what makes a bare `meow run`
//! reproducible (I-6). A builder flips one field to the real host source.

use std::collections::BTreeSet;

/// Fixed virtual origin so two runs agree (I-6): `2026-06-07T00:00:00Z` in ms.
/// A named constant, never a literal at the callsite — part of the
/// reproducibility contract; changing it changes default output deliberately.
pub const MEOW_VIRTUAL_EPOCH_MS: f64 = 1_780_790_400_000.0;

/// Fixed 32-byte default seed → the default RNG stream is identical across runs
/// and machines. Part of the reproducibility contract (Operator notes).
pub const MEOW_DEFAULT_SEED: [u8; 32] = [0u8; 32];

/// The clock/rng/env slice of a run's capability set. `Default` is fully
/// deterministic: virtual clock, seeded RNG, env invisible. A grant flips one
/// field to the real host source.
#[derive(Debug, Clone)]
pub struct HermeticConfig {
    /// Wall/monotonic time source. Default: [`ClockSource::Virtual`].
    pub clock: ClockSource,
    /// Randomness source backing `Math.random` + `crypto.getRandomValues`.
    /// Default: [`RngSource::Seeded`].
    pub rng: RngSource,
    /// Environment-variable visibility. Default: [`EnvPolicy::Deny`].
    pub env: EnvPolicy,
}

/// Where `Date.now()` / `new Date()` / `performance.now()` read time.
#[derive(Debug, Clone)]
pub enum ClockSource {
    /// Deterministic: starts at `origin_ms`, advances ONLY via the runtime's
    /// timer queue (RT-004 `setTimeout`); frozen at origin until that lands
    /// (honest boundary — a program measuring elapsed wall-time sees zero
    /// progression under the default clock, exactly what the test runner wants).
    Virtual { origin_ms: f64 },
    /// Granted real wall-clock + monotonic time. The ONLY `SystemTime::now`/
    /// `Instant::now` in the workspace (confined to [`super::state`]).
    Real,
}

/// Where `Math.random` + `crypto.getRandomValues` draw bytes.
#[derive(Debug, Clone)]
pub enum RngSource {
    /// Deterministic CSPRNG (ChaCha20) seeded from `seed`.
    Seeded { seed: [u8; 32] },
    /// Granted OS entropy (`getrandom`). The ONLY OS-entropy call in the
    /// workspace (confined to [`super::state`]).
    Os,
}

/// Environment-variable visibility policy.
#[derive(Debug, Clone)]
pub enum EnvPolicy {
    /// Ungranted host env is invisible: every lookup returns `None` (I-6).
    Deny,
    /// Scoped allowlist (§15.1 — grants are scoped, not "all env"): only these
    /// names resolve to the real value; everything else is `None`. The ONLY
    /// `std::env::var` in the workspace (confined to [`super::state`]).
    Allow(BTreeSet<String>),
}

impl Default for HermeticConfig {
    fn default() -> Self {
        Self {
            clock: ClockSource::Virtual {
                origin_ms: MEOW_VIRTUAL_EPOCH_MS,
            },
            rng: RngSource::Seeded {
                seed: MEOW_DEFAULT_SEED,
            },
            env: EnvPolicy::Deny,
        }
    }
}

impl HermeticConfig {
    /// Grant the real system clock (the run is no longer reproducible).
    pub fn with_real_clock(mut self) -> Self {
        self.clock = ClockSource::Real;
        self
    }

    /// Grant OS entropy for `Math.random` + `crypto.getRandomValues` (the run is
    /// no longer reproducible).
    pub fn with_os_rng(mut self) -> Self {
        self.rng = RngSource::Os;
        self
    }

    /// Grant a scoped env allowlist: only `names` resolve to the host value;
    /// every other lookup stays `None`.
    pub fn with_env_allow(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.env = EnvPolicy::Allow(names.into_iter().collect());
        self
    }

    /// Grant ALL host env — the widest grant (flagged loud by the CLI). Snapshots
    /// the host's current variable names (the one host read, confined to
    /// [`super::state::host_env_names`]) into an [`EnvPolicy::Allow`] set, so the
    /// seam stays a single scoped mechanism with no second "all env" code path.
    pub fn with_env_all(mut self) -> Self {
        self.env = EnvPolicy::Allow(super::state::host_env_names());
        self
    }
}
