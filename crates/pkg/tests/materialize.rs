use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use flate2::write::GzEncoder;
use flate2::Compression;
use meow_pkg::{
    Cache, CacheError, LinkStrategy, LockEntry, Lockfile, MaterializeError, MaterializeOptions,
    Materializer, PackageName, Projection, RegistryProvenance, ResolutionGraph, Version,
    VersionReq,
};
use serde::Deserialize;
use serde_json::json;

fn tmp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "meow-pkg-materialize-{tag}-{}-{n}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn ver(value: &str) -> Version {
    Version::parse(value).expect("valid version")
}

fn req(value: &str) -> VersionReq {
    VersionReq::parse(value).expect("valid requirement")
}

fn package_name(value: &str) -> PackageName {
    PackageName::new(value)
}

fn lock_entry(cache: &Cache, pkg: PackageSpec<'_>) -> LockEntry {
    let archive = package_archive(pkg.name, pkg.version, pkg.files);
    let integrity = cache.store(&archive).expect("store archive");
    LockEntry {
        name: package_name(pkg.name),
        version: ver(pkg.version),
        integrity,
        dependencies: pkg
            .deps
            .iter()
            .map(|(name, version)| (package_name(name), ver(version)))
            .collect(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: req("*"),
    }
}

fn package_archive(name: &str, version: &str, files: &[FileSpec<'_>]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let package_json = json!({ "name": name, "version": version }).to_string();
    append_file(
        &mut builder,
        "package/package.json",
        package_json.as_bytes(),
        0o644,
    );
    for file in files {
        append_file(
            &mut builder,
            &format!("package/{}", file.path),
            file.bytes,
            file.mode,
        );
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

fn append_file(
    builder: &mut tar::Builder<GzEncoder<Vec<u8>>>,
    path: &str,
    bytes: &[u8],
    mode: u32,
) {
    let mut header = tar::Header::new_gnu();
    header.set_mode(mode);
    header.set_mtime(777);
    header.set_size(bytes.len() as u64);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .expect("append tar member");
}

fn build_graph(
    cache: &Cache,
    roots: &[(&str, &str)],
    specs: Vec<PackageSpec<'_>>,
) -> ResolutionGraph {
    let mut lockfile = Lockfile::new();
    for spec in specs {
        lockfile.upsert(lock_entry(cache, spec));
    }
    ResolutionGraph::assemble(
        Arc::new(lockfile),
        roots
            .iter()
            .map(|(name, version)| (package_name(name), ver(version)))
            .collect(),
    )
    .expect("assemble graph")
}

#[derive(Clone, Copy)]
struct FileSpec<'a> {
    path: &'a str,
    bytes: &'a [u8],
    mode: u32,
}

struct PackageSpec<'a> {
    name: &'a str,
    version: &'a str,
    deps: &'a [(&'a str, &'a str)],
    files: &'a [FileSpec<'a>],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotEntry {
    path: String,
    kind: &'static str,
    mode: u32,
    bytes: Vec<u8>,
    target: Option<String>,
}

#[derive(Deserialize)]
struct PackageJson {
    name: String,
    version: String,
}

fn snapshot_tree(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    walk_tree(root, root, &mut entries);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
}

fn walk_tree(root: &Path, path: &Path, out: &mut Vec<SnapshotEntry>) {
    let meta = fs::symlink_metadata(path).expect("metadata");
    let rel = if path == root {
        String::new()
    } else {
        path.strip_prefix(root)
            .expect("relative path")
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mode = mode_bits(&meta);
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path).expect("read link");
        out.push(SnapshotEntry {
            path: rel,
            kind: "symlink",
            mode,
            bytes: vec![],
            target: Some(target.to_string_lossy().replace('\\', "/")),
        });
        return;
    }
    if meta.is_file() {
        out.push(SnapshotEntry {
            path: rel,
            kind: "file",
            mode,
            bytes: fs::read(path).expect("read file"),
            target: None,
        });
        return;
    }
    out.push(SnapshotEntry {
        path: rel.clone(),
        kind: "dir",
        mode,
        bytes: vec![],
        target: None,
    });
    let mut children = fs::read_dir(path)
        .expect("read dir")
        .map(|entry| entry.expect("dir entry").path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        walk_tree(root, &child, out);
    }
}

#[cfg(unix)]
fn mode_bits(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_bits(_meta: &fs::Metadata) -> u32 {
    0
}

fn canonical_package_dir(root: &Path, package_path: &Path) -> PathBuf {
    fs::canonicalize(root.join(package_path)).expect("canonical package dir")
}

fn store_package_dir(root: &Path, name: &str, version: &str) -> PathBuf {
    let key = format!("{}@{}", name.replace('/', "+"), version);
    let mut path = root.join(".meow").join(key).join("node_modules");
    for segment in name.split('/') {
        path.push(segment);
    }
    path
}

fn resolve_in_projection(
    projection_root: &Path,
    start_dir: &Path,
    specifier: &str,
) -> Option<(String, String)> {
    let spec_path = specifier
        .split('/')
        .fold(PathBuf::new(), |mut path, segment| {
            path.push(segment);
            path
        });
    let projection_root = fs::canonicalize(projection_root).ok()?;
    let mut current = fs::canonicalize(start_dir).ok()?;
    loop {
        let candidate = if current == projection_root {
            projection_root.join(&spec_path)
        } else {
            current.join("node_modules").join(&spec_path)
        };
        if candidate.exists() {
            return Some(read_package_identity(&candidate));
        }
        let parent = current.parent()?;
        if current == projection_root {
            return None;
        }
        current = parent.to_path_buf();
    }
}

fn read_package_identity(path: &Path) -> (String, String) {
    let json = fs::read(path.join("package.json")).expect("read package.json");
    let manifest: PackageJson = serde_json::from_slice(&json).expect("parse package.json");
    (manifest.name, manifest.version)
}

#[test]
fn determinism_tree_hash_and_materialized_tree_are_stable() {
    const APP_FILES: &[FileSpec<'_>] = &[
        FileSpec {
            path: "index.js",
            bytes: b"module.exports = require('b')\n",
            mode: 0o644,
        },
        FileSpec {
            path: "bin/run.js",
            bytes: b"#!/usr/bin/env node\n",
            mode: 0o777,
        },
    ];
    const C_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('b')\n",
        mode: 0o644,
    }];
    const B1_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 1\n",
        mode: 0o644,
    }];
    const B2_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 2\n",
        mode: 0o755,
    }];

    let cache = Cache::with_root(tmp_dir("determinism-cache"));
    let graph_a = build_graph(
        &cache,
        &[("a", "1.0.0"), ("c", "1.0.0")],
        vec![
            PackageSpec {
                name: "a",
                version: "1.0.0",
                deps: &[("b", "1.0.0")],
                files: APP_FILES,
            },
            PackageSpec {
                name: "b",
                version: "2.0.0",
                deps: &[],
                files: B2_FILES,
            },
            PackageSpec {
                name: "c",
                version: "1.0.0",
                deps: &[("b", "2.0.0")],
                files: C_FILES,
            },
            PackageSpec {
                name: "b",
                version: "1.0.0",
                deps: &[],
                files: B1_FILES,
            },
        ],
    );
    let graph_b = build_graph(
        &cache,
        &[("c", "1.0.0"), ("a", "1.0.0")],
        vec![
            PackageSpec {
                name: "b",
                version: "1.0.0",
                deps: &[],
                files: B1_FILES,
            },
            PackageSpec {
                name: "c",
                version: "1.0.0",
                deps: &[("b", "2.0.0")],
                files: C_FILES,
            },
            PackageSpec {
                name: "b",
                version: "2.0.0",
                deps: &[],
                files: B2_FILES,
            },
            PackageSpec {
                name: "a",
                version: "1.0.0",
                deps: &[("b", "1.0.0")],
                files: APP_FILES,
            },
        ],
    );
    let root_a = tmp_dir("determinism-root-a");
    let root_b = tmp_dir("determinism-root-b");
    let opts = MaterializeOptions::node_modules();

    let materializer_a = Materializer::new(&cache, &graph_a, &root_a);
    let materializer_b = Materializer::new(&cache, &graph_b, &root_b);
    let plan_a = materializer_a.plan(&opts).expect("plan a");
    let plan_b = materializer_b.plan(&opts).expect("plan b");
    assert_eq!(plan_a.tree_hash(), plan_b.tree_hash());
    assert_eq!(
        plan_a.tree_hash(),
        materializer_a.plan(&opts).expect("repeat plan").tree_hash()
    );

    materializer_a.materialize(&opts).expect("materialize a");
    materializer_b.materialize(&opts).expect("materialize b");
    let snap_a = snapshot_tree(&root_a.join("node_modules"));
    let snap_b = snapshot_tree(&root_b.join("node_modules"));
    assert_eq!(snap_a, snap_b);

    fs::remove_dir_all(cache.root()).ok();
    fs::remove_dir_all(root_a).ok();
    fs::remove_dir_all(root_b).ok();
}

