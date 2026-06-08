use std::path::PathBuf;
use std::sync::Arc;

use deno_core::url::Url;
use meow_loader::{ModuleLocator, ResolveError, Resolver};
use meow_pkg::{Cache, ResolutionGraph, UnpackedStore};
use meow_runtime::native::NativeModuleSource;

/// Editor-facing entry point over THE shared resolver.
///
/// This stays locate-only by design: editor parity is about resolving a specifier
/// to the same URL + locator the runtime would use. CommonJS refusal happens later,
/// in the runtime load path, after resolution has already agreed.
pub struct EditorResolver {
    resolver: Resolver,
    store: UnpackedStore,
}

#[derive(Debug, Clone)]
pub struct EditorResolution {
    pub url: Url,
    pub locator: ModuleLocator,
    pub editor_path: Option<PathBuf>,
}

impl EditorResolver {
    pub fn new(
        graph: &ResolutionGraph,
        cache: Arc<Cache>,
        project_root: Url,
        native: Arc<dyn NativeModuleSource>,
        store: UnpackedStore,
    ) -> EditorResolver {
        EditorResolver {
            resolver: Resolver::from_resolution(graph, cache, project_root, native),
            store,
        }
    }

    pub fn resolve_module(
        &self,
        specifier: &str,
        referrer: &Url,
    ) -> Result<EditorResolution, ResolveError> {
        let (url, locator) = self.resolver.locate(specifier, referrer)?;
        let editor_path = match &locator {
            ModuleLocator::LocalFile(path) => Some(path.clone()),
            ModuleLocator::Cached { package, member } => {
                Some(self.store.dir_for(package).join(member))
            }
            ModuleLocator::Native { .. } => None,
        };
        Ok(EditorResolution {
            url,
            locator,
            editor_path,
        })
    }

    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use meow_pkg::{
        ContentHash, LockEntry, Lockfile, PackageName, RegistryProvenance, Version, VersionReq,
    };

    fn unique_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "meow-lsp-resolver-{tag}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn parsed_version(text: &str) -> Version {
        Version::parse(text).expect("valid version")
    }

    fn root_deps(entries: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
        entries
            .iter()
            .map(|(name, version)| (PackageName::new(*name), parsed_version(version)))
            .collect()
    }

    fn lock_entry(
        name: &str,
        version: &str,
        integrity: ContentHash,
        deps: &[(&str, &str)],
    ) -> LockEntry {
        LockEntry {
            name: PackageName::new(name),
            version: parsed_version(version),
            integrity,
            dependencies: deps
                .iter()
                .map(|(dep, ver)| (PackageName::new(*dep), parsed_version(ver)))
                .collect(),
            registry: RegistryProvenance::new("https://registry.npmjs.org"),
            capabilities: Vec::new(),
            wasm: Vec::new(),
            meow: VersionReq::parse("*").expect("valid requirement"),
        }
    }

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("package/{path}"), *bytes)
                .expect("append tar member");
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    #[test]
    fn resolve_module_maps_cached_locators_into_the_unpacked_store() {
        let project = unique_dir("cached-path");
        let cache = Arc::new(Cache::with_root(project.join("cache")));
        let store = UnpackedStore::new(project.join("unpacked"), cache.clone());
        let dep_hash = cache
            .store(&archive(&[
                (
                    "package.json",
                    br#"{"name":"dep","version":"1.0.0","exports":"./index.js","type":"module"}"#,
                ),
                ("index.js", b"export const dep = true;\n"),
            ]))
            .expect("store dep");

        let mut lockfile = Lockfile::new();
        lockfile.upsert(lock_entry("dep", "1.0.0", dep_hash.clone(), &[]));
        let graph = ResolutionGraph::assemble(Arc::new(lockfile), root_deps(&[("dep", "1.0.0")]))
            .expect("assemble graph");
        let project_root = Url::from_directory_path(&project).expect("project url");
        let editor = EditorResolver::new(
            &graph,
            cache,
            project_root.clone(),
            meow_runtime::native::native_module_registry(),
            store,
        );
        let referrer = Url::from_file_path(project.join("main.ts")).expect("referrer url");

        let resolved = editor
            .resolve_module("dep", &referrer)
            .expect("resolve dep");

        assert_eq!(
            resolved.editor_path,
            Some(
                project
                    .join("unpacked")
                    .join(dep_hash.to_url_host())
                    .join("index.js")
            )
        );
        assert!(matches!(resolved.locator, ModuleLocator::Cached { .. }));
        assert_eq!(
            resolved.url,
            meow_loader::encode_cache_url(&dep_hash, "index.js")
        );
        assert_eq!(editor.resolver().project_root(), &project_root);

        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn resolve_module_leaves_native_editor_path_empty() {
        let project = unique_dir("native-path");
        let cache = Arc::new(Cache::with_root(project.join("cache")));
        let store = UnpackedStore::new(project.join("unpacked"), cache.clone());
        let graph = ResolutionGraph::assemble(Arc::new(Lockfile::new()), BTreeMap::new())
            .expect("assemble empty graph");
        let editor = EditorResolver::new(
            &graph,
            cache,
            Url::from_directory_path(&project).expect("project url"),
            meow_runtime::native::native_module_registry(),
            store,
        );
        let referrer = Url::from_file_path(project.join("main.ts")).expect("referrer url");

        let resolved = editor
            .resolve_module("meow:http", &referrer)
            .expect("resolve native module");

        assert!(resolved.editor_path.is_none());
        assert!(matches!(resolved.locator, ModuleLocator::Native { .. }));
        assert_eq!(resolved.url.as_str(), "meow:http");

        fs::remove_dir_all(&project).ok();
    }
}
