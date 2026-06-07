//! Registry seam + npm metadata model (PKG-002).
//!
//! The installer resolves against a [`RegistrySource`] trait so `meow-pkg` stays
//! network-free and deterministic under test: unit tests inject [`FixtureRegistry`],
//! while the CLI edge injects the real HTTPS client.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha512};

use crate::{PackageName, Version, VersionReq};

/// Where package metadata + tarball bytes come from.
pub trait RegistrySource {
    /// Fetch the registry document for one package name.
    fn fetch_metadata(&self, name: &PackageName) -> Result<PackageMetadata, RegistryError>;

    /// Fetch the exact gzip-tar tarball bytes for one published version.
    fn fetch_tarball(&self, url: &str) -> Result<Vec<u8>, RegistryError>;
}

/// The subset of an npm package document the resolver consults.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackageMetadata {
    /// npm `dist-tags`, e.g. `latest -> 5.0.0`.
    #[serde(rename = "dist-tags", default)]
    pub dist_tags: BTreeMap<String, Version>,
    /// Exact published versions keyed by semver.
    #[serde(default)]
    pub versions: BTreeMap<Version, VersionMetadata>,
}

/// The manifest subset for one published version.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionMetadata {
    /// Runtime dependencies, keyed by package name.
    #[serde(default)]
    pub dependencies: BTreeMap<PackageName, String>,
    /// Tarball location + registry-published integrity.
    pub dist: DistInfo,
}

/// Tarball URL + integrity metadata from the registry.
#[derive(Debug, Clone, Deserialize)]
pub struct DistInfo {
    /// The exact tarball URL to download.
    pub tarball: String,
    /// Registry-published SRI, normally `sha512-...`.
    #[serde(default)]
    pub integrity: String,
    /// Legacy npm sha1 hex. Recorded by npm; ignored by meow.
    #[serde(default)]
    pub shasum: Option<String>,
}

/// A dependency requirement: either a semver range or a registry dist-tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepSpec {
    Range(VersionReq),
    Tag(String),
}

impl DepSpec {
    /// Parse a direct or transitive dependency requirement.
    pub fn parse(value: &str) -> DepSpec {
        match VersionReq::parse(value) {
            Ok(req) => DepSpec::Range(req),
            Err(_) => DepSpec::Tag(value.to_owned()),
        }
    }
}

/// Typed registry failures. The installer wraps these; the seam never panics.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    #[error("registry request for {target} failed: {reason}")]
    Fetch { target: String, reason: String },
    #[error("registry returned HTTP {status} for {url}")]
    Status { url: String, status: u16 },
    #[error("malformed registry metadata for {name}: {reason}")]
    Metadata { name: String, reason: String },
}

/// In-memory registry fixture for deterministic tests.
#[derive(Debug, Default)]
pub struct FixtureRegistry {
    metadata: BTreeMap<PackageName, PackageMetadata>,
    tarballs: BTreeMap<String, Vec<u8>>,
}

impl FixtureRegistry {
    pub fn new() -> FixtureRegistry {
        FixtureRegistry::default()
    }

    /// Publish one version with the correct sha512 integrity for `bytes`.
    pub fn publish(
        &mut self,
        name: &str,
        version: &str,
        deps: &[(&str, &str)],
        bytes: Vec<u8>,
    ) -> &mut Self {
        let integrity = sha512_sri(&bytes);
        self.publish_with_integrity(name, version, deps, bytes, &integrity)
    }

    /// Publish one version with an explicit integrity string.
    pub fn publish_with_integrity(
        &mut self,
        name: &str,
        version: &str,
        deps: &[(&str, &str)],
        bytes: Vec<u8>,
        integrity: &str,
    ) -> &mut Self {
        let package = PackageName::new(name);
        let version = Version::parse(version).expect("fixture version must be valid semver");
        let dep_map = deps
            .iter()
            .map(|(dep, req)| (PackageName::new(*dep), (*req).to_owned()))
            .collect();
        let tarball = fixture_tarball_url(package.as_str(), version.as_str());

        let versions = &mut self.metadata.entry(package).or_default().versions;
        versions.insert(
            version,
            VersionMetadata {
                dependencies: dep_map,
                dist: DistInfo {
                    tarball: tarball.clone(),
                    integrity: integrity.to_owned(),
                    shasum: None,
                },
            },
        );
        self.tarballs.insert(tarball, bytes);
        self
    }

    /// Publish/update a dist-tag for a package.
    pub fn set_dist_tag(&mut self, name: &str, tag: &str, version: &str) -> &mut Self {
        let package = PackageName::new(name);
        let version = Version::parse(version).expect("fixture version must be valid semver");
        self.metadata
            .entry(package)
            .or_default()
            .dist_tags
            .insert(tag.to_owned(), version);
        self
    }
}

impl RegistrySource for FixtureRegistry {
    fn fetch_metadata(&self, name: &PackageName) -> Result<PackageMetadata, RegistryError> {
        self.metadata
            .get(name)
            .cloned()
            .ok_or_else(|| RegistryError::Fetch {
                target: name.to_string(),
                reason: "package not found in fixture registry".to_owned(),
            })
    }

    fn fetch_tarball(&self, url: &str) -> Result<Vec<u8>, RegistryError> {
        self.tarballs
            .get(url)
            .cloned()
            .ok_or_else(|| RegistryError::Fetch {
                target: url.to_owned(),
                reason: "tarball not found in fixture registry".to_owned(),
            })
    }
}

fn fixture_tarball_url(name: &str, version: &str) -> String {
    format!("fixture://{name}/{version}.tgz")
}

pub(crate) fn sha512_sri(bytes: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let body = base64::engine::general_purpose::STANDARD.encode(digest);
    format!("sha512-{body}")
}