#[test]
fn materialize_is_idempotent_and_reconciles_missing_edges() {
    const APP_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('dep')\n",
        mode: 0o644,
    }];
    const DEP_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 1\n",
        mode: 0o644,
    }];

    let cache = Cache::with_root(tmp_dir("idempotence-cache"));
    let graph = build_graph(
        &cache,
        &[("app", "1.0.0")],
        vec![
            PackageSpec {
                name: "app",
                version: "1.0.0",
                deps: &[("dep", "1.0.0")],
                files: APP_FILES,
            },
            PackageSpec {
                name: "dep",
                version: "1.0.0",
                deps: &[],
                files: DEP_FILES,
            },
        ],
    );
    let root = tmp_dir("idempotence-root");
    let materializer = Materializer::new(&cache, &graph, &root);
    let opts = MaterializeOptions::node_modules();

    let first = materializer.materialize(&opts).expect("first materialize");
    assert!(!first.skipped);
    let snapshot = snapshot_tree(&root.join("node_modules"));

    let second = materializer.materialize(&opts).expect("second materialize");
    assert!(second.skipped);
    assert_eq!(snapshot, snapshot_tree(&root.join("node_modules")));

    let missing_edge = root.join("node_modules/.meow/app@1.0.0/node_modules/dep");
    fs::remove_file(&missing_edge).expect("remove dep edge");
    let third = materializer.materialize(&opts).expect("third materialize");
    assert!(!third.skipped);
    assert_eq!(snapshot, snapshot_tree(&root.join("node_modules")));

    fs::remove_dir_all(cache.root()).ok();
    fs::remove_dir_all(root).ok();
}

