//! Dependency declaration persistence (`meow.config.json`) for PKG-002.

use std::path::Path;

use meow_pkg::{PackageName, VersionReq};

use crate::{ConfigError, MeowConfig};

const CONFIG_JSON_FILE: &str = "meow.config.json";

/// Write the JSON config file in canonical pretty-printed form.
pub fn write_json_config(cfg: &MeowConfig, root: &Path) -> Result<(), ConfigError> {
    let path = root.join(CONFIG_JSON_FILE);
    let mut contents =
        serde_json::to_string_pretty(cfg).map_err(|source| ConfigError::Serialize {
            path: path.clone(),
            source,
        })?;
    contents.push('\n');
    std::fs::write(&path, contents).map_err(|source| ConfigError::Io {
        action: "write",
        path,
        source,
    })
}

/// Add or update one dependency in `meow.config.json`.
pub fn add_dependency(
    root: &Path,
    name: PackageName,
    req: VersionReq,
) -> Result<MeowConfig, ConfigError> {
    let mut cfg = match MeowConfig::load(root) {
        Ok(cfg) => cfg,
        Err(ConfigError::NotFound(_)) => MeowConfig::default(),
        Err(err) => return Err(err),
    };
    cfg.dependencies.insert(name, req);
    write_json_config(&cfg, root)?;
    Ok(cfg)
}

/// Remove one dependency from `meow.config.json`.
pub fn remove_dependency(root: &Path, name: &PackageName) -> Result<MeowConfig, ConfigError> {
    let mut cfg = match MeowConfig::load(root) {
        Ok(cfg) => cfg,
        Err(ConfigError::NotFound(_)) => MeowConfig::default(),
        Err(err) => return Err(err),
    };
    cfg.dependencies.remove(name);
    write_json_config(&cfg, root)?;
    Ok(cfg)
}
