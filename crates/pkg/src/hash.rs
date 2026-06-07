//! Content-hash + package-specifier newtypes.
//!
//! Hashes and specifiers are modelled as types, not raw `String` (CRAFT Part B):
//! the type is the cheapest invariant enforcer. [`ContentHash`] serializes as
//! npm-compatible SRI (`sha256-<base64>`) in the lockfile and as lower-hex in
//! cache paths; [`PackageName`]/[`Version`] order by bytes, which is exactly the
//! git-merge-resistant sort the lockfile relies on.

use std::fmt;
use std::path::PathBuf;

use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::error::{ParseHashError, ParseVersionError};

/// Hash algorithm. Extensible by design; only `Sha256` ships at P0.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HashAlgo {
    Sha256,
}

impl HashAlgo {
    /// Lowercase identifier used in SRI prefixes and cache paths (`"sha256"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            HashAlgo::Sha256 => "sha256",
        }
    }

    /// Digest length in bytes (`sha256` → 32).
    pub const fn digest_len(self) -> usize {
        match self {
            HashAlgo::Sha256 => 32,
        }
    }

    /// Parse an algorithm identifier; `None` for an unknown prefix.
    fn parse(s: &str) -> Option<HashAlgo> {
        match s {
            "sha256" => Some(HashAlgo::Sha256),
            _ => None,
        }
    }
}

/// Fixed digest capacity. `sha256` fills 32 of these; a future wider algorithm
/// can grow this without touching call sites (only [`HashAlgo::digest_len`]
/// decides how many bytes are significant).
const DIGEST_CAP: usize = 32;

/// A content hash. Textual form is SRI (`sha256-<base64>`, lockfile/greppable);
/// cache-path form is `<algo>/<lowerhex>`. NOT a raw `String`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ContentHash {
    algo: HashAlgo,
    digest: [u8; DIGEST_CAP],
}

impl ContentHash {
    /// Hash `bytes` with the default algorithm (sha256). Pure, total, no panic.
    pub fn of(bytes: &[u8]) -> ContentHash {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let out = hasher.finalize();
        let mut digest = [0u8; DIGEST_CAP];
        // `out` is exactly 32 bytes (sha256), which fits DIGEST_CAP.
        digest[..out.len()].copy_from_slice(&out);
        ContentHash {
            algo: HashAlgo::Sha256,
            digest,
        }
    }

    /// The hashing algorithm.
    pub fn algo(&self) -> HashAlgo {
        self.algo
    }

    /// The raw digest bytes (length == `algo.digest_len()`).
    pub fn digest(&self) -> &[u8] {
        &self.digest[..self.algo.digest_len()]
    }

    /// Canonical lockfile form: `"<algo>-<standard-base64-with-padding>"`.
    pub fn to_sri(&self) -> String {
        let body = base64::engine::general_purpose::STANDARD.encode(self.digest());
        format!("{}-{}", self.algo.as_str(), body)
    }

