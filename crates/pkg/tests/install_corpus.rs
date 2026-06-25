use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use meow_pkg::{
    resolve_roots, Cache, CacheError, ContentHash, DepSpec, FixtureRegistry, InstallError,
    Installer, LockEntry, Lockfile, PackageName, RegistryError, RegistryProvenance, RegistrySource,
    RootResolveError, Version, VersionReq,
};

fn tmp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("meow-pkg-install-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn req(value: &str) -> VersionReq {
    VersionReq::parse(value).expect("valid req")
}

fn ver(value: &str) -> Version {
    Version::parse(value).expect("valid version")
}

fn dep(name: &str, spec: DepSpec) -> BTreeMap<PackageName, DepSpec> {
    BTreeMap::from([(PackageName::new(name), spec)])
}

fn declared(name: &str, spec: &str) -> BTreeMap<PackageName, VersionReq> {
    BTreeMap::from([(PackageName::new(name), req(spec))])
}

fn installer<'a, R>(registry: &R, cache: &'a Cache) -> Installer<'a>
where
    R: RegistrySource + Clone + 'static,
{
    Installer::new(
        registry.clone(),
        cache,
        "https://registry.npmjs.org",
        req("^0.0.0"),
    )
}

fn entry(name: &str, version: &str) -> LockEntry {
    LockEntry {
        name: PackageName::new(name),
        version: ver(version),
        integrity: ContentHash::of(format!("{name}@{version}").as_bytes()),
        dependencies: BTreeMap::new(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: req("^0.0.0"),
    }
}

#[test]
fn happy_single_dep_pins_and_caches_the_tarball() {
    let root = tmp_dir("single");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    let bytes = b"pkg-a-1.0.0".to_vec();
    registry.publish("a", "1.0.0", &[], bytes.clone());

    let lockfile = installer(&registry, &cache)
        .resolve(&dep("a", DepSpec::Range(req("^1.0.0"))))
        .expect("install succeeds");

    assert_eq!(lockfile.len(), 1);
    let a = lockfile
        .get(&PackageName::new("a"), &ver("1.0.0"))
        .expect("entry present");
    assert!(
        a.dependencies.is_empty(),
        "zero-dep package records an empty map"
    );
    assert_eq!(a.registry.registry, "https://registry.npmjs.org");
    assert!(cache.contains(&a.integrity), "cache populated");
    assert_eq!(cache.read(&a.integrity).expect("cache read"), bytes);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn transitive_resolution_records_exact_versions() {
    let root = tmp_dir("transitive");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish("a", "1.0.0", &[("b", "^1.0.0")], b"a-1.0.0".to_vec());
    registry.publish("b", "1.2.0", &[], b"b-1.2.0".to_vec());

    let lockfile = installer(&registry, &cache)
        .resolve(&dep("a", DepSpec::Range(req("^1.0.0"))))
        .expect("install succeeds");

    assert_eq!(lockfile.len(), 2);
    let a = lockfile
        .get(&PackageName::new("a"), &ver("1.0.0"))
        .expect("a present");
    assert_eq!(
        a.dependencies.get(&PackageName::new("b")),
        Some(&ver("1.2.0"))
    );
    let b = lockfile
        .get(&PackageName::new("b"), &ver("1.2.0"))
        .expect("b present");
    assert!(cache.contains(&a.integrity));
    assert!(cache.contains(&b.integrity));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn root_overrides_constrain_transitive_dependency_selection() {
    let root = tmp_dir("override-transitive");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish("app", "1.0.0", &[("vite", "*")], b"app-1.0.0".to_vec());
    registry.publish("vite", "7.3.6", &[], b"vite-7.3.6".to_vec());
    registry.publish("vite", "8.1.0", &[], b"vite-8.1.0".to_vec());

    let overrides = BTreeMap::from([(PackageName::new("vite"), DepSpec::Range(req("^7")))]);
    let lockfile = installer(&registry, &cache)
        .with_overrides(overrides)
        .resolve(&dep("app", DepSpec::Range(req("^1.0.0"))))
        .expect("install succeeds");

    let app = lockfile
        .get(&PackageName::new("app"), &ver("1.0.0"))
        .expect("app present");
    assert_eq!(
        app.dependencies.get(&PackageName::new("vite")),
        Some(&ver("7.3.6"))
    );
    assert!(
        lockfile
            .get(&PackageName::new("vite"), &ver("8.1.0"))
            .is_none(),
        "override must prevent the unconstrained latest version from being locked"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn root_overrides_reject_reusing_locked_version_outside_override_range() {
    let root = tmp_dir("override-reuse");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish("app", "1.0.0", &[("vite", "*")], b"app-1.0.0".to_vec());
    registry.publish("vite", "7.3.6", &[], b"vite-7.3.6".to_vec());
    registry.publish("vite", "8.1.0", &[], b"vite-8.1.0".to_vec());

    let mut old_lock = Lockfile::new();
    old_lock.upsert(entry("vite", "8.1.0"));
    let overrides = BTreeMap::from([(PackageName::new("vite"), DepSpec::Range(req("^7")))]);

    let lockfile = installer(&registry, &cache)
        .with_reuse_lockfile(old_lock)
        .with_overrides(overrides)
        .resolve(&dep("app", DepSpec::Range(req("^1.0.0"))))
        .expect("install succeeds");

    assert!(lockfile
        .get(&PackageName::new("vite"), &ver("8.1.0"))
        .is_none());
    assert!(lockfile
        .get(&PackageName::new("vite"), &ver("7.3.6"))
        .is_some());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn npm_alias_fetches_target_metadata_but_pins_alias_name() {
    let root = tmp_dir("alias");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish(
        "@swc/helpers",
        "0.4.14",
        &[("tslib", "^2.4.0")],
        b"helpers".to_vec(),
    );
    registry.publish("tslib", "2.4.1", &[], b"tslib".to_vec());
    let direct = BTreeMap::from([(
        PackageName::new("@swc/legacy-helpers"),
        DepSpec::parse("npm:@swc/helpers@=0.4.14"),
    )]);

    let lockfile = installer(&registry, &cache)
        .resolve(&direct)
        .expect("install succeeds");

    let alias = lockfile
        .get(&PackageName::new("@swc/legacy-helpers"), &ver("0.4.14"))
        .expect("alias entry is present");
    assert!(
        lockfile
            .get(&PackageName::new("@swc/helpers"), &ver("0.4.14"))
            .is_none(),
        "alias does not require a second target-named lock entry"
    );
    assert_eq!(
        alias.dependencies.get(&PackageName::new("tslib")),
        Some(&ver("2.4.1"))
    );
    assert!(cache.contains(&alias.integrity));
    std::fs::remove_dir_all(root).ok();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn darwin_arm64_optional_dependencies_are_filtered_by_platform_support() {
    let root = tmp_dir("optional-filter");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();

    registry.publish_with_optional_dependencies_and_platform(
        "next",
        "15.0.0",
        &[],
        (
            &[
                ("@next/swc-darwin-arm64", "15.0.0"),
                ("@next/swc-linux-x64-gnu", "15.0.0"),
            ],
            &[],
            &[],
        ),
        b"next".to_vec(),
    );
    registry.publish_with_optional_dependencies_and_platform(
        "@next/swc-darwin-arm64",
        "15.0.0",
        &[],
        (&[], &["darwin"], &["arm64"]),
        b"swc-darwin-arm64".to_vec(),
    );
    registry.publish_with_optional_dependencies_and_platform(
        "@next/swc-linux-x64-gnu",
        "15.0.0",
        &[],
        (&[], &["linux"], &["x64"]),
        b"swc-linux-x64-gnu".to_vec(),
    );

    let lockfile = installer(&registry, &cache)
        .resolve(&dep("next", DepSpec::Range(req("^15.0.0"))))
        .expect("install succeeds");
    let next = lockfile
        .get(&PackageName::new("next"), &ver("15.0.0"))
        .expect("root package is present");

    assert_eq!(
        next.dependencies
            .get(&PackageName::new("@next/swc-darwin-arm64")),
        Some(&ver("15.0.0")),
        "darwin/arm64 optional dependency is included in the graph"
    );
    assert!(
        lockfile
            .get(&PackageName::new("@next/swc-darwin-arm64"), &ver("15.0.0"))
            .is_some(),
        "compatible optional dependency package is pinned"
    );
    assert!(
        next.dependencies
            .get(&PackageName::new("@next/swc-linux-x64-gnu"))
            .is_none(),
        "linux/x64 optional dependency is not in the graph"
    );
    assert!(
        lockfile
            .get(&PackageName::new("@next/swc-linux-x64-gnu"), &ver("15.0.0"))
            .is_none(),
        "incompatible optional dependency package is not pinned"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn multi_version_graph_keeps_both_pins() {
    let root = tmp_dir("multiver");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish("a", "1.0.0", &[("b", "^1.0.0")], b"a-1".to_vec());
    registry.publish("c", "1.0.0", &[("b", "^2.0.0")], b"c-1".to_vec());
    registry.publish("b", "1.5.0", &[], b"b-1.5".to_vec());
    registry.publish("b", "2.1.0", &[], b"b-2.1".to_vec());

    let direct = BTreeMap::from([
        (PackageName::new("a"), DepSpec::Range(req("^1.0.0"))),
        (PackageName::new("c"), DepSpec::Range(req("^1.0.0"))),
    ]);
    let lockfile = installer(&registry, &cache)
        .resolve(&direct)
        .expect("install succeeds");

    assert!(lockfile
        .get(&PackageName::new("b"), &ver("1.5.0"))
        .is_some());
    assert!(lockfile
        .get(&PackageName::new("b"), &ver("2.1.0"))
        .is_some());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn semver_max_satisfying_and_dist_tags_are_honored() {
    let root = tmp_dir("selection");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish("pkg", "1.0.0", &[], b"pkg-1.0.0".to_vec());
    registry.publish("pkg", "1.1.0", &[], b"pkg-1.1.0".to_vec());
    registry.publish("pkg", "1.2.0", &[], b"pkg-1.2.0".to_vec());
    registry.publish("pkg", "1.3.0-beta.1", &[], b"pkg-1.3.0-beta.1".to_vec());
    registry.set_dist_tag("pkg", "latest", "1.1.0");

    let max = installer(&registry, &cache)
        .resolve(&dep("pkg", DepSpec::Range(req("^1.0.0"))))
        .expect("caret range resolves");
    assert!(max.get(&PackageName::new("pkg"), &ver("1.2.0")).is_some());
    assert!(
        max.get(&PackageName::new("pkg"), &ver("1.3.0-beta.1"))
            .is_none(),
        "pre-releases excluded unless requested"
    );

    let tilde = installer(&registry, &cache)
        .resolve(&dep("pkg", DepSpec::Range(req("~1.1.0"))))
        .expect("tilde range resolves");
    assert!(tilde.get(&PackageName::new("pkg"), &ver("1.1.0")).is_some());

    let disjunction = installer(&registry, &cache)
        .resolve(&dep("pkg", DepSpec::Range(req("^0.9 || ^1.1.0"))))
        .expect("disjunction range resolves");
    assert!(disjunction
        .get(&PackageName::new("pkg"), &ver("1.2.0"))
        .is_some());

    let tagged = installer(&registry, &cache)
        .resolve(&dep("pkg", DepSpec::Tag("latest".to_owned())))
        .expect("dist tag resolves");
    assert!(tagged
        .get(&PackageName::new("pkg"), &ver("1.1.0"))
        .is_some());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn lockfile_bytes_are_deterministic_and_sorted() {
    let root = tmp_dir("determinism");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish("a", "1.0.0", &[("c", "^1.0.0")], b"a".to_vec());
    registry.publish("b", "1.0.0", &[], b"b".to_vec());
    registry.publish("c", "1.0.0", &[], b"c".to_vec());

    let direct_one = BTreeMap::from([
        (PackageName::new("b"), DepSpec::Range(req("^1.0.0"))),
        (PackageName::new("a"), DepSpec::Range(req("^1.0.0"))),
    ]);
    let direct_two = BTreeMap::from([
        (PackageName::new("a"), DepSpec::Range(req("^1.0.0"))),
        (PackageName::new("b"), DepSpec::Range(req("^1.0.0"))),
    ]);

    let left = installer(&registry, &cache)
        .resolve(&direct_one)
        .expect("left resolves")
        .to_canonical_string();
    let right = installer(&registry, &cache)
        .resolve(&direct_two)
        .expect("right resolves")
        .to_canonical_string();
    assert_eq!(left, right, "same graph -> byte-identical lockfile");

    let parsed = Lockfile::parse(&left).expect("canonical lockfile parses strictly");
    assert_eq!(parsed.to_canonical_string(), left);

    let names: Vec<_> = parsed.iter().map(|entry| entry.name.to_string()).collect();
    assert_eq!(
        names,
        vec!["a", "b", "c"],
        "entries sorted by (name, version)"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn warm_reused_lockfile_resolves_without_registry_requests() {
    #[derive(Clone)]
    struct OfflineRegistry;

    impl RegistrySource for OfflineRegistry {
        fn fetch_metadata<'a>(
            &'a self,
            name: &'a PackageName,
        ) -> Pin<
            Box<dyn Future<Output = Result<meow_pkg::PackageMetadata, RegistryError>> + Send + 'a>,
        > {
            Box::pin(async move {
                Err(RegistryError::Fetch {
                    target: name.to_string(),
                    reason: "metadata should not be fetched for warm lockfile installs".to_owned(),
                })
            })
        }

        fn fetch_tarball<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, RegistryError>> + Send + 'a>> {
            Box::pin(async move {
                Err(RegistryError::Fetch {
                    target: url.to_owned(),
                    reason: "tarball should not be fetched for warm lockfile installs".to_owned(),
                })
            })
        }
    }

    let root = tmp_dir("reuse-offline");
    let cache = Cache::with_root(root.join("cache"));
    let a_bytes = b"a-1.0.0".to_vec();
    let b_bytes = b"b-1.0.0".to_vec();
    let a_integrity = cache.store(&a_bytes).expect("cache a");
    let b_integrity = cache.store(&b_bytes).expect("cache b");
    let mut a = entry("a", "1.0.0");
    a.integrity = a_integrity;
    a.dependencies = BTreeMap::from([(PackageName::new("b"), ver("1.0.0"))]);
    let mut b = entry("b", "1.0.0");
    b.integrity = b_integrity;

    let mut reuse = Lockfile::new();
    reuse.upsert(a);
    reuse.upsert(b);

    let resolved = installer(&OfflineRegistry, &cache)
        .with_reuse_lockfile(reuse.clone())
        .resolve(&dep("a", DepSpec::Range(req("^1.0.0"))))
        .expect("warm lockfile install must not touch registry");

    assert_eq!(resolved.to_canonical_string(), reuse.to_canonical_string());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn reused_lockfile_requires_cached_blob_presence_before_claiming_hit() {
    let root = tmp_dir("reuse-missing");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    let bytes = b"a-1.0.0".to_vec();
    registry.publish("a", "1.0.0", &[], bytes.clone());
    let direct = dep("a", DepSpec::Range(req("^1.0.0")));

    let lockfile = installer(&registry, &cache)
        .resolve(&direct)
        .expect("initial install succeeds");
    let entry = lockfile
        .get(&PackageName::new("a"), &ver("1.0.0"))
        .expect("entry present")
        .clone();
    std::fs::remove_file(cache.path_for(&entry.integrity)).expect("remove cache blob");
    assert!(matches!(
        cache.read(&entry.integrity),
        Err(CacheError::NotFound(_))
    ));

    let repaired = installer(&registry, &cache)
        .with_reuse_lockfile(lockfile.clone())
        .resolve(&direct)
        .expect("missing cache hit is repaired by redownload");
    assert_eq!(
        repaired.to_canonical_string(),
        lockfile.to_canonical_string()
    );
    assert_eq!(cache.read(&entry.integrity).expect("cache repaired"), bytes);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn integrity_mismatch_returns_typed_error_and_stores_nothing() {
    let root = tmp_dir("integrity-mismatch");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    let bytes = b"tampered".to_vec();
    registry.publish_with_integrity("a", "1.0.0", &[], bytes.clone(), "sha512-AAAA");

    let err = installer(&registry, &cache)
        .resolve(&dep("a", DepSpec::Range(req("^1.0.0"))))
        .expect_err("integrity mismatch must fail");
    assert!(matches!(
        err,
        InstallError::MalformedIntegrity { .. } | InstallError::IntegrityMismatch { .. }
    ));
    assert!(
        !cache.contains(&ContentHash::of(&bytes)),
        "failed verification must not populate the cache"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn unsupported_integrity_no_match_and_unsupported_spec_are_honest_errors() {
    let root = tmp_dir("honesty");
    let cache = Cache::with_root(root.join("cache"));
    let mut registry = FixtureRegistry::new();
    registry.publish_with_integrity("a", "1.0.0", &[], b"a".to_vec(), "sha1-deadbeef");
    registry.publish("b", "1.0.0", &[], b"b".to_vec());

    let integrity_err = installer(&registry, &cache)
        .resolve(&dep("a", DepSpec::Range(req("^1.0.0"))))
        .expect_err("sha1-only integrity must fail");
    assert!(matches!(
        integrity_err,
        InstallError::UnsupportedIntegrity { .. }
    ));

    let no_match = installer(&registry, &cache)
        .resolve(&dep("b", DepSpec::Range(req("^9.0.0"))))
        .expect_err("missing version must fail");
    assert!(matches!(no_match, InstallError::NoMatchingVersion { .. }));

    let unsupported = installer(&registry, &cache)
        .resolve(&dep("b", DepSpec::parse("file:../local")))
        .expect_err("unsupported non-registry specifier must fail");
    assert!(matches!(unsupported, InstallError::UnsupportedRange { .. }));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_registry_metadata_surfaces_typed_error() {
    #[derive(Clone)]
    struct BrokenRegistry;

    impl RegistrySource for BrokenRegistry {
        fn fetch_metadata<'a>(
            &'a self,
            name: &'a PackageName,
        ) -> Pin<
            Box<dyn Future<Output = Result<meow_pkg::PackageMetadata, RegistryError>> + Send + 'a>,
        > {
            Box::pin(async move {
                Err(RegistryError::Metadata {
                    name: name.to_string(),
                    reason: "bad version key".to_owned(),
                })
            })
        }

        fn fetch_tarball<'a>(
            &'a self,
            url: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, RegistryError>> + Send + 'a>> {
            Box::pin(async move {
                Err(RegistryError::Fetch {
                    target: url.to_owned(),
                    reason: "unreachable".to_owned(),
                })
            })
        }
    }

    let root = tmp_dir("metadata");
    let cache = Cache::with_root(root.join("cache"));
    let err = installer(&BrokenRegistry, &cache)
        .resolve(&dep("a", DepSpec::Range(req("^1.0.0"))))
        .expect_err("metadata error must surface");
    assert!(matches!(
        err,
        InstallError::Registry(RegistryError::Metadata { .. })
    ));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolve_roots_picks_the_max_satisfying_declared_pin() {
    let mut lockfile = Lockfile::new();
    lockfile.upsert(entry("a", "1.0.0"));
    lockfile.upsert(entry("a", "1.2.0"));
    lockfile.upsert(entry("a", "2.0.0"));

    let roots = resolve_roots(&declared("a", "^1.1.0"), &lockfile).expect("root resolves");
    assert_eq!(roots.get(&PackageName::new("a")), Some(&ver("1.2.0")));
}

#[test]
fn resolve_roots_supports_disjunction_ranges() {
    let mut lockfile = Lockfile::new();
    lockfile.upsert(entry("a", "4.1.0"));
    lockfile.upsert(entry("a", "5.2.0"));
    lockfile.upsert(entry("a", "6.3.0"));

    let roots = resolve_roots(&declared("a", "^4.0.2 || ^5.0 || ^6.0"), &lockfile)
        .expect("disjunction root resolves");
    assert_eq!(roots.get(&PackageName::new("a")), Some(&ver("6.3.0")));
}

#[test]
fn resolve_roots_errors_when_declared_dep_is_absent() {
    let mut lockfile = Lockfile::new();
    lockfile.upsert(entry("a", "1.0.0"));

    let err =
        resolve_roots(&declared("b", "^1.0.0"), &lockfile).expect_err("missing dep must fail");
    assert!(matches!(err, RootResolveError::NotInLockfile { .. }));
}
