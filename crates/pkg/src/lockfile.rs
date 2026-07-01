//! `meow.lock.jsonl` model + canonical reader/writer (CANON §12.3).
//!
//! The file is strictly-sorted JSON-lines: one [`LockEntry`] per line, compact
//! JSON, ascending by `(name, version)`, trailing newline. Entries are backed by
//! a [`BTreeMap`] keyed on `(name, version)`, so ordering and de-duplication are
//! inherent — there is no code path that can emit an unsorted or duplicated line.
//! The reader is *strict*: a non-canonical file is a defect, surfaced as
//! [`LockError::NotCanonical`], never silently re-sorted (I-7).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::LockError;
use crate::hash::{ContentHash, PackageName, Version, VersionReq};
use crate::tmp_path;

/// One dependency line. Field declaration order IS the canonical JSON key order
/// (serde serializes struct fields in declaration order → deterministic).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LockEntry {
    /// Package name (also the primary sort key).
    pub name: PackageName,
    /// Exact resolved version (secondary sort key).
    pub version: Version,
    /// Content hash of the package tarball/dir (SRI). The I-7 anchor.
    pub integrity: ContentHash,
    /// Resolved transitive deps: dep-name → exact resolved version. A
    /// [`BTreeMap`] so keys serialize sorted → byte-stable. Always serialized
    /// (an empty map renders `{}`), matching the on-disk example.
    pub dependencies: BTreeMap<PackageName, Version>,
    /// Registry provenance (where this resolution came from).
    pub registry: RegistryProvenance,
    /// Capability-grants placeholder. Empty at P0; the capability subsystem
    /// (I-8) defines the real grant type later. Skipped when empty so adding
    /// grants later is a non-breaking, canonical-form-stable change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<CapabilityGrant>,
    /// Wasm-artifact hashes for this package. Empty for pure-JS; skipped when
    /// empty for the same forward-compatibility reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wasm: Vec<ContentHash>,
    /// The meow runtime-version constraint this resolution targets (§12.3,
    /// §26.3 engine-bump reproducibility).
    pub meow: VersionReq,
}

/// Provenance of a resolution. Minimal at P0; Sigstore/OSV land with §15.4.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RegistryProvenance {
    /// Registry URL, e.g. `"https://registry.npmjs.org"`.
    pub registry: String,
}

impl RegistryProvenance {
    /// Construct provenance from a registry URL.
    pub fn new(registry: impl Into<String>) -> RegistryProvenance {
        RegistryProvenance {
            registry: registry.into(),
        }
    }
}

/// Opaque capability-grant placeholder (real shape owned by the I-8 spec). Kept
/// `#[non_exhaustive]` so external crates cannot match/construct it exhaustively
/// before that shape lands.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CapabilityGrant(String);

impl CapabilityGrant {
    /// Construct a placeholder grant from its serialized token.
    pub fn new(token: impl Into<String>) -> CapabilityGrant {
        CapabilityGrant(token.into())
    }

    /// The underlying token text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Canonical line-order key: `(name, version)`. Private — the lockfile owns its
/// ordering invariant and never leaks the key type.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct EntryKey {
    name: PackageName,
    version: Version,
}

impl EntryKey {
    fn of(entry: &LockEntry) -> EntryKey {
        EntryKey {
            name: entry.name.clone(),
            version: entry.version.clone(),
        }
    }
}

/// The whole lockfile. The [`BTreeMap`] keeps entries inherently sorted + de-dup'd.
#[derive(Clone, Default, Debug)]
pub struct Lockfile {
    entries: BTreeMap<EntryKey, LockEntry>,
}

impl Serialize for Lockfile {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.entries.len()))?;
        for entry in self.entries.values() {
            seq.serialize_element(entry)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Lockfile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Lockfile, D::Error> {
        let entries: Vec<LockEntry> = Vec::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for entry in entries {
            map.insert(EntryKey::of(&entry), entry);
        }
        Ok(Lockfile { entries: map })
    }
}

impl Lockfile {
    /// An empty lockfile.
    pub fn new() -> Lockfile {
        Lockfile::default()
    }

