//! `meow why-dep` path-tracing (OBS-001).
//!
//! A pure, deterministic, network-free path finder over the `meow.lock.jsonl`
//! dependency graph (PKG-001): each `LockEntry.dependencies` is `dep → exact
//! version`, i.e. one directed edge per pair, and the project's direct
//! dependencies (supplied by the caller) are the roots. `why-dep` reports the
//! path(s) from a root down to a target package — the only honest answer to "why
//! is this here?". It builds NO second resolver (I-1): it reads the same graph the
//! one resolver loads from, so its answer matches what actually executes.
//!
//! Honest boundaries (I-11): it reports ONLY what the lockfile proves — version +
//! integrity hash — never capabilities or cost (P6). An absent name is a defined
//! "not found", never a crash; cycles, dangling edges, and empty lockfiles each
//! produce a defined result. No `unwrap`/`panic!`/`unsafe`, no I/O.

// === OBS-001 ===

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use meow_pkg::{ContentHash, Lockfile, PackageName, Version};

/// A resolved node identity: an exact `(name, version)`. Multi-version is inherent
/// — `lodash@3` and `lodash@4` are two distinct nodes (mirrors the lockfile's
/// `(name, version)` keying and LOAD-003's multi-version resolution).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, serde::Serialize)]
pub struct DepNode {
    pub name: PackageName,
    pub version: Version,
}

/// One simple dependency chain. `nodes[0]` is a project direct dependency (a root),
/// `nodes[last]` is the target, and each consecutive pair is a lockfile edge. No
/// node repeats (a SIMPLE path), so a cycle can never make a chain infinite.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
pub struct DepPath {
    pub nodes: Vec<DepNode>,
}

/// All the ways ONE concrete target version is reached.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TargetVersion {
    pub node: DepNode,
    /// Integrity (SRI) from the matching `LockEntry`. `None` ⇒ the `name@version` is
    /// referenced by an edge but has no entry of its own (an incomplete lockfile —
    /// surfaced honestly, never silently dropped).
    pub integrity: Option<ContentHash>,
    /// `true` ⇒ this exact version is itself a project direct dependency.
    pub direct: bool,
    /// Canonical-ordered, de-duplicated chains. `Shortest` holds at most one;
    /// `All` holds every simple path up to `limit`.
    pub paths: Vec<DepPath>,
    /// `true` ⇒ `All` enumeration stopped at `limit` (the chains are a deterministic
    /// prefix, not all of them).
    pub truncated: bool,
}

/// The full answer for `why-dep <name>`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct WhyDep {
    pub target: PackageName,
    /// `true` ⇒ `target` appears in the tree (entry, edge target, or pinned root).
    pub found: bool,
    /// One per matching version, ascending by `version`; empty when `!found`.
    pub versions: Vec<TargetVersion>,
}

/// Path-enumeration mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMode {
    /// Every simple path from a root to the target (the default — a package's full
    /// "why" is every owner that pulls it in).
    All,
    /// One shortest path per target version (multi-source BFS; ties canonical).
    Shortest,
}

/// Default cap on enumerated simple paths per target version (`All`). Simple-path
/// enumeration is worst-case exponential; the cap keeps `why-dep` total and fast
/// and forces an honest "truncated" note rather than a hang.
pub const DEFAULT_PATH_LIMIT: usize = 256;

/// The dependency graph as an adjacency map: each entry node → its outgoing edges
/// (`dep name → exact version`). `BTreeMap` ⇒ sorted, deterministic traversal.
type Adjacency<'a> = BTreeMap<DepNode, &'a BTreeMap<PackageName, Version>>;