    /// Parse an SRI string. Rejects unknown algos, non-base64 bodies, and
    /// wrong-length digests with a typed [`ParseHashError`].
    pub fn from_sri(s: &str) -> Result<ContentHash, ParseHashError> {
        let (algo_str, body) = s
            .split_once('-')
            .ok_or_else(|| ParseHashError::Malformed(s.to_owned()))?;
        let algo = HashAlgo::parse(algo_str)
            .ok_or_else(|| ParseHashError::UnknownAlgo(algo_str.to_owned()))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|_| ParseHashError::BadBase64)?;
        let expected = algo.digest_len();
        if bytes.len() != expected {
            return Err(ParseHashError::BadLength {
                algo: algo.as_str(),
                expected,
                got: bytes.len(),
            });
        }
        let mut digest = [0u8; DIGEST_CAP];
        digest[..expected].copy_from_slice(&bytes);
        Ok(ContentHash { algo, digest })
    }

    /// Cache sub-path relative to the cache root: `<algo>/<lowerhex-digest>`.
    pub fn cache_subpath(&self) -> PathBuf {
        let mut p = PathBuf::from(self.algo.as_str());
        p.push(to_hex(self.digest()));
        p
    }
    // === LOAD-003 ===
    /// Canonical URL-host form for `meow-cache://`: `"<algo>-<lowerhex>"`.
    pub fn to_url_host(&self) -> String {
        format!("{}-{}", self.algo.as_str(), to_hex(self.digest()))
    }

    /// Parse the canonical URL-host form `"<algo>-<lowerhex>"`.
    pub fn from_url_host(s: &str) -> Result<ContentHash, ParseHashError> {
        let (algo_str, hex) = s
            .split_once('-')
            .ok_or_else(|| ParseHashError::Malformed(s.to_owned()))?;
        let algo = HashAlgo::parse(algo_str)
            .ok_or_else(|| ParseHashError::UnknownAlgo(algo_str.to_owned()))?;
        let expected = algo.digest_len();
        if hex.len() != expected * 2 {
            return Err(ParseHashError::BadLength {
                algo: algo.as_str(),
                expected,
                got: hex.len() / 2,
            });
        }
        let mut digest = [0u8; DIGEST_CAP];
        for (idx, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
            let hi = decode_hex_nibble(chunk[0])
                .ok_or_else(|| ParseHashError::Malformed(s.to_owned()))?;
            let lo = decode_hex_nibble(chunk[1])
                .ok_or_else(|| ParseHashError::Malformed(s.to_owned()))?;
            digest[idx] = (hi << 4) | lo;
        }
        Ok(ContentHash { algo, digest })
    }
    // === /LOAD-003 ===
}

/// `Debug` renders the SRI so diagnostics and `assert_eq!` failures are legible.
impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.to_sri())
    }
}

/// Serializes AS the SRI string; deserializes via [`ContentHash::from_sri`].
impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_sri())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ContentHash::from_sri(&s).map_err(serde::de::Error::custom)
    }
}

/// Lower-hex encode without pulling in the `hex` crate (footprint, I-10).
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// npm package name (`"lodash"`, `"@scope/pkg"`). `Ord` == byte order == the
/// alphabetical sort used for git-merge-resistant lockfile lines.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct PackageName(String);

impl PackageName {
    /// Wrap a package name.
    pub fn new(name: impl Into<String>) -> PackageName {
        PackageName(name.into())
    }

    /// The underlying name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Exact resolved version. A newtype, not raw `String`, so it cannot be confused
/// with a [`VersionReq`] constraint or an arbitrary string at call sites.
///
/// Untrusted text (a lockfile line) MUST enter through [`Version::parse`] or
/// `Deserialize`, which validate against semver and reject garbage — the lockfile
/// is the execution contract (I-7), so a malformed version never crosses the
/// boundary. The exact original text is stored (byte-stable canonical form);
/// ordering is lexicographic over it (deterministic — all the canonical sort needs).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize)]
pub struct Version(String);

impl Version {
    /// Test-only convenience for known-valid literals. Production code constructs
    /// a `Version` only via [`Version::parse`] (validated), so no unchecked
    /// version can be emitted into a lockfile.
    #[cfg(test)]
    pub(crate) fn new(version: impl Into<String>) -> Version {
        Version(version.into())
    }

    /// Parse + validate an exact semver version, preserving the original text.
    pub fn parse(s: &str) -> Result<Version, ParseVersionError> {
        semver::Version::parse(s).map_err(|e| ParseVersionError::Version {
            value: s.to_owned(),
            reason: e.to_string(),
        })?;
        Ok(Version(s.to_owned()))
    }

    /// The underlying version text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Version, D::Error> {
        let s = String::deserialize(deserializer)?;
        Version::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A semver *constraint* (the per-entry meow runtime-version requirement, §12.3).
/// A distinct newtype from [`Version`] so a constraint can never be passed where
/// an exact version is expected. Untrusted text is validated on
/// [`VersionReq::parse`] / `Deserialize` (semver range); original text preserved.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize)]
pub struct VersionReq(String);

impl VersionReq {
    /// Test-only convenience for known-valid literals; production uses
    /// [`VersionReq::parse`] (validated).
    #[cfg(test)]
    pub(crate) fn new(req: impl Into<String>) -> VersionReq {
        VersionReq(req.into())
    }