    /// Insert or replace by `(name, version)`. Returns the prior entry, if any.
    pub fn upsert(&mut self, entry: LockEntry) -> Option<LockEntry> {
        self.entries.insert(EntryKey::of(&entry), entry)
    }

    /// Look up an entry by `(name, version)`.
    pub fn get(&self, name: &PackageName, version: &Version) -> Option<&LockEntry> {
        self.entries.get(&EntryKey {
            name: name.clone(),
            version: version.clone(),
        })
    }

    /// Entries in ascending `(name, version)` order.
    pub fn iter(&self) -> impl Iterator<Item = &LockEntry> {
        self.entries.values()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the lockfile has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read + parse a lockfile from disk. Strict (see [`Lockfile::parse`]).
    pub fn read(path: &Path) -> Result<Lockfile, LockError> {
        let text = fs::read_to_string(path).map_err(|source| LockError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Lockfile::parse(&text)
    }

    /// Parse from an in-memory string with the same strictness as [`read`].
    ///
    /// Errors ([`LockError::NotCanonical`]) if any line is not strictly ascending
    /// by `(name, version)` (catches both unsorted lines and duplicates) or is
    /// not byte-identical to its canonical re-serialization (catches reordered
    /// keys and stray whitespace). Never panics on any input.
    ///
    /// [`read`]: Lockfile::read
    pub fn parse(text: &str) -> Result<Lockfile, LockError> {
        let mut entries: BTreeMap<EntryKey, LockEntry> = BTreeMap::new();
        let mut prev: Option<EntryKey> = None;

        for (idx, raw) in text.lines().enumerate() {
            let line = idx + 1;
            if raw.is_empty() {
                return Err(LockError::NotCanonical {
                    line,
                    reason: "blank line".to_owned(),
                });
            }

            let entry: LockEntry =
                serde_json::from_str(raw).map_err(|source| diagnose_line(raw, line, source))?;
            let key = EntryKey::of(&entry);

            if let Some(previous) = &prev {
                if key <= *previous {
                    return Err(LockError::NotCanonical {
                        line,
                        reason: "line is not strictly ascending by (name, version) \
                                 (unsorted or duplicate)"
                            .to_owned(),
                    });
                }
            }

            // Re-serialize and demand byte equality: a hand-edited reorder of
            // keys, pretty-printing, or a non-canonical empty array is rejected,
            // not absorbed.
            let canonical =
                serde_json::to_string(&entry).map_err(|source| LockError::Json { line, source })?;
            if canonical != raw {
                return Err(LockError::NotCanonical {
                    line,
                    reason: "line is not canonical compact JSON (reordered keys or whitespace)"
                        .to_owned(),
                });
            }

            prev = Some(key.clone());
            entries.insert(key, entry);
        }

        Ok(Lockfile { entries })
    }

    /// The exact bytes [`write_canonical`] would emit: one compact-JSON entry
    /// per line in ascending key order, each `\n`-terminated. A pure function of
    /// the entry set — same set ⇒ identical bytes regardless of insertion order.
    /// An empty lockfile is the empty string.
    ///
    /// [`write_canonical`]: Lockfile::write_canonical
    pub fn to_canonical_string(&self) -> String {
        let mut out = String::new();
        for entry in self.entries.values() {
            // Serializing an owned, validated LockEntry (strings, BTreeMaps keyed
            // by PackageName which serializes to a string, no floats/NaN) is
            // total: a failure here is a logic bug in this crate, not a reachable
            // input error, so this is an invariant assertion, not a hope on user
            // input.
            let line = serde_json::to_string(entry)
                .expect("LockEntry serialization is infallible for validated entries");
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Deterministically write the canonical form, atomically (write to a sibling
    /// `*.tmp` then `rename`) so a crash never leaves a half-written lockfile.
    pub fn write_canonical(&self, path: &Path) -> Result<(), LockError> {
        let data = self.to_canonical_string();
        let tmp = tmp_path(path);
        fs::write(&tmp, data.as_bytes()).map_err(|source| LockError::Io {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, path).map_err(|source| LockError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

/// On a line-deserialize failure, pin the blame to a specific field with a typed
/// error (a bad integrity SRI or version reads as exactly that, not "invalid
/// JSON" — diagnostics point at the fix, CRAFT). Runs ONLY on the error path; the
/// happy path is untouched. Falls back to [`LockError::Json`] when the JSON shape
/// itself is the problem.
fn diagnose_line(raw: &str, line: usize, json_err: serde_json::Error) -> LockError {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return LockError::Json {
            line,
            source: json_err,
        };
    };
    if let Some(s) = value.get("integrity").and_then(|v| v.as_str()) {
        if let Err(source) = ContentHash::from_sri(s) {
            return LockError::HashField { line, source };
        }
    }
    if let Some(s) = value.get("version").and_then(|v| v.as_str()) {
        if let Err(source) = Version::parse(s) {
            return LockError::VersionField { line, source };
        }
    }
    if let Some(deps) = value.get("dependencies").and_then(|v| v.as_object()) {
        for v in deps.values() {
            if let Some(s) = v.as_str() {
                if let Err(source) = Version::parse(s) {
                    return LockError::VersionField { line, source };
                }
            }
        }
    }
    if let Some(s) = value.get("meow").and_then(|v| v.as_str()) {
        if let Err(source) = VersionReq::parse(s) {
            return LockError::VersionField { line, source };
        }
    }
    LockError::Json {
        line,
        source: json_err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unique_tmp_dir;

    fn entry(name: &str, version: &str) -> LockEntry {
        LockEntry {
            name: PackageName::new(name),
            version: Version::new(version),
            integrity: ContentHash::of(format!("{name}@{version}").as_bytes()),
            dependencies: BTreeMap::new(),
            registry: RegistryProvenance::new("https://registry.npmjs.org"),
            capabilities: Vec::new(),
            wasm: Vec::new(),
            meow: VersionReq::new("^0.1"),
        }
    }

    #[test]
    fn canonical_string_is_byte_stable() {
        let mut lf = Lockfile::new();
        lf.upsert(entry("b", "1.0.0"));
        lf.upsert(entry("a", "2.0.0"));
        assert_eq!(lf.to_canonical_string(), lf.to_canonical_string());
    }

    #[test]
    fn canonical_order_is_independent_of_insertion_order() {
        let mut a = Lockfile::new();
        a.upsert(entry("a", "1.0.0"));
        a.upsert(entry("b", "1.0.0"));
        a.upsert(entry("a", "2.0.0"));

        let mut b = Lockfile::new();
        b.upsert(entry("b", "1.0.0"));
        b.upsert(entry("a", "2.0.0"));
        b.upsert(entry("a", "1.0.0"));

        assert_eq!(a.to_canonical_string(), b.to_canonical_string());

        // Lines strictly ascending; the empty file ends in a trailing newline.
        let s = a.to_canonical_string();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3);
        for pair in lines.windows(2) {
            assert!(pair[0] < pair[1], "lines must be strictly ascending");
        }
        assert!(s.ends_with('\n'));
    }

    #[test]
    fn upsert_dedups_by_key() {
        let mut lf = Lockfile::new();
        assert!(lf.upsert(entry("a", "1.0.0")).is_none());
        // Second upsert of the same (name, version) replaces, not appends.
        assert!(lf.upsert(entry("a", "1.0.0")).is_some());
        assert_eq!(lf.len(), 1);
        assert_eq!(lf.to_canonical_string().lines().count(), 1);
    }

    #[test]
    fn empty_lockfile_is_empty_string() {
        assert_eq!(Lockfile::new().to_canonical_string(), "");
        assert!(Lockfile::new().is_empty());
    }

    #[test]
    fn write_read_round_trips() {
        let dir = unique_tmp_dir("lockfile");
        let path = dir.join("meow.lock.jsonl");

        let mut lf = Lockfile::new();
        let mut deps = BTreeMap::new();
        deps.insert(PackageName::new("is-number"), Version::new("6.0.0"));
        let mut e = entry("is-odd", "3.0.1");
        e.dependencies = deps;
        lf.upsert(e);
        lf.upsert(entry("is-number", "6.0.0"));

        lf.write_canonical(&path).expect("write");
        let back = Lockfile::read(&path).expect("read");
        assert_eq!(back.to_canonical_string(), lf.to_canonical_string());
        assert_eq!(back.len(), lf.len());

        // Write fixpoint: write(read(write(x))) is byte-identical to write(x).
        let first = fs::read(&path).expect("first bytes");
        back.write_canonical(&path).expect("rewrite");
        let second = fs::read(&path).expect("second bytes");
        assert_eq!(first, second);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_lookup_recovers_entry() {
        let mut lf = Lockfile::new();
        lf.upsert(entry("a", "1.2.3"));
        let got = lf.get(&PackageName::new("a"), &Version::new("1.2.3"));
        assert!(got.is_some());
        assert!(lf
            .get(&PackageName::new("a"), &Version::new("9.9.9"))
            .is_none());
    }

    #[test]
    fn parse_rejects_unsorted_lines() {
        let mut lf = Lockfile::new();
        lf.upsert(entry("a", "1.0.0"));
        lf.upsert(entry("b", "1.0.0"));
        let canonical = lf.to_canonical_string();
        let mut lines: Vec<&str> = canonical.lines().collect();
        lines.reverse();
        let unsorted = format!("{}\n", lines.join("\n"));

        let err = Lockfile::parse(&unsorted).unwrap_err();
        assert!(matches!(err, LockError::NotCanonical { line: 2, .. }));
    }

    #[test]
    fn parse_rejects_duplicate_lines() {
        let line = serde_json::to_string(&entry("a", "1.0.0")).unwrap();
        let dup = format!("{line}\n{line}\n");
        let err = Lockfile::parse(&dup).unwrap_err();
        assert!(matches!(err, LockError::NotCanonical { line: 2, .. }));
    }

    #[test]
    fn parse_rejects_reordered_keys() {
        // Same data, but `version` precedes `name` — valid JSON, non-canonical.
        let reordered = r#"{"version":"1.0.0","name":"a","integrity":"sha256-uU0nuZNNPgilLlLX2n2r+sSE7+N6U4DukIj3rOLvzek=","dependencies":{},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.1"}"#;
        let err = Lockfile::parse(&format!("{reordered}\n")).unwrap_err();
        assert!(matches!(err, LockError::NotCanonical { line: 1, .. }));
    }

    #[test]
    fn parse_rejects_invalid_json() {
        let err = Lockfile::parse("{not json}\n").unwrap_err();
        assert!(matches!(err, LockError::Json { line: 1, .. }));
    }

    #[test]
    fn parse_never_panics_on_garbage() {
        // A spread of hostile inputs must each yield Err, never panic.
        for garbage in [
            "\u{0}\u{1}\u{2}",
            "\n\n\n",
            "[]",
            "{\"name\":42}",
            "sha256-",
            "ÿÿÿ",
        ] {
            assert!(Lockfile::parse(garbage).is_err());
        }
    }

    #[test]
    fn parse_bad_integrity_is_hash_field_error() {
        let mut lf = Lockfile::new();
        lf.upsert(entry("a", "1.0.0"));
        // Corrupt only the integrity SRI to a non-SRI token.
        let sri = entry("a", "1.0.0").integrity.to_sri();
        let bad = lf.to_canonical_string().replace(&sri, "not-an-sri");
        let err = Lockfile::parse(&bad).unwrap_err();
        assert!(
            matches!(err, LockError::HashField { line: 1, .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_bad_version_is_version_field_error() {
        let mut lf = Lockfile::new();
        lf.upsert(entry("a", "1.0.0"));
        let bad = lf
            .to_canonical_string()
            .replace("\"1.0.0\"", "\"not-a-version\"");
        let err = Lockfile::parse(&bad).unwrap_err();
        assert!(
            matches!(err, LockError::VersionField { line: 1, .. }),
            "got {err:?}"
        );
    }
}
