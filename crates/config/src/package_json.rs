//! Read the user-owned root `package.json`.
//!
//! CFG-003 inverts CFG-002: `package.json` is no longer a meow-generated projection.
//! It is the authority for direct dependencies, scripts, workspaces, and project
//! metadata; `meow` reads it and mutates only the `dependencies` object on explicit
//! add/remove/install-with-package actions.

use crate::ConfigError;
use meow_pkg::{DepSpec, PackageName};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) const PACKAGE_JSON_FILE: &str = "package.json";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageJson {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<PackageName, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<PackageName, String>,
    #[serde(default)]
    pub peer_dependencies: BTreeMap<PackageName, String>,
    #[serde(default)]
    pub overrides: BTreeMap<PackageName, String>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
    #[serde(default)]
    pub workspaces: Option<PackageJsonWorkspaces>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageJsonWorkspaces {
    Patterns(Vec<String>),
    Config(PackageJsonWorkspaceConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageJsonWorkspaceConfig {
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub nohoist: Vec<String>,
}

impl PackageJson {
    pub fn read(root: &Path) -> Result<PackageJson, ConfigError> {
        let path = package_json_path(root);
        let bytes = std::fs::read(&path).map_err(|source| match source.kind() {
            std::io::ErrorKind::NotFound => ConfigError::PackageJsonNotFound(path.clone()),
            _ => ConfigError::Io {
                action: "read",
                path: path.clone(),
                source,
            },
        })?;
        parse_package_json_bytes(&path, &bytes)
    }

    pub fn direct_dependencies(&self) -> Result<BTreeMap<PackageName, DepSpec>, ConfigError> {
        let mut merged = BTreeMap::new();
        merge_dependency_section(&mut merged, &self.dependencies)?;
        merge_dev_dependency_section(&mut merged, &self.dev_dependencies)?;
        Ok(merged)
    }

    pub fn package_overrides(&self) -> Result<BTreeMap<PackageName, DepSpec>, ConfigError> {
        let mut overrides = BTreeMap::new();
        merge_dependency_section(&mut overrides, &self.overrides)?;
        Ok(overrides)
    }
}

pub(crate) fn package_json_path(root: &Path) -> PathBuf {
    root.join(PACKAGE_JSON_FILE)
}

pub(crate) fn parse_package_json_bytes(
    path: &Path,
    bytes: &[u8],
) -> Result<PackageJson, ConfigError> {
    serde_json::from_slice(bytes).map_err(|source| ConfigError::PackageJsonParse {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn parse_package_json_value(
    path: &Path,
    value: &serde_json::Value,
) -> Result<PackageJson, ConfigError> {
    serde_json::from_value(value.clone()).map_err(|source| ConfigError::PackageJsonParse {
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn render_package_json_value(
    path: &Path,
    value: &serde_json::Value,
) -> Result<String, ConfigError> {
    let mut contents = serde_json::to_string_pretty(value).map_err(|source| {
        ConfigError::PackageJsonSerialize {
            path: path.to_path_buf(),
            source,
        }
    })?;
    contents.push('\n');
    Ok(contents)
}

fn dep_spec_to_string(spec: &DepSpec) -> String {
    match spec {
        DepSpec::Range(req) => req.to_string(),
        DepSpec::Tag(tag) => tag.clone(),
        DepSpec::Alias { package, spec } => {
            let inner = dep_spec_to_string(spec);
            if inner == "latest" {
                format!("npm:{package}")
            } else {
                format!("npm:{package}@{inner}")
            }
        }
    }
}

fn merge_dependency_section(
    merged: &mut BTreeMap<PackageName, DepSpec>,
    deps: &BTreeMap<PackageName, String>,
) -> Result<(), ConfigError> {
    for (name, spec) in deps {
        let parsed = DepSpec::parse(spec);
        merged.insert(name.clone(), parsed);
    }
    Ok(())
}

fn merge_dev_dependency_section(
    merged: &mut BTreeMap<PackageName, DepSpec>,
    dev_deps: &BTreeMap<PackageName, String>,
) -> Result<(), ConfigError> {
    for (name, spec) in dev_deps {
        let parsed = DepSpec::parse(spec);
        if let Some(existing) = merged.get(name) {
            if existing != &parsed {
                return Err(ConfigError::ConflictingDependencySpecifier {
                    name: name.to_string(),
                    dependencies_spec: dep_spec_to_string(existing),
                    dev_dependencies_spec: spec.clone(),
                });
            }
            continue;
        }
        merged.insert(name.clone(), parsed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> TempDir {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("meow-package-json-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn read_stock_package_json_shape() {
        let tmp = TempDir::new();
        std::fs::write(
            tmp.path().join(PACKAGE_JSON_FILE),
            r#"{
  "name": "stock-app",
  "version": "1.0.0",
  "dependencies": {
    "dep": "^1.2.3"
  },
  "devDependencies": {
    "typescript": "^5.8.0"
  },
  "peerDependencies": {
    "react": "^19.0.0"
  },
  "overrides": {
    "vite": "^7"
  },
  "scripts": {
    "dev": "vite"
  },
  "workspaces": {
    "packages": ["apps/*", "packages/*"]
  },
  "private": true
}
"#,
        )
        .expect("write package.json");

        let package_json = PackageJson::read(tmp.path()).expect("read package.json");
        assert_eq!(package_json.name.as_deref(), Some("stock-app"));
        assert_eq!(package_json.version.as_deref(), Some("1.0.0"));
        assert_eq!(
            package_json
                .dependencies
                .get(&PackageName::new("dep"))
                .map(String::as_str),
            Some("^1.2.3")
        );
        assert_eq!(
            package_json
                .dev_dependencies
                .get(&PackageName::new("typescript"))
                .map(String::as_str),
            Some("^5.8.0")
        );
        assert_eq!(
            package_json
                .peer_dependencies
                .get(&PackageName::new("react"))
                .map(String::as_str),
            Some("^19.0.0")
        );
        assert_eq!(
            package_json
                .overrides
                .get(&PackageName::new("vite"))
                .map(String::as_str),
            Some("^7")
        );
        assert_eq!(
            package_json.scripts.get("dev").map(String::as_str),
            Some("vite")
        );
        assert_eq!(
            package_json.workspaces,
            Some(PackageJsonWorkspaces::Config(PackageJsonWorkspaceConfig {
                packages: vec!["apps/*".to_owned(), "packages/*".to_owned()],
                nohoist: Vec::new(),
            }))
        );
    }

    #[test]
    fn direct_dependencies_merge_dependencies_and_dev_dependencies() {
        let package_json = PackageJson {
            dependencies: BTreeMap::from([(PackageName::new("dep"), "^1.2.3".to_owned())]),
            dev_dependencies: BTreeMap::from([(PackageName::new("star"), "*".to_owned())]),
            ..PackageJson::default()
        };

        let direct = package_json
            .direct_dependencies()
            .expect("merge direct deps");
        assert_eq!(
            direct.get(&PackageName::new("dep")).map(dep_spec_to_string),
            Some("^1.2.3".to_owned())
        );
        assert_eq!(
            direct
                .get(&PackageName::new("star"))
                .map(dep_spec_to_string),
            Some("*".to_owned())
        );
    }

    #[test]
    fn direct_dependencies_accept_dist_tags_and_npm_aliases() {
        let package_json = PackageJson {
            dependencies: BTreeMap::from([
                (PackageName::new("dep"), "latest".to_owned()),
                (
                    PackageName::new("alias"),
                    "npm:@types/webpack-sources@0.1.5".to_owned(),
                ),
            ]),
            ..PackageJson::default()
        };

        let direct = package_json
            .direct_dependencies()
            .expect("parse dependencies");
        assert_eq!(
            direct.get(&PackageName::new("dep")).map(dep_spec_to_string),
            Some("latest".to_owned())
        );
        assert_eq!(
            direct
                .get(&PackageName::new("alias"))
                .map(dep_spec_to_string),
            Some("npm:@types/webpack-sources@0.1.5".to_owned())
        );
    }

    #[test]
    fn direct_dependencies_reject_conflicting_duplicate_specs() {
        let package_json = PackageJson {
            dependencies: BTreeMap::from([(PackageName::new("dep"), "^1.0.0".to_owned())]),
            dev_dependencies: BTreeMap::from([(PackageName::new("dep"), "^2.0.0".to_owned())]),
            ..PackageJson::default()
        };

        let err = package_json
            .direct_dependencies()
            .expect_err("conflicting duplicate spec must error");
        match err {
            ConfigError::ConflictingDependencySpecifier {
                name,
                dependencies_spec,
                dev_dependencies_spec,
            } => {
                assert_eq!(name, "dep");
                assert_eq!(dependencies_spec, "^1.0.0");
                assert_eq!(dev_dependencies_spec, "^2.0.0");
            }
            other => panic!("got {other:?}, want ConflictingDependencySpecifier"),
        }
    }

    #[test]
    fn package_overrides_parse_semver_specs() {
        let package_json = PackageJson {
            overrides: BTreeMap::from([(PackageName::new("vite"), "^7".to_owned())]),
            ..PackageJson::default()
        };

        let overrides = package_json.package_overrides().expect("parse overrides");
        assert_eq!(
            overrides
                .get(&PackageName::new("vite"))
                .map(dep_spec_to_string),
            Some("^7".to_owned())
        );
    }

    #[test]
    fn missing_package_json_is_typed_not_found() {
        let tmp = TempDir::new();
        let err = PackageJson::read(tmp.path()).expect_err("missing package.json must error");
        match err {
            ConfigError::PackageJsonNotFound(path) => {
                assert_eq!(path, tmp.path().join(PACKAGE_JSON_FILE));
            }
            other => panic!("got {other:?}, want PackageJsonNotFound"),
        }
    }
}
