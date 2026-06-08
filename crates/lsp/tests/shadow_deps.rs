mod support;

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use meow_lsp::{ShadowDeps, ShadowError};
use meow_pkg::{
    Cache, CacheError, Lockfile, MaterializeError, ResolutionGraph, UnpackedStore, Version,
};

use support::{archive, lock_entry, root_deps, unique_dir};

struct Roots {
    project: PathBuf,
    global: PathBuf,
    cache: Arc<Cache>,
    store: UnpackedStore,
}

fn roots(tag: &str) -> Roots {
    let project = unique_dir(&format!("{tag}-project"));
    let global = unique_dir(&format!("{tag}-global"));
    let cache = Arc::new(Cache::with_root(global.join("cache")));
    let store = UnpackedStore::new(global.join("unpacked"), cache.clone());
    Roots {
        project,
        global,
        cache,
        store,
    }
}

#[test]
fn generate_writes_cache_backed_symlinks_and_is_idempotent() {
    let roots = roots("shadow-ok");
    let plain_manifest =
        br#"{"name":"plain","version":"1.0.0","exports":"./index.js","type":"module"}"#;
    let scoped_manifest =
        br#"{"name":"@scope/name","version":"1.0.0","exports":"./src/index.js","type":"module"}"#;
    let plain_hash = roots
        .cache
        .store(&archive(&[
            ("package.json", plain_manifest),
            ("index.js", b"export const plain = true;\n"),
        ]))
        .expect("store plain");
    let scoped_hash = roots
        .cache
        .store(&archive(&[
            ("package.json", scoped_manifest),
            ("src/index.js", b"export const scoped = true;\n"),
        ]))
        .expect("store scoped");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("plain", "1.0.0", plain_hash.clone(), &[]));
    lockfile.upsert(lock_entry("@scope/name", "1.0.0", scoped_hash.clone(), &[]));
    let graph = ResolutionGraph::assemble(
        Arc::new(lockfile),
        root_deps(&[("plain", "1.0.0"), ("@scope/name", "1.0.0")]),
    )
    .expect("assemble graph");

    let first = ShadowDeps::generate(&roots.project, &graph, &roots.store).expect("first generate");
    let second =
        ShadowDeps::generate(&roots.project, &graph, &roots.store).expect("second generate");

    let plain_link = first.root().join("plain");
    let scoped_dir = first.root().join("@scope");
    let scoped_link = scoped_dir.join("name");
    let store_root = roots
        .store
        .root()
        .canonicalize()
        .expect("canonical store root");
    let plain_target = roots
        .store
        .dir_for(&plain_hash)
        .canonicalize()
        .expect("canonical plain target");
    let scoped_target = roots
        .store
        .dir_for(&scoped_hash)
        .canonicalize()
        .expect("canonical scoped target");
    assert!(plain_link.exists());
    assert!(scoped_dir.is_dir());
    assert!(scoped_link.exists());
    assert_eq!(first.links(), second.links());
    assert_eq!(first.collisions(), second.collisions());
    assert_eq!(
        plain_link.canonicalize().expect("canonical plain link"),
        plain_target
    );
    assert_eq!(
        scoped_link.canonicalize().expect("canonical scoped link"),
        scoped_target
    );
    assert!(plain_link
        .canonicalize()
        .expect("canonical plain link")
        .starts_with(&store_root));
    assert!(!plain_link
        .canonicalize()
        .expect("canonical plain link")
        .starts_with(&roots.project));
    assert_eq!(
        fs::read(plain_link.join("package.json")).expect("read plain manifest"),
        plain_manifest
    );
    assert_eq!(
        fs::read(scoped_link.join("package.json")).expect("read scoped manifest"),
        scoped_manifest
    );
    assert_eq!(
        fs::read(scoped_link.join("src/index.js")).expect("read scoped member"),
        b"export const scoped = true;\n"
    );
    assert!(!roots.project.join("node_modules").exists());

    fs::remove_dir_all(&roots.project).ok();
    fs::remove_dir_all(&roots.global).ok();
}

