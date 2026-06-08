use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use meow_pkg::{
    Cache, CacheError, ContentHash, LockEntry, Lockfile, PackageName, PnpError, RegistryProvenance,
    ResolutionGraph, Version, VersionReq,
};

fn tmp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-pkg-pnp-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn ver(value: &str) -> Version {
    Version::parse(value).expect("valid version")
}

fn req(value: &str) -> VersionReq {
    VersionReq::parse(value).expect("valid requirement")
}

fn lock_entry(
    name: &str,
    version: &str,
    integrity: ContentHash,
    deps: &[(&str, &str)],
) -> LockEntry {
    LockEntry {
        name: PackageName::new(name),
        version: ver(version),
        integrity,
        dependencies: deps
            .iter()
            .map(|(dep, dep_version)| (PackageName::new(*dep), ver(dep_version)))
            .collect(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: req("*"),
    }
}

fn roots(entries: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
    entries
        .iter()
        .map(|(name, version)| (PackageName::new(*name), ver(version)))
        .collect()
}

fn package_fingerprint(graph: &ResolutionGraph) -> Vec<(String, String, String)> {
    graph
        .packages()
        .map(|pkg| {
            (
                pkg.name.to_string(),
                pkg.version.to_string(),
                pkg.integrity.to_sri(),
            )
        })
        .collect()
}

#[test]
fn assemble_closes_a_real_tree_and_keeps_both_versions() {
    let a_hash = ContentHash::of(b"a@1.0.0");
    let b1_hash = ContentHash::of(b"b@1.0.0");
    let b2_hash = ContentHash::of(b"b@2.0.0");
    let c_hash = ContentHash::of(b"c@1.0.0");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));

    let graph =
        ResolutionGraph::assemble(Arc::new(lockfile), roots(&[("a", "1.0.0"), ("c", "1.0.0")]))
            .expect("graph assembles");

    assert_eq!(
        package_fingerprint(&graph),
        vec![
            ("a".to_owned(), "1.0.0".to_owned(), a_hash.to_sri()),
            ("b".to_owned(), "1.0.0".to_owned(), b1_hash.to_sri()),
            ("b".to_owned(), "2.0.0".to_owned(), b2_hash.to_sri()),
            ("c".to_owned(), "1.0.0".to_owned(), c_hash.to_sri()),
        ]
    );

    let deps: Vec<_> = graph
        .packages()
        .map(|pkg| {
            (
                pkg.name.to_string(),
                pkg.version.to_string(),
                pkg.dependencies
                    .iter()
                    .map(|(name, version)| (name.to_string(), version.to_string()))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        deps,
        vec![
            (
                "a".to_owned(),
                "1.0.0".to_owned(),
                vec![("b".to_owned(), "1.0.0".to_owned())],
            ),
            ("b".to_owned(), "1.0.0".to_owned(), vec![]),
            ("b".to_owned(), "2.0.0".to_owned(), vec![]),
            (
                "c".to_owned(),
                "1.0.0".to_owned(),
                vec![("b".to_owned(), "2.0.0".to_owned())],
            ),
        ]
    );
}

#[test]
fn assemble_reports_a_missing_root_pin() {
    let err = match ResolutionGraph::assemble(Arc::new(Lockfile::new()), roots(&[("a", "1.0.0")])) {
        Ok(_) => panic!("missing root pin must fail"),
        Err(err) => err,
    };

    assert!(matches!(
        err,
        PnpError::RootNotLocked { name, version }
            if name == PackageName::new("a") && version == ver("1.0.0")
    ));
}

#[test]
fn assemble_reports_a_dangling_edge() {
    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "a",
        "1.0.0",
        ContentHash::of(b"a@1.0.0"),
        &[("b", "2.0.0")],
    ));

    let err = match ResolutionGraph::assemble(Arc::new(lockfile), roots(&[("a", "1.0.0")])) {
        Ok(_) => panic!("dangling edge must fail"),
        Err(err) => err,
    };

    assert!(matches!(
        err,
        PnpError::DanglingEdge {
            name,
            version,
            dep,
            dep_version,
        } if name == PackageName::new("a")
            && version == ver("1.0.0")
            && dep == PackageName::new("b")
            && dep_version == ver("2.0.0")
    ));
}

