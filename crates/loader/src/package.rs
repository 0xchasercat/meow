use std::collections::BTreeMap;

use serde::Deserialize;

/// The subset of `package.json` that resolution consults.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PackageJson {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(rename = "type", default)]
    pub package_type: Option<String>,
    #[serde(default)]
    pub main: Option<String>,
    // === RUN-001 ===
    #[serde(default)]
    pub bin: Option<BinField>,
    // === /RUN-001 ===
    #[serde(default)]
    pub exports: Option<Exports>,
    #[serde(default)]
    pub imports: Option<BTreeMap<String, ExportsTarget>>,
    #[serde(rename = "peerDependencies", default)]
    pub peer_dependencies: BTreeMap<String, String>,
}

// === RUN-001 ===
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BinField {
    Path(String),
    Map(BTreeMap<String, String>),
}

impl PackageJson {
    pub fn bin_entry(&self, command: &str) -> Option<&str> {
        match self.bin.as_ref()? {
            BinField::Path(path) => {
                let expected = self
                    .name
                    .as_deref()
                    .map(default_bin_name)
                    .filter(|name| !name.is_empty())?;
                (expected == command).then_some(path.as_str())
            }
            BinField::Map(entries) => entries.get(command).map(String::as_str),
        }
    }
}

fn default_bin_name(package_name: &str) -> &str {
    package_name.rsplit('/').next().unwrap_or(package_name)
}
// === /RUN-001 ===

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Exports {
    Single(ExportsTarget),
    Map(BTreeMap<String, ExportsTarget>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ExportsTarget {
    Path(String),
    Conditions(BTreeMap<String, ExportsTarget>),
    Fallback(Vec<ExportsTarget>),
    Blocked,
}

#[cfg(test)]
mod tests {
    use super::*;

    // === RUN-001 ===
    #[test]
    fn bin_entry_matches_unscoped_name_for_string_bin() {
        let manifest: PackageJson =
            serde_json::from_slice(br#"{"name":"@scope/toolkit","bin":"bin/tool.cjs"}"#)
                .expect("manifest");
        assert_eq!(manifest.bin_entry("toolkit"), Some("bin/tool.cjs"));
        assert_eq!(manifest.bin_entry("@scope/toolkit"), None);
    }

    #[test]
    fn bin_entry_matches_explicit_map_key() {
        let manifest: PackageJson = serde_json::from_slice(
            br#"{"name":"typescript","bin":{"tsc":"bin/tsc","tsserver":"bin/tsserver"}}"#,
        )
        .expect("manifest");
        assert_eq!(manifest.bin_entry("tsc"), Some("bin/tsc"));
        assert_eq!(manifest.bin_entry("tsserver"), Some("bin/tsserver"));
        assert_eq!(manifest.bin_entry("typescript"), None);
    }
    // === /RUN-001 ===
    #[test]
    fn parses_peer_dependencies() {
        let manifest: PackageJson = serde_json::from_slice(
            br#"{"peerDependencies":{"react":"^19.0.0","react-dom":"^19.0.0"}}"#,
        )
        .expect("manifest");
        assert_eq!(manifest.peer_dependencies.len(), 2);
        assert!(manifest.peer_dependencies.contains_key("react"));
        assert_eq!(
            manifest.peer_dependencies.get("react"),
            Some(&"^19.0.0".to_string())
        );
    }
}