#[test]
fn regeneration_prunes_removed_dependencies() {
    let roots = roots("shadow-prune");
    let plain_hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"plain","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const plain = true;\n"),
        ]))
        .expect("store plain");
    let stale_hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"stale","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const stale = true;\n"),
        ]))
        .expect("store stale");

    let mut first_lockfile = Lockfile::new();
    first_lockfile.upsert(lock_entry("plain", "1.0.0", plain_hash.clone(), &[]));
    first_lockfile.upsert(lock_entry("stale", "1.0.0", stale_hash, &[]));
    let first_graph = ResolutionGraph::assemble(
        Arc::new(first_lockfile),
        root_deps(&[("plain", "1.0.0"), ("stale", "1.0.0")]),
    )
    .expect("assemble first graph");

    let mut second_lockfile = Lockfile::new();
    second_lockfile.upsert(lock_entry("plain", "1.0.0", plain_hash, &[]));
    let second_graph =
        ResolutionGraph::assemble(Arc::new(second_lockfile), root_deps(&[("plain", "1.0.0")]))
            .expect("assemble second graph");

    ShadowDeps::generate(&roots.project, &first_graph, &roots.store).expect("first generate");
    let second =
        ShadowDeps::generate(&roots.project, &second_graph, &roots.store).expect("second generate");

    assert!(second.root().join("plain").exists());
    assert!(!second.root().join("stale").exists());
    assert_eq!(
        second.links().keys().cloned().collect::<Vec<_>>(),
        vec!["plain".to_owned()]
    );
    assert!(!roots.project.join("node_modules").exists());

    fs::remove_dir_all(&roots.project).ok();
    fs::remove_dir_all(&roots.global).ok();
}

#[test]
fn tampered_blob_returns_unpack_and_leaves_shadow_root_absent() {
    let roots = roots("shadow-tampered");
    let hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"broken","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const broken = true;\n"),
        ]))
        .expect("store broken");
    fs::write(roots.cache.path_for(&hash), b"tampered").expect("tamper cache blob");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("broken", "1.0.0", hash, &[]));
    let graph = ResolutionGraph::assemble(Arc::new(lockfile), root_deps(&[("broken", "1.0.0")]))
        .expect("assemble graph");

    let err = ShadowDeps::generate(&roots.project, &graph, &roots.store)
        .expect_err("tampered blob must refuse generation");
    assert!(matches!(
        err,
        ShadowError::Unpack {
            source: MaterializeError::CacheBlob {
                source: CacheError::IntegrityMismatch { .. },
                ..
            },
            ..
        }
    ));
    assert!(!roots.project.join(".meow/deps").exists());
    assert!(!roots.project.join("node_modules").exists());

    fs::remove_dir_all(&roots.project).ok();
    fs::remove_dir_all(&roots.global).ok();
}

#[test]
fn multi_version_name_records_collision_and_links_lowest_when_not_root_dep() {
    let roots = roots("shadow-collision");
    let a_hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"a","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const a = true;\n"),
        ]))
        .expect("store a");
    let c_hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"c","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const c = true;\n"),
        ]))
        .expect("store c");
    let b1_hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"1.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const b = '1';\n"),
        ]))
        .expect("store b1");
    let b2_hash = roots
        .cache
        .store(&archive(&[
            (
                "package.json",
                br#"{"name":"b","version":"2.0.0","exports":"./index.js","type":"module"}"#,
            ),
            ("index.js", b"export const b = '2';\n"),
        ]))
        .expect("store b2");

    let mut lockfile = Lockfile::new();
    lockfile.upsert(lock_entry("a", "1.0.0", a_hash, &[("b", "1.0.0")]));
    lockfile.upsert(lock_entry("b", "1.0.0", b1_hash.clone(), &[]));
    lockfile.upsert(lock_entry("c", "1.0.0", c_hash, &[("b", "2.0.0")]));
    lockfile.upsert(lock_entry("b", "2.0.0", b2_hash, &[]));
    let graph = ResolutionGraph::assemble(
        Arc::new(lockfile),
        root_deps(&[("a", "1.0.0"), ("c", "1.0.0")]),
    )
    .expect("assemble graph");

    let shadow =
        ShadowDeps::generate(&roots.project, &graph, &roots.store).expect("generate shadow deps");

    assert_eq!(
        shadow
            .root()
            .join("b")
            .canonicalize()
            .expect("canonical b link"),
        roots
            .store
            .dir_for(&b1_hash)
            .canonicalize()
            .expect("canonical b target")
    );
    assert_eq!(
        shadow.collisions().get("b"),
        Some(&vec![Version::parse("2.0.0").expect("valid version")])
    );
    assert!(!roots.project.join("node_modules").exists());

    fs::remove_dir_all(&roots.project).ok();
    fs::remove_dir_all(&roots.global).ok();
}
