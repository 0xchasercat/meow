//! Resolve and parse the project config.
//!
//! HONEST PRE-RUNTIME BOUNDARY (I-11): evaluating a real `meow.config.ts` requires
//! the runtime (RT-001) to execute TypeScript, which does not yet exist. CFG-001
//! therefore loads only the static `meow.config.json`; a project that ships *only*
//! a `meow.config.ts` yields [`ConfigError::TsNotSupported`] — a clear, fix-pointing
//! error, NEVER a fabricated default. When RT-001 lands the TS-evaluated path is
//! wired in behind this same return type.

use crate::schema::MeowConfig;
use std::path::{Path, PathBuf};

/// Typed, causal errors for config resolution, package.json dependency authority,
/// and package.json mutation (CRAFT Part B). No path here `panic!`s/`unwrap()`s on
/// user input.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no meow config found under {0} (looked for meow.config.json / meow.config.ts)")]
    NotFound(PathBuf),

    #[error("config at {path} is invalid: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "meow.config.ts evaluation requires the runtime (RT-001), not yet available; \
         provide meow.config.json for now"
    )]
    TsNotSupported,

    #[error("failed to render config at {path}: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("no package.json found at {0}")]
    PackageJsonNotFound(PathBuf),

    #[error("package.json at {path} is invalid: {source}")]
    PackageJsonParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("package.json at {path} must be a JSON object")]
    PackageJsonRootMustBeObject { path: PathBuf },

    #[error("package.json at {path} has non-object {field}; expected an object")]
    PackageJsonFieldMustBeObject { path: PathBuf, field: &'static str },

    #[error("failed to render package.json at {path}: {source}")]
    PackageJsonSerialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("unsupported dependency specifier {name}: {spec} — lands in a later slice")]
    UnsupportedDependencySpecifier {
        name: String,
        spec: String,
        #[source]
        source: meow_pkg::ParseVersionError,
    },

    #[error(
        "dependency {name} is declared in both dependencies and devDependencies with different specifiers ({dependencies_spec} vs {dev_dependencies_spec})"
    )]
    ConflictingDependencySpecifier {
        name: String,
        dependencies_spec: String,
        dev_dependencies_spec: String,
    },

    #[error("failed to {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl MeowConfig {
    /// Resolve and parse the project config under `root`.
    ///
    /// Resolution order:
    /// 1. `root/meow.config.json` exists → parse via `serde_json` into [`MeowConfig`].
    /// 2. Else `root/meow.config.ts` exists → [`ConfigError::TsNotSupported`]
    ///    (HONEST BOUNDARY — see module docs; TS eval needs RT-001).
    /// 3. Else → [`ConfigError::NotFound`].
    ///
    /// Parse failures surface the serde error with the offending file path; user
    /// bytes are never `unwrap`ped.
    pub fn load(root: &Path) -> Result<MeowConfig, ConfigError> {
        let json_path = root.join("meow.config.json");
        if json_path.is_file() {
            let bytes = std::fs::read(&json_path).map_err(|source| ConfigError::Io {
                action: "read",
                path: json_path.clone(),
                source,
            })?;
            return serde_json::from_slice(&bytes).map_err(|source| ConfigError::Parse {
                path: json_path,
                source,
            });
        }

        if root.join("meow.config.ts").is_file() {
            return Err(ConfigError::TsNotSupported);
        }

        Err(ConfigError::NotFound(root.to_path_buf()))
    }
}