#[test]
fn materialized_tree_matches_graph_and_preserves_multi_version_edges() {
    const A_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('b')\n",
        mode: 0o644,
    }];
    const C_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('b')\n",
        mode: 0o644,
    }];
    const B1_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 1\n",
        mode: 0o644,
    }];
    const B2_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 2\n",
        mode: 0o644,
    }];

    let cache = Cache::with_root(tmp_dir("parity-cache"));
    let graph = build_graph(
        &cache,
        &[("a", "1.0.0"), ("c", "1.0.0")],
        vec![
            PackageSpec {
                name: "a",
                version: "1.0.0",
                deps: &[("b", "1.0.0")],
                files: A_FILES,
            },
            PackageSpec {
                name: "b",
                version: "1.0.0",
                deps: &[],
                files: B1_FILES,
            },
            PackageSpec {
                name: "c",
                version: "1.0.0",
                deps: &[("b", "2.0.0")],
                files: C_FILES,
            },
            PackageSpec {
                name: "b",
                version: "2.0.0",
                deps: &[],
                files: B2_FILES,
            },
        ],
    );
    let root = tmp_dir("parity-root");
    Materializer::new(&cache, &graph, &root)
        .materialize(&MaterializeOptions::node_modules())
        .expect("materialize");
    let projection = root.join("node_modules");

    let a_dir = store_package_dir(&projection, "a", "1.0.0");
    let c_dir = store_package_dir(&projection, "c", "1.0.0");
    assert_eq!(
        resolve_in_projection(&projection, &a_dir, "b"),
        Some(("b".to_owned(), "1.0.0".to_owned()))
    );
    assert_eq!(
        resolve_in_projection(&projection, &c_dir, "b"),
        Some(("b".to_owned(), "2.0.0".to_owned()))
    );
    assert!(
        !projection.join("b").exists(),
        "b must not be hoisted to top-level"
    );

    let a_link = projection.join(".meow/a@1.0.0/node_modules/b");
    let c_link = projection.join(".meow/c@1.0.0/node_modules/b");
    assert_eq!(
        fs::read_link(&a_link)
            .expect("a dep target")
            .to_string_lossy(),
        "../../b@1.0.0/node_modules/b"
    );
    assert_eq!(
        fs::read_link(&c_link)
            .expect("c dep target")
            .to_string_lossy(),
        "../../b@2.0.0/node_modules/b"
    );
    assert_eq!(
        resolve_in_projection(&projection, &projection, "a"),
        Some(("a".to_owned(), "1.0.0".to_owned()))
    );
    assert_eq!(
        resolve_in_projection(&projection, &projection, "c"),
        Some(("c".to_owned(), "1.0.0".to_owned()))
    );

    fs::remove_dir_all(cache.root()).ok();
    fs::remove_dir_all(root).ok();
}

