//! OBS-001 library tests: `why_dep` over synthetic lockfiles. Proves honest
//! not-found, multi-version grouping, cycle-safety, dangling-edge tolerance,
//! determinism, and bounded enumeration — pure, no I/O (PKG-001 only).

use std::collections::BTreeMap;

use meow_obs::{why_dep, DepNode, PathMode, DEFAULT_PATH_LIMIT};
use meow_pkg::{
    ContentHash, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
};

fn entry(name: &str, version: &str, deps: &[(&str, &str)]) -> LockEntry {
    LockEntry {
        name: PackageName::new(name),
        version: Version::parse(version).expect("version"),
        integrity: ContentHash::of(format!("{name}@{version}").as_bytes()),
        dependencies: deps
            .iter()
            .map(|(n, v)| {
                (
                    PackageName::new(*n),
                    Version::parse(v).expect("dep version"),
                )
            })
            .collect(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: VersionReq::parse(">=0.0.0").expect("req"),
    }
}

fn lockfile(entries: Vec<LockEntry>) -> Lockfile {
    let mut lf = Lockfile::new();
    for e in entries {
        lf.upsert(e);
    }
    lf
}

fn roots(pairs: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
    pairs
        .iter()
        .map(|(n, v)| {
            (
                PackageName::new(*n),
                Version::parse(v).expect("root version"),
            )
        })
        .collect()
}

fn chain(path: &meow_obs::DepPath) -> Vec<(String, String)> {
    path.nodes
        .iter()
        .map(|n| (n.name.to_string(), n.version.to_string()))
        .collect()
}

#[test]
fn multi_level_single_chain() {
    // app -> a@1 -> b@1 -> target@1
    let lf = lockfile(vec![
        entry("a", "1.0.0", &[("b", "1.0.0")]),
        entry("b", "1.0.0", &[("target", "1.0.0")]),
        entry("target", "1.0.0", &[]),
    ]);
    let r = why_dep(
        &lf,
        &roots(&[("a", "1.0.0")]),
        &PackageName::new("target"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert!(r.found);
    assert_eq!(r.versions.len(), 1);
    let tv = &r.versions[0];
    assert!(!tv.direct);
    assert_eq!(tv.paths.len(), 1);
    assert_eq!(
        chain(&tv.paths[0]),
        vec![
            ("a".into(), "1.0.0".into()),
            ("b".into(), "1.0.0".into()),
            ("target".into(), "1.0.0".into())
        ]
    );
}

#[test]
fn all_paths_diamond_vs_shortest() {
    // app -> a@1 -> {b@1, target@1}; b@1 -> target@1  (two paths to target@1)
    let entries = || {
        vec![
            entry("a", "1.0.0", &[("b", "1.0.0"), ("target", "1.0.0")]),
            entry("b", "1.0.0", &[("target", "1.0.0")]),
            entry("target", "1.0.0", &[]),
        ]
    };
    let lf = lockfile(entries());
    let rt = roots(&[("a", "1.0.0")]);

    let all = why_dep(
        &lf,
        &rt,
        &PackageName::new("target"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert_eq!(
        all.versions[0].paths.len(),
        2,
        "all paths reports both chains"
    );

    let short = why_dep(
        &lf,
        &rt,
        &PackageName::new("target"),
        PathMode::Shortest,
        DEFAULT_PATH_LIMIT,
    );
    assert_eq!(
        short.versions[0].paths.len(),
        1,
        "shortest reports one chain"
    );
    assert_eq!(
        chain(&short.versions[0].paths[0]),
        vec![
            ("a".into(), "1.0.0".into()),
            ("target".into(), "1.0.0".into())
        ],
        "shortest is the direct a->target edge"
    );
}

#[test]
fn multi_version_grouped_with_integrity() {
    // a -> target@1 ; b -> target@2  (two distinct target versions)
    let lf = lockfile(vec![
        entry("a", "1.0.0", &[("target", "1.0.0")]),
        entry("b", "1.0.0", &[("target", "2.0.0")]),
        entry("target", "1.0.0", &[]),
        entry("target", "2.0.0", &[]),
    ]);
    let r = why_dep(
        &lf,
        &roots(&[("a", "1.0.0"), ("b", "1.0.0")]),
        &PackageName::new("target"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert_eq!(r.versions.len(), 2);
    assert_eq!(r.versions[0].node.version.to_string(), "1.0.0", "ascending");
    assert_eq!(r.versions[1].node.version.to_string(), "2.0.0");
    assert!(r.versions[0].integrity.is_some() && r.versions[1].integrity.is_some());
}

#[test]
fn cycle_terminates_with_finite_simple_paths() {
    // a@1 -> b@1 -> a@1 (cycle) ; b@1 -> target@1
    let lf = lockfile(vec![
        entry("a", "1.0.0", &[("b", "1.0.0")]),
        entry("b", "1.0.0", &[("a", "1.0.0"), ("target", "1.0.0")]),
        entry("target", "1.0.0", &[]),
    ]);
    // The test itself would hang on a naive (non-simple-path) recursion.
    let r = why_dep(
        &lf,
        &roots(&[("a", "1.0.0")]),
        &PackageName::new("target"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert!(r.found);
    let tv = &r.versions[0];
    assert_eq!(
        tv.paths.len(),
        1,
        "the only simple path is a -> b -> target"
    );
    for p in &tv.paths {
        let mut seen = std::collections::BTreeSet::new();
        assert!(
            p.nodes.iter().all(|n| seen.insert(n.clone())),
            "no node repeats"
        );
    }
}

#[test]
fn honest_not_found() {
    let lf = lockfile(vec![entry("a", "1.0.0", &[])]);
    let r = why_dep(
        &lf,
        &roots(&[("a", "1.0.0")]),
        &PackageName::new("ghost"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert!(!r.found);
    assert!(r.versions.is_empty());
}

#[test]
fn empty_lockfile_empty_roots_is_not_found_no_panic() {
    let r = why_dep(
        &Lockfile::new(),
        &BTreeMap::new(),
        &PackageName::new("x"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert!(!r.found);
}

#[test]
fn dangling_edge_is_endpoint_with_no_integrity() {
    // a -> target@9, but target@9 has no entry of its own (incomplete lockfile).
    let lf = lockfile(vec![entry("a", "1.0.0", &[("target", "9.0.0")])]);
    let r = why_dep(
        &lf,
        &roots(&[("a", "1.0.0")]),
        &PackageName::new("target"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    assert!(r.found);
    let tv = &r.versions[0];
    assert!(tv.integrity.is_none(), "referenced but not in lockfile");
    assert_eq!(
        chain(&tv.paths[0]),
        vec![
            ("a".into(), "1.0.0".into()),
            ("target".into(), "9.0.0".into())
        ]
    );
}

#[test]
fn direct_dependency_is_trivial_chain() {
    let lf = lockfile(vec![entry("target", "1.0.0", &[])]);
    let r = why_dep(
        &lf,
        &roots(&[("target", "1.0.0")]),
        &PackageName::new("target"),
        PathMode::All,
        DEFAULT_PATH_LIMIT,
    );
    let tv = &r.versions[0];
    assert!(tv.direct);
    assert_eq!(tv.paths.len(), 1);
    assert_eq!(
        tv.paths[0].nodes,
        vec![DepNode {
            name: PackageName::new("target"),
            version: Version::parse("1.0.0").unwrap()
        }]
    );
}

#[test]
fn deterministic_across_insertion_order() {
    let forward = lockfile(vec![
        entry("a", "1.0.0", &[("b", "1.0.0"), ("target", "1.0.0")]),
        entry("b", "1.0.0", &[("target", "1.0.0")]),
        entry("target", "1.0.0", &[]),
    ]);
    let reversed = lockfile(vec![
        entry("target", "1.0.0", &[]),
        entry("b", "1.0.0", &[("target", "1.0.0")]),
        entry("a", "1.0.0", &[("target", "1.0.0"), ("b", "1.0.0")]),
    ]);
    let rt = roots(&[("a", "1.0.0")]);
    let t = PackageName::new("target");
    let a = why_dep(&forward, &rt, &t, PathMode::All, DEFAULT_PATH_LIMIT);
    let b = why_dep(&reversed, &rt, &t, PathMode::All, DEFAULT_PATH_LIMIT);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
        "identical logical lockfile ⇒ byte-identical report"
    );
}

#[test]
fn truncation_sets_flag_and_caps_paths() {
    // A fan-out of many distinct roots all reaching target@1 -> > limit paths.
    let mut entries = vec![entry("target", "1.0.0", &[])];
    let mut root_pairs = Vec::new();
    for i in 0..10 {
        let name = format!("r{i}");
        entries.push(entry(&name, "1.0.0", &[("target", "1.0.0")]));
        root_pairs.push((name, "1.0.0".to_string()));
    }
    let lf = lockfile(entries);
    let rt: BTreeMap<PackageName, Version> = root_pairs
        .iter()
        .map(|(n, v)| (PackageName::new(n.clone()), Version::parse(v).unwrap()))
        .collect();
    let r = why_dep(&lf, &rt, &PackageName::new("target"), PathMode::All, 4);
    let tv = &r.versions[0];
    assert!(tv.truncated, "10 paths > limit 4 ⇒ truncated");
    assert_eq!(tv.paths.len(), 4, "exactly limit chains kept");
    // Deterministic prefix: same inputs ⇒ same kept set.
    let r2 = why_dep(&lf, &rt, &PackageName::new("target"), PathMode::All, 4);
    assert_eq!(
        serde_json::to_string(&r.versions[0].paths).unwrap(),
        serde_json::to_string(&r2.versions[0].paths).unwrap()
    );
}
