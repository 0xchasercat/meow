use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path};
use std::sync::Arc;

use flate2::read::GzDecoder;
use serde::Deserialize;

use crate::resolver::ResolveError;

const ARCHIVE_LABEL: &str = "<cached package>";

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

/// Read-only in-memory view of one cached package archive.
#[derive(Debug)]
pub(crate) struct PackageFs {
    manifest: PackageJson,
    nested_manifests: BTreeMap<String, PackageJson>,
    members: BTreeMap<String, Arc<[u8]>>,
}

impl PackageFs {
    pub fn from_archive(bytes: &[u8]) -> Result<PackageFs, ResolveError> {
        let decoder = GzDecoder::new(bytes);
        let mut archive = tar::Archive::new(decoder);
        let mut top_level_manifest = None;
        let mut nested_manifests = BTreeMap::new();
        let mut members = BTreeMap::new();

        let entries = archive
            .entries()
            .map_err(|err| invalid_archive(err.to_string()))?;
        for entry in entries {
            let mut entry = entry.map_err(|err| invalid_archive(err.to_string()))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry
                .path()
                .map_err(|err| invalid_archive(err.to_string()))?;
            let Some(member) = strip_package_prefix(path.as_ref())? else {
                continue;
            };

            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|err| invalid_archive(err.to_string()))?;
            let bytes: Arc<[u8]> = Arc::from(buf);
            if member == "package.json" || member.ends_with("/package.json") {
                let manifest: PackageJson = serde_json::from_slice(bytes.as_ref())
                    .map_err(|err| invalid_manifest(err.to_string()))?;
                if member == "package.json" {
                    top_level_manifest = Some(manifest.clone());
                } else if let Some(dir) = member.strip_suffix("/package.json") {
                    nested_manifests.insert(dir.to_owned(), manifest);
                }
            }
            members.insert(member, bytes);
        }

        let manifest = top_level_manifest.ok_or_else(|| {
            invalid_archive("missing top-level package/package.json member".to_owned())
        })?;
        Ok(PackageFs {
            manifest,
            nested_manifests,
            members,
        })
    }

    pub fn manifest(&self) -> &PackageJson {
        &self.manifest
    }

    pub fn contains(&self, member: &str) -> bool {
        self.members.contains_key(member)
    }

    pub fn read(&self, member: &str) -> Option<Arc<[u8]>> {
        self.members.get(member).cloned()
    }

    pub fn nearest_manifest(&self, member: &str) -> &PackageJson {
        let mut current = member.rsplit_once('/').map(|(dir, _)| dir);
        while let Some(dir) = current {
            if let Some(manifest) = self.nested_manifests.get(dir) {
                return manifest;
            }
            current = dir.rsplit_once('/').map(|(parent, _)| parent);
        }
        &self.manifest
    }
}

fn invalid_archive(reason: String) -> ResolveError {
    ResolveError::InvalidArchive {
        package: ARCHIVE_LABEL.to_owned(),
        reason,
    }
}

fn invalid_manifest(reason: String) -> ResolveError {
    ResolveError::InvalidManifest {
        package: ARCHIVE_LABEL.to_owned(),
        reason,
    }
}

fn strip_package_prefix(path: &Path) -> Result<Option<String>, ResolveError> {
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(first)) if first == "package" => {}
        Some(_) => return Ok(None),
        None => return Ok(None),
    }

    let mut member = String::new();
    for component in components {
        let Component::Normal(segment) = component else {
            return Err(invalid_archive(format!(
                "invalid archive member path {:?}",
                path
            )));
        };
        let segment = segment
            .to_str()
            .ok_or_else(|| invalid_archive(format!("non-utf8 archive member path {:?}", path)))?;
        if !member.is_empty() {
            member.push('/');
        }
        member.push_str(segment);
    }

    if member.is_empty() {
        return Ok(None);
    }
    Ok(Some(member))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("package/{path}"), *bytes)
                .expect("append tar member");
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    #[test]
    fn decodes_archive_members_and_manifest() {
        let bytes = archive(&[
            (
                "package.json",
                br#"{"name":"dep","version":"1.0.0","type":"module"}"#,
            ),
            ("dist/index.js", b"export const value = 1;\n"),
        ]);
        let fs = PackageFs::from_archive(&bytes).expect("archive decodes");
        assert_eq!(fs.manifest().name.as_deref(), Some("dep"));
        assert!(fs.contains("package.json"));
        assert!(fs.contains("dist/index.js"));
        assert_eq!(
            fs.read("dist/index.js").expect("member bytes").as_ref(),
            b"export const value = 1;\n"
        );
    }

    #[test]
    fn nearest_manifest_prefers_nested_package_json() {
        let bytes = archive(&[
            ("package.json", br#"{"name":"dep","type":"commonjs"}"#),
            ("esm/package.json", br#"{"type":"module"}"#),
            ("esm/index.js", b"export const value = 1;\n"),
        ]);
        let fs = PackageFs::from_archive(&bytes).expect("archive decodes");
        assert_eq!(
            fs.nearest_manifest("esm/index.js").package_type.as_deref(),
            Some("module")
        );
        assert_eq!(
            fs.nearest_manifest("root.js").package_type.as_deref(),
            Some("commonjs")
        );
    }

    #[test]
    fn rejects_missing_top_level_package_json() {
        let bytes = archive(&[("dist/index.js", b"export const value = 1;\n")]);
        let err = PackageFs::from_archive(&bytes).unwrap_err();
        assert!(matches!(err, ResolveError::InvalidArchive { .. }));
    }

    #[test]
    fn rejects_malformed_manifest() {
        let bytes = archive(&[
            ("package.json", b"{not json"),
            ("index.js", b"export {};\n"),
        ]);
        let err = PackageFs::from_archive(&bytes).unwrap_err();
        assert!(matches!(err, ResolveError::InvalidManifest { .. }));
    }

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
    fn rejects_non_archive_bytes() {
        let err = PackageFs::from_archive(b"not a gzip tarball").unwrap_err();
        assert!(matches!(err, ResolveError::InvalidArchive { .. }));
    }
}