#[test]
fn tampered_cache_blob_refuses_to_materialize() {
    const FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 1\n",
        mode: 0o644,
    }];

    let cache = Cache::with_root(tmp_dir("tamper-cache"));
    let graph = build_graph(
        &cache,
        &[("pkg", "1.0.0")],
        vec![PackageSpec {
            name: "pkg",
            version: "1.0.0",
            deps: &[],
            files: FILES,
        }],
    );
    let pkg = graph.packages().next().expect("package");
    fs::write(cache.path_for(pkg.integrity), b"tampered bytes").expect("tamper cache blob");

    let root = tmp_dir("tamper-root");
    let err = Materializer::new(&cache, &graph, &root)
        .materialize(&MaterializeOptions::node_modules())
        .expect_err("tampered cache must fail");
    assert!(matches!(
        err,
        MaterializeError::Cache {
            source: CacheError::IntegrityMismatch { .. },
            ..
        }
    ));
    assert!(!root.join("node_modules").exists());

    fs::remove_dir_all(cache.root()).ok();
    fs::remove_dir_all(root).ok();
}

#[test]
fn truncated_archive_is_invalid_and_commits_nothing() {
    let cache = Cache::with_root(tmp_dir("truncated-cache"));
    let bytes = b"bad archive bytes";
    let integrity = cache.store(bytes).expect("store bytes");
    let mut lockfile = Lockfile::new();
    lockfile.upsert(LockEntry {
        name: package_name("pkg"),
        version: ver("1.0.0"),
        integrity,
        dependencies: BTreeMap::new(),
        registry: RegistryProvenance::new("https://registry.npmjs.org"),
        capabilities: vec![],
        wasm: vec![],
        meow: req("*"),
    });
    let graph = ResolutionGraph::assemble(
        Arc::new(lockfile),
        [(package_name("pkg"), ver("1.0.0"))].into_iter().collect(),
    )
    .expect("graph");
    let root = tmp_dir("truncated-root");

    let err = Materializer::new(&cache, &graph, &root)
        .materialize(&MaterializeOptions::node_modules())
        .expect_err("invalid archive must fail");
    assert!(matches!(err, MaterializeError::InvalidArchive { .. }));
    assert!(!root.join("node_modules").exists());

    fs::remove_dir_all(cache.root()).ok();
    fs::remove_dir_all(root).ok();
}

