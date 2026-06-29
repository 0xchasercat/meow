//! Dependency declaration persistence (`package.json`) for CFG-003.

use std::path::Path;

use meow_pkg::{PackageName, VersionReq};
use serde_json::{Map, Value};

use crate::package_json::{package_json_path, parse_package_json_value, render_package_json_value};
use crate::{ConfigError, PackageJson};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySection {
    Dependencies,
    DevDependencies,
}

impl DependencySection {
    fn field(self) -> &'static str {
        match self {
            Self::Dependencies => "dependencies",
            Self::DevDependencies => "devDependencies",
        }
    }
}

/// Add or update one dependency in the user-owned root `package.json`.
pub fn add_dependency(
    root: &Path,
    name: PackageName,
    req: VersionReq,
) -> Result<PackageJson, ConfigError> {
    add_dependency_to(root, name, req, DependencySection::Dependencies)
}

pub fn add_dependency_to(
    root: &Path,
    name: PackageName,
    req: VersionReq,
    section: DependencySection,
) -> Result<PackageJson, ConfigError> {
    let (path, mut value) = read_package_json_document(root, true)?;
    let root_object = root_object_mut(&path, &mut value)?;
    let deps = match dependencies_object_mut(&path, root_object, section, true)? {
        Some(deps) => deps,
        None => return parse_package_json_value(&path, &value),
    };

    let raw_name = name.as_str();
    let raw_req = req.as_str();
    if matches!(deps.get(raw_name), Some(Value::String(existing)) if existing == raw_req) {
        return parse_package_json_value(&path, &value);
    }

    deps.insert(raw_name.to_owned(), Value::String(raw_req.to_owned()));
    write_package_json_document(&path, &value)?;
    parse_package_json_value(&path, &value)
}

/// Remove one dependency from the user-owned root `package.json`.
pub fn remove_dependency(root: &Path, name: &PackageName) -> Result<PackageJson, ConfigError> {
    let (path, mut value) = read_package_json_document(root, false)?;
    let root_object = root_object_mut(&path, &mut value)?;
    let remove_field = {
        let Some(deps) = dependencies_object_mut(
            &path,
            root_object,
            DependencySection::Dependencies,
            false,
        )? else {
            return parse_package_json_value(&path, &value);
        };

        let removed = deps.remove(name.as_str()).is_some();
        if !removed {
            return parse_package_json_value(&path, &value);
        }
        deps.is_empty()
    };
    if remove_field {
        root_object.remove("dependencies");
    }

    write_package_json_document(&path, &value)?;
    parse_package_json_value(&path, &value)
}

fn read_package_json_document(
    root: &Path,
    create_if_missing: bool,
) -> Result<(std::path::PathBuf, Value), ConfigError> {
    let path = package_json_path(root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && create_if_missing => {
            let value = Value::Object(Map::new());
            parse_package_json_value(&path, &value)?;
            return Ok((path, value));
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfigError::PackageJsonNotFound(path));
        }
        Err(source) => {
            return Err(ConfigError::Io {
                action: "read",
                path,
                source,
            });
        }
    };
    let value = serde_json::from_slice(&bytes).map_err(|source| ConfigError::PackageJsonParse {
        path: path.clone(),
        source,
    })?;
    parse_package_json_value(&path, &value)?;
    Ok((path, value))
}

fn root_object_mut<'a>(
    path: &Path,
    value: &'a mut Value,
) -> Result<&'a mut Map<String, Value>, ConfigError> {
    value
        .as_object_mut()
        .ok_or_else(|| ConfigError::PackageJsonRootMustBeObject {
            path: path.to_path_buf(),
        })
}

fn dependencies_object_mut<'a>(
    path: &Path,
    root: &'a mut Map<String, Value>,
    section: DependencySection,
    create_if_missing: bool,
) -> Result<Option<&'a mut Map<String, Value>>, ConfigError> {
    let field = section.field();
    if !root.contains_key(field) {
        if !create_if_missing {
            return Ok(None);
        }
        root.insert(field.to_owned(), Value::Object(Map::new()));
    }

    match root.get_mut(field) {
        Some(Value::Object(deps)) => Ok(Some(deps)),
        Some(_) => Err(ConfigError::PackageJsonFieldMustBeObject {
            path: path.to_path_buf(),
            field,
        }),
        None => Ok(None),
    }
}

fn write_package_json_document(path: &Path, value: &Value) -> Result<(), ConfigError> {
    let contents = render_package_json_value(path, value)?;
    std::fs::write(path, contents).map_err(|source| ConfigError::Io {
        action: "write",
        path: path.to_path_buf(),
        source,
    })
}