/// Explain why `target` is in the dependency tree. Pure, total, deterministic, no
/// I/O. Reads only the lockfile edge set and the provided `roots` (the project's
/// direct deps pinned to exact versions — the caller decides where they come from).
pub fn why_dep(
    lockfile: &Lockfile,
    roots: &BTreeMap<PackageName, Version>,
    target: &PackageName,
    mode: PathMode,
    limit: usize,
) -> WhyDep {
    let adjacency: Adjacency = lockfile
        .iter()
        .map(|e| {
            (
                DepNode {
                    name: e.name.clone(),
                    version: e.version.clone(),
                },
                &e.dependencies,
            )
        })
        .collect();
    let integrity: BTreeMap<DepNode, &ContentHash> = lockfile
        .iter()
        .map(|e| {
            (
                DepNode {
                    name: e.name.clone(),
                    version: e.version.clone(),
                },
                &e.integrity,
            )
        })
        .collect();

    // Every version of `target` reachable as an entry, an edge target, or a root.
    let mut target_vers: BTreeSet<Version> = BTreeSet::new();
    for entry in lockfile.iter() {
        if &entry.name == target {
            target_vers.insert(entry.version.clone());
        }
        for (dep, ver) in &entry.dependencies {
            if dep == target {
                target_vers.insert(ver.clone());
            }
        }
    }
    if let Some(v) = roots.get(target) {
        target_vers.insert(v.clone());
    }

    let root_nodes: Vec<DepNode> = roots
        .iter()
        .map(|(name, version)| DepNode {
            name: name.clone(),
            version: version.clone(),
        })
        .collect();

    let versions = target_vers
        .into_iter()
        .map(|version| {
            let node = DepNode {
                name: target.clone(),
                version: version.clone(),
            };
            let direct = roots.get(target) == Some(&version);
            let (paths, truncated) = match mode {
                PathMode::All => enumerate_all(&adjacency, &root_nodes, &node, limit),
                PathMode::Shortest => (
                    shortest(&adjacency, &root_nodes, &node)
                        .into_iter()
                        .collect(),
                    false,
                ),
            };
            TargetVersion {
                node,
                integrity: integrity
                    .get(&DepNode {
                        name: target.clone(),
                        version,
                    })
                    .map(|h| (*h).clone()),
                direct,
                paths,
                truncated,
            }
        })
        .collect::<Vec<_>>();

    WhyDep {
        target: target.clone(),
        found: !versions.is_empty(),
        versions,
    }
}

/// Every simple path from any root to `target`, capped at `limit`. Cycle-safe: a
/// node already on the current path is skipped, so enumeration always terminates.
fn enumerate_all(
    adjacency: &Adjacency,
    roots: &[DepNode],
    target: &DepNode,
    limit: usize,
) -> (Vec<DepPath>, bool) {
    let mut search = Search {
        adjacency,
        target,
        limit,
        paths: Vec::new(),
        truncated: false,
    };
    for root in roots {
        if search.truncated {
            break;
        }
        let mut path = Vec::new();
        search.dfs(root.clone(), &mut path);
    }
    // Canonical order: identical inputs ⇒ identical bytes (determinism gate).
    search.paths.sort_by(|a, b| a.nodes.cmp(&b.nodes));
    search.paths.dedup();
    (search.paths, search.truncated)
}

struct Search<'a> {
    adjacency: &'a Adjacency<'a>,
    target: &'a DepNode,
    limit: usize,
    paths: Vec<DepPath>,
    truncated: bool,
}

impl Search<'_> {
    fn dfs(&mut self, node: DepNode, path: &mut Vec<DepNode>) {
        if self.truncated {
            return;
        }
        let is_target = &node == self.target;
        path.push(node.clone());
        if is_target {
            // The chain ends AT the target (its own edges are not expanded).
            if self.paths.len() < self.limit {
                self.paths.push(DepPath {
                    nodes: path.clone(),
                });
            } else {
                self.truncated = true;
            }
        } else if let Some(deps) = self.adjacency.get(&node) {
            for (name, version) in deps.iter() {
                let next = DepNode {
                    name: name.clone(),
                    version: version.clone(),
                };
                if !path.contains(&next) {
                    self.dfs(next, path);
                    if self.truncated {
                        break;
                    }
                }
            }
        }
        path.pop();
    }
}

/// One shortest path from the nearest root to `target` (multi-source BFS; ties
/// broken by the sorted root/edge order, so the choice is deterministic).
fn shortest(adjacency: &Adjacency, roots: &[DepNode], target: &DepNode) -> Option<DepPath> {
    let mut visited: BTreeSet<DepNode> = BTreeSet::new();
    let mut pred: BTreeMap<DepNode, DepNode> = BTreeMap::new();
    let mut queue: VecDeque<DepNode> = VecDeque::new();

    for root in roots {
        if root == target {
            return Some(DepPath {
                nodes: vec![target.clone()],
            });
        }
        if visited.insert(root.clone()) {
            queue.push_back(root.clone());
        }
    }

    while let Some(node) = queue.pop_front() {
        let Some(deps) = adjacency.get(&node) else {
            continue;
        };
        for (name, version) in deps.iter() {
            let next = DepNode {
                name: name.clone(),
                version: version.clone(),
            };
            if &next == target {
                // Reconstruct target <- node <- … <- root, then reverse.
                let mut chain = vec![target.clone(), node.clone()];
                let mut cursor = node;
                while let Some(prev) = pred.get(&cursor) {
                    chain.push(prev.clone());
                    cursor = prev.clone();
                }
                chain.reverse();
                return Some(DepPath { nodes: chain });
            }
            if visited.insert(next.clone()) {
                pred.insert(next.clone(), node.clone());
                queue.push_back(next);
            }
        }
    }
    None
}
// === /OBS-001 ===
