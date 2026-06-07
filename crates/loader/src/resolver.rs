//! THE single module resolver for the whole toolchain (I-1, I-5).
//!
//! The runtime's [`crate::MeowModuleLoader`] and (later, LSP-001) the language
//! server both consume this exact type — there is no second resolver, and no
//! second resolution algorithm, anywhere in the binary. Resolution is split into
//! two layers so the LSP can resolve identity without reading bytes and the loader
//! never reads the same bytes twice:
//!
//! - [`Resolver::locate`] — pure: `specifier` + `referrer` → URL + locator, **no I/O**.
//! - [`Resolver::resolve`] — `locate` + read source (disk for `file:`, the PKG cache
//!   by content hash for cached deps).
//!
//! No `node_modules` is ever created or consulted (I-5): bare specifiers resolve
//! through a constructor-provided `name → ContentHash` map (the stand-in for the
//! lockfile-driven resolution PKG-002 / LOAD-003 complete) straight to the
//! content-addressed cache.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use deno_core::url::Url;
use meow_pkg::{Cache, CacheError, ContentHash};

use crate::url as virtual_url;

/// LOAD-001 handles ESM only. First-party CJS is refused; CJS→ESM wrapping is later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Esm,
}

/// *Where* a resolved module's bytes live — the result of pure resolution, before
/// any source I/O. Keeps the algorithm separable from reading.
#[derive(Debug, Clone)]
pub enum ModuleLocator {
    /// First-party file on disk, addressed by its `file://` URL.
    LocalFile(PathBuf),
    /// Cached dependency, addressed by content hash in the PKG cache (no `node_modules`).
    Cached(ContentHash),
}

/// A fully resolved **and read** module: the contract returned to the loader and to
/// every non-deno_core consumer (tests, future LSP).
#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub url: Url,
    /// Source text. `Arc<str>` matches `GraphDb::set_file(_, Arc<str>)`.
    pub source: Arc<str>,
    pub kind: ModuleKind,
}

/// Typed, causal resolution errors. Every reachable failure is one of these — the
/// resolver never panics on user input (CRAFT Part B). Messages point at the fix.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("could not resolve {specifier:?} from {referrer}")]
    SpecifierNotFound { specifier: String, referrer: Url },
    #[error("bare specifier {name:?} is not resolvable — run `meow install`")]
    BareSpecifierNotInLockfile { name: String },
    #[error(
        "first-party CommonJS is not supported — meow is ESM-only (I-2 / ADR-3); \
         rewrite {} as an ES module (.mjs/.js)",
        .path.display()
    )]
    FirstPartyCjs { path: PathBuf },
    #[error("unsupported module URL scheme {scheme:?} in {url}")]
    UnsupportedScheme { scheme: String, url: Url },
    #[error("reading {}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("module source is not valid UTF-8: {url}")]
    NotUtf8 { url: Url },
    #[error("malformed virtual cache URL: {0}")]
    InvalidVirtualUrl(Url),
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// The one module resolver (I-1, I-5). See module docs.
pub struct Resolver {
    cache: Arc<Cache>,
    /// `name → ContentHash` for resolvable bare specifiers. Constructor-provided
    /// stand-in for lockfile-driven resolution (PKG-002 / LOAD-003).
    bare: HashMap<String, ContentHash>,
    /// `file://` base for first-party resolution; also the default referrer for the
    /// entry/main module.
    project_root: Url,
}

impl Resolver {
    pub fn new(
        cache: Arc<Cache>,
        bare: HashMap<String, ContentHash>,
        project_root: Url,
    ) -> Resolver {
        Resolver {
            cache,
            bare,
            project_root,
        }
    }

    pub fn project_root(&self) -> &Url {
        &self.project_root
    }

