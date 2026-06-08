use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use meow_pkg::{ContentHash, MaterializeError, ResolutionGraph, UnpackedStore, Version};

#[derive(Debug, Clone)]
pub struct ShadowDeps {
    root: PathBuf,
    links: BTreeMap<String, PathBuf>,
    collisions: BTreeMap<String, Vec<Version>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShadowError {
    #[error("unpacking {name} for the shadow deps map")]
    Unpack {
        name: String,
        #[source]
        source: MaterializeError,
    },
    #[error("writing shadow symlink {} -> {}", link.display(), target.display())]
    Symlink {
        link: PathBuf,
        target: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("preparing the shadow deps dir {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[cfg(windows)]
    #[error(
        "the shadow symlink map needs directory-symlink privilege on Windows; enable \
         Developer Mode or run `meow install --materialize` (LSP-001 prototype is Unix-first)"
    )]
    UnsupportedPlatform,
}

#[derive(Clone)]
struct SelectedPackage {
    integrity: ContentHash,
    version: Version,
    root_dep: bool,
}

impl ShadowDeps {
    #[allow(clippy::result_large_err)]
    pub fn generate(
        project_root: &Path,
        graph: &ResolutionGraph,
        store: &UnpackedStore,
    ) -> Result<ShadowDeps, ShadowError> {
        #[cfg(windows)]
        {
            let _ = (project_root, graph, store);
            return Err(ShadowError::UnsupportedPlatform);
        }
        #[cfg(unix)]
        {
            generate_unix(project_root, graph, store)
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn links(&self) -> &BTreeMap<String, PathBuf> {
        &self.links
    }

    pub fn collisions(&self) -> &BTreeMap<String, Vec<Version>> {
        &self.collisions
    }

    pub fn tsconfig_paths(&self) -> BTreeMap<String, Vec<String>> {
        let mut paths = BTreeMap::from([("*".to_owned(), vec!["./.meow/deps/*".to_owned()])]);
        for name in self.links.keys() {
            if let Some(scope) = package_scope(name) {
                paths
                    .entry(format!("{scope}/*"))
                    .or_insert_with(|| vec![format!("./.meow/deps/{scope}/*")]);
            }
        }
        paths
    }
}

#[cfg(unix)]
#[allow(clippy::result_large_err)]
fn generate_unix(
    project_root: &Path,
    graph: &ResolutionGraph,
    store: &UnpackedStore,
) -> Result<ShadowDeps, ShadowError> {
    let meta_root = project_root.join(".meow");
    let root = meta_root.join("deps");
    let tmp = meta_root.join("deps.tmp");
    ensure_dir(&meta_root)?;
    remove_path_if_exists(&tmp)?;
    ensure_dir(&tmp)?;

    let (selected, mut collisions) = select_packages(graph);
    let mut links = BTreeMap::new();
    for (name, package) in selected {
        let target = match store.ensure(&package.integrity) {
            Ok(target) => target,
            Err(source) => {
                cleanup_best_effort(&tmp);
                return Err(ShadowError::Unpack {
                    name: name.clone(),
                    source,
                });
            }
        };
        let link = shadow_link_path(&tmp, &name);
        if let Some(parent) = link.parent() {
            if let Err(err) = ensure_dir(parent) {
                cleanup_best_effort(&tmp);
                return Err(err);
            }
        }
        if let Err(source) = std::os::unix::fs::symlink(&target, &link) {
            cleanup_best_effort(&tmp);
            return Err(ShadowError::Symlink {
                link,
                target,
                source,
            });
        }
        links.insert(name, target);
    }
    for versions in collisions.values_mut() {
        versions.sort();
    }

    remove_path_if_exists(&root)?;
    if let Err(source) = fs::rename(&tmp, &root) {
        cleanup_best_effort(&tmp);
        return Err(ShadowError::Io {
            path: root.clone(),
            source,
        });
    }

    Ok(ShadowDeps {
        root,
        links,
        collisions,
    })
}

fn select_packages(
    graph: &ResolutionGraph,
) -> (
    BTreeMap<String, SelectedPackage>,
    BTreeMap<String, Vec<Version>>,
) {
    let mut selected = BTreeMap::new();
    let mut collisions = BTreeMap::new();

    for package in graph.packages() {
        let name = package.name.to_string();
        let candidate = SelectedPackage {
            integrity: package.integrity.clone(),
            version: package.version.clone(),
            root_dep: graph.root_deps().get(package.name) == Some(package.version),
        };

        match selected.get_mut(&name) {
            None => {
                selected.insert(name, candidate);
            }
            Some(current) if prefer_candidate(&candidate, current) => {
                collisions
                    .entry(name.clone())
                    .or_insert_with(Vec::new)
                    .push(current.version.clone());
                *current = candidate;
            }
            Some(_) => {
                collisions
                    .entry(name.clone())
                    .or_insert_with(Vec::new)
                    .push(candidate.version);
            }
        }
    }

    (selected, collisions)
}

fn prefer_candidate(candidate: &SelectedPackage, current: &SelectedPackage) -> bool {
    match (candidate.root_dep, current.root_dep) {
        (true, false) => true,
        (false, true) => false,
        _ => candidate.version < current.version,
    }
}

fn shadow_link_path(root: &Path, name: &str) -> PathBuf {
    match scoped_name_parts(name) {
        Some((scope, package)) => root.join(scope).join(package),
        None => root.join(name),
    }
}

fn package_scope(name: &str) -> Option<&str> {
    scoped_name_parts(name).map(|(scope, _)| scope)
}

fn scoped_name_parts(name: &str) -> Option<(&str, &str)> {
    if !name.starts_with('@') {
        return None;
    }
    name.split_once('/')
}

#[allow(clippy::result_large_err)]
fn ensure_dir(path: &Path) -> Result<(), ShadowError> {
    fs::create_dir_all(path).map_err(|source| ShadowError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[allow(clippy::result_large_err)]
fn remove_path_if_exists(path: &Path) -> Result<(), ShadowError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path).map_err(|source| ShadowError::Io {
            path: path.to_path_buf(),
            source,
        }),
        Ok(_) => fs::remove_file(path).map_err(|source| ShadowError::Io {
            path: path.to_path_buf(),
            source,
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ShadowError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn cleanup_best_effort(path: &Path) {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsconfig_paths_include_wildcard_and_scoped_entries() {
        let deps = ShadowDeps {
            root: PathBuf::from("/tmp/project/.meow/deps"),
            links: BTreeMap::from([
                ("plain".to_owned(), PathBuf::from("/cache/plain")),
                ("@scope/pkg".to_owned(), PathBuf::from("/cache/scoped")),
            ]),
            collisions: BTreeMap::new(),
        };

        assert_eq!(
            deps.tsconfig_paths(),
            BTreeMap::from([
                ("*".to_owned(), vec!["./.meow/deps/*".to_owned()]),
                (
                    "@scope/*".to_owned(),
                    vec!["./.meow/deps/@scope/*".to_owned()],
                ),
            ])
        );
    }
}
