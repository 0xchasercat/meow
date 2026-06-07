//! `meow-config` — the single human-edited config source for a `meow` project.
//!
//! CANON §18 / principle 11 / ADR-8: `meow` collapses the per-tool config graveyard
//! into one source. CFG-001 stands up:
//!
//! - the typed [`MeowConfig`] shape of `defineMeow({...})` (a serde model), and
//! - `tsconfig` *shadow* generation — [`generate_shadow_tsconfig`] writes the
//!   gitignored `.meow/tsconfig.json` the delegated typechecker (ADR-5) consumes,
//!   and [`write_root_tsconfig_shim`] writes the committed one-line root
//!   `tsconfig.json` `extends` shim every legacy tool reads (I-1).
//!
//! [`MeowConfig::load`] is the HONEST pre-runtime boundary (I-11): it reads the
//! static `meow.config.json`; evaluating a real `meow.config.ts` needs the runtime
//! (RT-001) and is reported, never faked.

mod load;
mod schema;
mod shadow;

pub use load::ConfigError;
pub use schema::{
    Clock, Format, FormatStyle, Install, InstallMode, Lint, MeowConfig, Mode, Network, Permissions,
    Runtime, Severity, TestConfig, TsHandling, Types, Workspace,
};
pub use shadow::{
    generate_shadow_tsconfig, write_root_tsconfig_shim, write_shadow_types, GENERATED_HEADER,
    ROOT_TSCONFIG_SHIM, STRICT_WEB_DTS_FILE,
};