#[test]
fn verify_cached_checks_presence_but_leaves_integrity_to_cache_reads() {
    let root = tmp_dir("cache");
    let cache = Cache::with_root(root.join("cache"));
    let a_hash = cache.store(b"archive-a").expect("store a");
    let b_hash = cache.store(b"archive-b").expect("store b");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash, &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b_hash.clone(), &[]));
    let graph = ResolutionGraph::assemble(Arc::new(lockfile), roots(&[("a", "1.0.0")]))
        .expect("graph assembles");

    graph.verify_cached(&cache).expect("all blobs are present");

    std::fs::remove_file(cache.path_for(&b_hash)).expect("remove b blob");
    assert!(matches!(
        graph.verify_cached(&cache),
        Err(PnpError::NotCached { name, version })
            if name == PackageName::new("b") && version == ver("1.0.0")
    ));

    std::fs::write(cache.path_for(&b_hash), b"tampered").expect("rewrite tampered blob");
    graph
        .verify_cached(&cache)
        .expect("presence-only check accepts tampered bytes");
    assert!(matches!(
        cache.read(&b_hash),
        Err(CacheError::IntegrityMismatch { .. })
    ));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn assemble_excludes_unreachable_lockfile_orphans() {
    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry(
        "a",
        "1.0.0",
        ContentHash::of(b"a@1.0.0"),
        &[("b", "1.0.0")],
    ));
    lockfile.upsert(lock_entry("b", "1.0.0", ContentHash::of(b"b@1.0.0"), &[]));
    lockfile.upsert(lock_entry(
        "orphan",
        "9.9.9",
        ContentHash::of(b"orphan@9.9.9"),
        &[],
    ));

    let graph = ResolutionGraph::assemble(Arc::new(lockfile), roots(&[("a", "1.0.0")]))
        .expect("graph assembles");
    let packages = package_fingerprint(&graph);

    assert_eq!(packages.len(), 2);
    assert!(packages.iter().all(|(name, _, _)| name != "orphan"));
}

#[test]
fn assemble_is_deterministic_across_insertion_orders() {
    let a_hash = ContentHash::of(b"a@1.0.0");
    let b1_hash = ContentHash::of(b"b@1.0.0");
    let b2_hash = ContentHash::of(b"b@2.0.0");
    let c_hash = ContentHash::of(b"c@1.0.0");

    let mut left_lockfile = Lockfile::new();
    left_lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));
    left_lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    left_lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    left_lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));

    let mut right_lockfile = Lockfile::new();
    right_lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    right_lockfile.upsert(lock_entry("a", "1.0.0", a_hash.clone(), &[("b", "1.0.0")]));
    right_lockfile.upsert(lock_entry("b", "2.0.0", b2_hash.clone(), &[]));
    right_lockfile.upsert(lock_entry("c", "1.0.0", c_hash.clone(), &[("b", "2.0.0")]));

    let mut left_roots = BTreeMap::new();
    left_roots.insert(PackageName::new("c"), ver("1.0.0"));
    left_roots.insert(PackageName::new("a"), ver("1.0.0"));

    let mut right_roots = BTreeMap::new();
    right_roots.insert(PackageName::new("a"), ver("1.0.0"));
    right_roots.insert(PackageName::new("c"), ver("1.0.0"));

    let left = ResolutionGraph::assemble(Arc::new(left_lockfile), left_roots).expect("left graph");
    let right =
        ResolutionGraph::assemble(Arc::new(right_lockfile), right_roots).expect("right graph");

    assert_eq!(package_fingerprint(&left), package_fingerprint(&right));
}

#[test]
fn from_project_pins_roots_and_handles_empty_or_missing_lockfiles() {
    let project = tmp_dir("from-project");
    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", ContentHash::of(b"a@1.0.0"), &[]));
    lockfile.upsert(lock_entry(
        "a",
        "1.2.0",
        ContentHash::of(b"a@1.2.0"),
        &[("b", "1.0.0")],
    ));
    lockfile.upsert(lock_entry("b", "1.0.0", ContentHash::of(b"b@1.0.0"), &[]));
    lockfile
        .write_canonical(&project.join("meow.lock.jsonl"))
        .expect("write canonical lockfile");

    let direct = BTreeMap::from([(PackageName::new("a"), req("^1.0.0"))]);
    let graph = ResolutionGraph::from_project(&project, &direct).expect("graph from project");
    assert_eq!(
        graph.root_deps().get(&PackageName::new("a")),
        Some(&ver("1.2.0"))
    );
    assert_eq!(
        package_fingerprint(&graph)
            .into_iter()
            .map(|(name, version, _)| (name, version))
            .collect::<Vec<_>>(),
        vec![
            ("a".to_owned(), "1.2.0".to_owned()),
            ("b".to_owned(), "1.0.0".to_owned()),
        ]
    );

    let missing = tmp_dir("from-project-missing");
    let empty =
        ResolutionGraph::from_project(&missing, &BTreeMap::new()).expect("missing lockfile");
    assert!(empty.root_deps().is_empty());
    assert_eq!(empty.packages().count(), 0);

    std::fs::write(missing.join("meow.lock.jsonl"), "").expect("write empty lockfile");
    let empty_file =
        ResolutionGraph::from_project(&missing, &BTreeMap::new()).expect("empty lockfile file");
    assert!(empty_file.root_deps().is_empty());
    assert_eq!(empty_file.packages().count(), 0);

    std::fs::remove_dir_all(project).ok();
    std::fs::remove_dir_all(missing).ok();
}