#[test]
fn vendor_projection_is_self_contained_and_copy_only() {
    const APP_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('dep')\n",
        mode: 0o644,
    }];
    const DEP_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('leaf')\n",
        mode: 0o644,
    }];
    const LEAF_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 42\n",
        mode: 0o644,
    }];

    let cache = Cache::with_root(tmp_dir("vendor-cache"));
    let graph = build_graph(
        &cache,
        &[("app", "1.0.0")],
        vec![
            PackageSpec {
                name: "app",
                version: "1.0.0",
                deps: &[("dep", "1.0.0")],
                files: APP_FILES,
            },
            PackageSpec {
                name: "dep",
                version: "1.0.0",
                deps: &[("leaf", "1.0.0")],
                files: DEP_FILES,
            },
            PackageSpec {
                name: "leaf",
                version: "1.0.0",
                deps: &[],
                files: LEAF_FILES,
            },
        ],
    );
    let root = tmp_dir("vendor-root");
    let mut opts = MaterializeOptions::vendor();
    opts.vendor_dir = PathBuf::from("vendor");
    let report = Materializer::new(&cache, &graph, &root)
        .materialize(&opts)
        .expect("vendor materialize");
    assert!(matches!(opts.projection, Projection::Vendor));
    assert!(matches!(opts.link, LinkStrategy::Copy));
    assert_eq!(report.root, root.join("vendor"));

    let snapshot = snapshot_tree(&root.join("vendor"));
    assert!(snapshot.iter().all(|entry| entry.kind != "symlink"));
    assert_eq!(
        resolve_in_projection(&root.join("vendor"), &root.join("vendor"), "app"),
        Some(("app".to_owned(), "1.0.0".to_owned()))
    );
    let app_dir = canonical_package_dir(&root.join("vendor"), Path::new("app"));
    assert_eq!(
        resolve_in_projection(&root.join("vendor"), &app_dir, "dep"),
        Some(("dep".to_owned(), "1.0.0".to_owned()))
    );
    let dep_dir = app_dir.join("node_modules/dep");
    assert_eq!(
        resolve_in_projection(&root.join("vendor"), &dep_dir, "leaf"),
        Some(("leaf".to_owned(), "1.0.0".to_owned()))
    );

    fs::remove_dir_all(cache.root()).expect("remove cache root");
    assert_eq!(
        read_package_identity(&dep_dir.join("node_modules/leaf")),
        ("leaf".to_owned(), "1.0.0".to_owned())
    );

    fs::remove_dir_all(root).ok();
}

#[test]
fn scoped_package_names_use_escaped_store_keys_and_relative_edges() {
    const CONSUMER_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = require('@scope/pkg')\n",
        mode: 0o644,
    }];
    const SCOPED_FILES: &[FileSpec<'_>] = &[FileSpec {
        path: "index.js",
        bytes: b"module.exports = 'scoped'\n",
        mode: 0o644,
    }];

    let cache = Cache::with_root(tmp_dir("scoped-cache"));
    let graph = build_graph(
        &cache,
        &[("consumer", "1.0.0")],
        vec![
            PackageSpec {
                name: "consumer",
                version: "1.0.0",
                deps: &[("@scope/pkg", "1.2.0")],
                files: CONSUMER_FILES,
            },
            PackageSpec {
                name: "@scope/pkg",
                version: "1.2.0",
                deps: &[],
                files: SCOPED_FILES,
            },
        ],
    );
    let root = tmp_dir("scoped-root");
    Materializer::new(&cache, &graph, &root)
        .materialize(&MaterializeOptions::node_modules())
        .expect("materialize");
    let projection = root.join("node_modules");

    let scoped_store = projection.join(".meow/@scope+pkg@1.2.0/node_modules/@scope/pkg");
    assert!(scoped_store.join("package.json").is_file());
    let edge = projection.join(".meow/consumer@1.0.0/node_modules/@scope/pkg");
    assert_eq!(
        fs::read_link(&edge)
            .expect("scoped dep target")
            .to_string_lossy(),
        "../../../@scope+pkg@1.2.0/node_modules/@scope/pkg"
    );
    let consumer_dir = store_package_dir(&projection, "consumer", "1.0.0");
    assert_eq!(
        resolve_in_projection(&projection, &consumer_dir, "@scope/pkg"),
        Some(("@scope/pkg".to_owned(), "1.2.0".to_owned()))
    );

    fs::remove_dir_all(cache.root()).ok();
    fs::remove_dir_all(root).ok();
}