    /// Pure resolution: `specifier` + `referrer` → resolved URL + locator, **no I/O**.
    /// The single resolution algorithm lives here; both `resolve()` and deno_core's
    /// sync `resolve` (URL dedup) call it.
    pub fn locate(
        &self,
        specifier: &str,
        referrer: &Url,
    ) -> Result<(Url, ModuleLocator), ResolveError> {
        // 1. Already-absolute URL (deno_core re-entry, or an explicit `file:`/`meow-cache:`).
        if let Ok(url) = Url::parse(specifier) {
            return self.locate_url(url);
        }
        // 2. Relative (`./`, `../`) or rooted (`/`) → join against the referrer.
        if specifier.starts_with("./") || specifier.starts_with("../") || specifier.starts_with('/')
        {
            let url = referrer
                .join(specifier)
                .map_err(|_| ResolveError::SpecifierNotFound {
                    specifier: specifier.to_owned(),
                    referrer: referrer.clone(),
                })?;
            return self.locate_url(url);
        }
        // 3. Bare specifier (`name` or `name/subpath`) → content-addressed cache.
        //    No disk walk, no `node_modules` (I-5).
        let name = package_name(specifier);
        match self.bare.get(name) {
            Some(hash) => Ok((
                virtual_url::encode(hash),
                ModuleLocator::Cached(hash.clone()),
            )),
            None => Err(ResolveError::BareSpecifierNotInLockfile {
                name: name.to_owned(),
            }),
        }
    }

    /// Locate an already-parsed absolute URL (the `file:`/`meow-cache:` cases).
    fn locate_url(&self, url: Url) -> Result<(Url, ModuleLocator), ResolveError> {
        match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| ResolveError::SpecifierNotFound {
                        specifier: url.to_string(),
                        referrer: self.project_root.clone(),
                    })?;
                // First-party CommonJS is refused in every mode (I-2 / ADR-3).
                if is_cjs(&path) {
                    return Err(ResolveError::FirstPartyCjs { path });
                }
                Ok((url, ModuleLocator::LocalFile(path)))
            }
            virtual_url::SCHEME => {
                let hash = virtual_url::decode(&url)?;
                Ok((url, ModuleLocator::Cached(hash)))
            }
            other => Err(ResolveError::UnsupportedScheme {
                scheme: other.to_owned(),
                url,
            }),
        }
    }

    /// THE resolution entrypoint: `locate` then read the source. `LocalFile` reads
    /// from disk; `Cached` reads from the PKG cache by content hash — which
    /// recomputes + verifies the hash before returning bytes (I-7). A cache integrity
    /// failure propagates as [`ResolveError::Cache`], never swallowed.
    pub fn resolve(&self, specifier: &str, referrer: &Url) -> Result<ResolvedModule, ResolveError> {
        let (url, locator) = self.locate(specifier, referrer)?;
        let bytes = match &locator {
            ModuleLocator::LocalFile(path) => {
                std::fs::read(path).map_err(|source| ResolveError::Io {
                    path: path.clone(),
                    source,
                })?
            }
            ModuleLocator::Cached(hash) => self.cache.read(hash)?,
        };
        let source =
            String::from_utf8(bytes).map_err(|_| ResolveError::NotUtf8 { url: url.clone() })?;
        Ok(ResolvedModule {
            url,
            source: Arc::from(source),
            kind: ModuleKind::Esm,
        })
    }
}

/// The package name of a bare specifier: `lodash/fp` → `lodash`, `@scope/p/x` →
/// `@scope/p`. Pure; no allocation (returns a borrow).
fn package_name(specifier: &str) -> &str {
    if let Some(rest) = specifier.strip_prefix('@') {
        // Scoped: keep `@scope/name`, drop any further subpath.
        let mut slashes = rest.match_indices('/');
        slashes.next(); // the `@scope/name` separator stays
        match slashes.next() {
            Some((idx, _)) => &specifier[..idx + 1],
            None => specifier,
        }
    } else {
        match specifier.find('/') {
            Some(idx) => &specifier[..idx],
            None => specifier,
        }
    }
}

