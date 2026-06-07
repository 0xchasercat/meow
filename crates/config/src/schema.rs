//! The typed shape of `meow.config.ts` (CANON §18).
//!
//! Mirrors the `defineMeow({...})` object exactly. Every nested struct carries a
//! `Default` so an absent key takes the CANON §18 default (`mode: strict-web`,
//! `install.mode: pnp`, `types.strict: true`, `test.isolate/clock=deterministic/
//! network=fake`, `format.style: meow`). `deny_unknown_fields` makes an unknown
//! config key a typed parse error rather than a silent ignore (I-9 spirit — config
//! is one closed schema). Maps use `BTreeMap` so serialization is deterministically
//! key-sorted, which feeds byte-stable shadow regeneration.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The typed shape of `meow.config.ts` (CANON §18). Mirrors the `defineMeow({...})`
/// object exactly; `runtime.engine` is deliberately absent (ADR-1 — V8 non-swappable).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MeowConfig {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub workspace: Workspace,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub install: Install,
    #[serde(default)]
    pub lint: Lint,
    #[serde(default)]
    pub format: Format,
    #[serde(default)]
    pub types: Types,
    #[serde(default)]
    pub test: TestConfig,
    #[serde(default)]
    pub permissions: Permissions,
    // === CFG-002 ===
    /// Publishing metadata projected into the generated root `package.json` (ADR-8).
    /// `meow.config.ts` is the SOLE authority for that file; package.json is derived.
    #[serde(default)]
    pub publish: Publish,
    // === /CFG-002 ===
}

// === CFG-002 ===
/// Publishing metadata projected into the generated root `package.json` (ADR-8).
/// `meow.config.ts` is the SOLE authority for that file; `package.json` is derived,
/// owned, and overwritten by `meow` (it has no `extends`, so the shim trick used
/// for `tsconfig` cannot apply — CANON §18.1).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Publish {
    /// → `package.json.name`. Absent ⇒ field omitted (a config may be private/unnamed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// → `package.json.version`. Absent ⇒ omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// → `package.json.description`. Absent ⇒ omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// → `package.json.license`. Absent ⇒ omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// → `package.json.private`, emitted only when `true`.
    #[serde(default)]
    pub private: bool,
    /// Subpath → target, projected verbatim into `package.json.exports`. `BTreeMap`
    /// ⇒ key-sorted ⇒ byte-stable (feeds idempotent regeneration).
    #[serde(default)]
    pub exports: BTreeMap<String, String>,
}
// === /CFG-002 ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// `"strict-web"`
    #[default]
    StrictWeb,
    /// `"node-compat"`
    NodeCompat,
    /// `"legacy"`
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Workspace {
    /// Glob roots, e.g. `["apps/*", "packages/*"]` (CANON §13).
    #[serde(default)]
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Runtime {
    /// How TS is handled for execution. Only "strip" is meaningful in P0 (ADR-5, I-3).
    #[serde(default)]
    pub typescript: TsHandling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TsHandling {
    /// `"strip"`
    #[default]
    Strip,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Install {
    #[serde(default)]
    pub mode: InstallMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallMode {
    /// `"pnp"`
    #[default]
    Pnp,
    /// `"vfs"`
    Vfs,
    /// `"materialized"`
    Materialized,
    /// `"vendor"`
    Vendor,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Lint {
    /// rule-id -> severity (`"error"` | `"warn"` | `"off"`).
    #[serde(default)]
    pub rules: BTreeMap<String, Severity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Off,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Format {
    #[serde(default)]
    pub style: FormatStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatStyle {
    /// `"meow"`
    #[default]
    Meow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Types {
    #[serde(default = "default_true")]
    pub strict: bool,
}

impl Default for Types {
    fn default() -> Self {
        Self { strict: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestConfig {
    #[serde(default = "default_true")]
    pub isolate: bool,
    /// `"deterministic"` | `"system"`
    #[serde(default)]
    pub clock: Clock,
    /// `"fake"` | `"real"`
    #[serde(default)]
    pub network: Network,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            isolate: true,
            clock: Clock::default(),
            network: Network::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Clock {
    #[default]
    Deterministic,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[default]
    Fake,
    Real,
}

/// Per-package capability grants (CANON §15). P0 carries only `default` plus the
/// per-package grant map; no tier/grant *enforcement* logic lives here (ADR-6).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Permissions {
    #[serde(default)]
    pub default: Vec<String>,
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<String>>,
}

fn default_true() -> bool {
    true
}
