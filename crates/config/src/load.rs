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

/// Typed, causal errors for config resolution and parsing (CRAFT Part B). No path
/// here `panic!`s/`unwrap()`s on user input.
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