/// Whether a path is first-party CommonJS (`.cjs`), which meow refuses (I-2 / ADR-3).
fn is_cjs(path: &std::path::Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("cjs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver_with(bare: HashMap<String, ContentHash>) -> Resolver {
        let root = Url::parse("file:///proj/").unwrap();
        Resolver::new(Arc::new(Cache::with_root("/nonexistent")), bare, root)
    }

    #[test]
    fn resolves_relative_local_file() {
        let r = resolver_with(HashMap::new());
        let referrer = Url::parse("file:///proj/main.ts").unwrap();
        let (url, locator) = r.locate("./a.ts", &referrer).expect("relative resolves");
        assert_eq!(url.as_str(), "file:///proj/a.ts");
        assert!(matches!(locator, ModuleLocator::LocalFile(_)));
    }

    #[test]
    fn resolves_parent_relative_file() {
        let r = resolver_with(HashMap::new());
        let referrer = Url::parse("file:///proj/nested/main.ts").unwrap();
        let (url, _) = r.locate("../a.ts", &referrer).expect("../ resolves");
        assert_eq!(url.as_str(), "file:///proj/a.ts");
    }

    #[test]
    fn resolves_absolute_file_url() {
        let r = resolver_with(HashMap::new());
        let referrer = Url::parse("file:///proj/main.ts").unwrap();
        let (url, locator) = r
            .locate("file:///elsewhere/b.mjs", &referrer)
            .expect("absolute file resolves");
        assert_eq!(url.as_str(), "file:///elsewhere/b.mjs");
        assert!(matches!(locator, ModuleLocator::LocalFile(_)));
    }

    #[test]
    fn resolves_bare_specifier_to_cached_hash() {
        let hash = ContentHash::of(b"export const v = 1;\n");
        let mut bare = HashMap::new();
        bare.insert("dep".to_owned(), hash.clone());
        let r = resolver_with(bare);
        let referrer = Url::parse("file:///proj/main.ts").unwrap();
        let (url, locator) = r.locate("dep", &referrer).expect("bare resolves");
        assert_eq!(url.scheme(), virtual_url::SCHEME);
        match locator {
            ModuleLocator::Cached(h) => assert_eq!(h, hash),
            other => panic!("expected Cached, got {other:?}"),
        }
    }

    #[test]
    fn bare_specifier_subpath_uses_package_name() {
        let hash = ContentHash::of(b"x");
        let mut bare = HashMap::new();
        bare.insert("pkg".to_owned(), hash.clone());
        let r = resolver_with(bare);
        let referrer = Url::parse("file:///proj/main.ts").unwrap();
        let (_, locator) = r.locate("pkg/sub", &referrer).expect("subpath resolves");
        assert!(matches!(locator, ModuleLocator::Cached(_)));
    }

    #[test]
    fn missing_bare_specifier_is_typed_error_not_disk_walk() {
        let r = resolver_with(HashMap::new());
        let referrer = Url::parse("file:///proj/main.ts").unwrap();
        let err = r
            .locate("ghost", &referrer)
            .expect_err("missing dep errors");
        assert!(matches!(
            err,
            ResolveError::BareSpecifierNotInLockfile { name } if name == "ghost"
        ));
    }

    #[test]
    fn first_party_cjs_is_refused() {
        let r = resolver_with(HashMap::new());
        let referrer = Url::parse("file:///proj/main.mjs").unwrap();
        let err = r
            .locate("./legacy.cjs", &referrer)
            .expect_err(".cjs is refused");
        assert!(matches!(err, ResolveError::FirstPartyCjs { .. }));
    }

    #[test]
    fn scoped_package_name_extraction() {
        assert_eq!(package_name("@scope/pkg"), "@scope/pkg");
        assert_eq!(package_name("@scope/pkg/sub"), "@scope/pkg");
        assert_eq!(package_name("lodash"), "lodash");
        assert_eq!(package_name("lodash/fp"), "lodash");
    }
}