    /// Parse + validate a semver requirement range, preserving the original text.
    pub fn parse(s: &str) -> Result<VersionReq, ParseVersionError> {
        semver::VersionReq::parse(s).map_err(|e| ParseVersionError::Req {
            value: s.to_owned(),
            reason: e.to_string(),
        })?;
        Ok(VersionReq(s.to_owned()))
    }

    /// The underlying constraint text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for VersionReq {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<VersionReq, D::Error> {
        let s = String::deserialize(deserializer)?;
        VersionReq::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn of_is_deterministic_and_total() {
        assert_eq!(ContentHash::of(b"abc"), ContentHash::of(b"abc"));
        assert_ne!(ContentHash::of(b"abc"), ContentHash::of(b"abd"));
        // empty input must not panic and yields a stable digest length.
        assert_eq!(ContentHash::of(b"").digest().len(), 32);
    }

    #[test]
    fn sri_round_trips() {
        for sample in [&b""[..], b"x", b"hello world", &[0u8; 64][..]] {
            let h = ContentHash::of(sample);
            let sri = h.to_sri();
            assert!(sri.starts_with("sha256-"));
            assert_eq!(ContentHash::from_sri(&sri).expect("round-trip"), h);
        }
    }

    #[test]
    fn url_host_round_trips() {
        for sample in [&b""[..], b"x", b"hello world", &[0u8; 64][..]] {
            let hash = ContentHash::of(sample);
            let host = hash.to_url_host();
            assert!(host.starts_with("sha256-"));
            assert_eq!(ContentHash::from_url_host(&host).expect("round-trip"), hash);
        }
    }

    #[test]
    fn from_url_host_rejects_non_lower_hex() {
        let hash = ContentHash::of(b"url host");
        let upper = hash.to_url_host().to_ascii_uppercase();
        let err = ContentHash::from_url_host(&upper).unwrap_err();
        assert!(matches!(
            err,
            ParseHashError::UnknownAlgo(_) | ParseHashError::Malformed(_)
        ));
    }

    #[test]
    fn from_sri_rejects_unknown_algo() {
        let err = ContentHash::from_sri("md5-AAAA").unwrap_err();
        assert!(matches!(err, ParseHashError::UnknownAlgo(a) if a == "md5"));
    }

    #[test]
    fn from_sri_rejects_missing_dash() {
        let err = ContentHash::from_sri("sha256").unwrap_err();
        assert!(matches!(err, ParseHashError::Malformed(_)));
    }

    #[test]
    fn from_sri_rejects_bad_base64() {
        let err = ContentHash::from_sri("sha256-not*base64*").unwrap_err();
        assert!(matches!(err, ParseHashError::BadBase64));
    }

    #[test]
    fn from_sri_rejects_wrong_length() {
        // Valid base64 but only 3 bytes — not a 32-byte sha256 digest.
        let short = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        let err = ContentHash::from_sri(&format!("sha256-{short}")).unwrap_err();
        assert!(matches!(
            err,
            ParseHashError::BadLength {
                expected: 32,
                got: 3,
                ..
            }
        ));
    }

    #[test]
    fn cache_subpath_is_algo_then_lowerhex() {
        let h = ContentHash::of(b"");
        let sub = h.cache_subpath();
        let mut comps = sub.components();
        let algo = comps.next().expect("algo component");
        assert_eq!(algo.as_os_str(), "sha256");
        let hex = comps.next().expect("hex component");
        let hex = hex.as_os_str().to_str().expect("utf8");
        assert_eq!(hex.len(), 64);
        assert!(hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert!(comps.next().is_none());
    }

    #[test]
    fn version_parse_accepts_semver_rejects_garbage() {
        assert!(Version::parse("3.0.1").is_ok());
        assert!(Version::parse("1.2.3-beta.1").is_ok());
        assert!(matches!(
            Version::parse("not-a-version").unwrap_err(),
            ParseVersionError::Version { .. }
        ));
        // a bare major is not a full exact semver version
        assert!(Version::parse("1").is_err());
    }

    #[test]
    fn version_req_parse_accepts_range_rejects_garbage() {
        assert!(VersionReq::parse("^0.1").is_ok());
        assert!(VersionReq::parse(">=1.2, <2").is_ok());
        assert!(matches!(
            VersionReq::parse("definitely not a range").unwrap_err(),
            ParseVersionError::Req { .. }
        ));
    }
}
