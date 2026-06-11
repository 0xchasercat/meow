//! Opaque Deno Node service bundle for runtime extension assembly.
//!
//! `meow-runtime` must not depend on `meow-loader` because the loader already
//! depends on the runtime for native-module source registration. The concrete
//! package graph/cache bridge is therefore supplied by the binary edge as an
//! object implementing [`DenoNodeBridge`]. This module owns the Deno-facing
//! adapter and constructs the real filesystem/sys services that `deno_node`
//! requires.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub use deno_core::url::Url;
pub use deno_core::FastString;
pub use deno_error::JsErrorBox;
pub use deno_fs::FileSystemRc;
pub use deno_napi::DenoRtNativeAddonLoaderRc;
pub use deno_node::{NodeRequireLoader, NodeRequireLoaderRc};
pub use deno_permissions::PermissionsContainer;
pub use deno_semver::Version;
pub use node_resolver::errors::{PackageFolderResolveError, PackageJsonLoadError};
pub use node_resolver::{
    InNpmPackageChecker, NodeResolverOptions, NpmPackageFolderResolver, UrlOrPathRef,
};

/// Concrete system type used by Deno's Node resolver in the runtime.
pub type DenoNodeSys = sys_traits::impls::RealSys;

/// Type-erased, runtime-owned Node service adapter.
pub type DenoNodeExtInitServices =
    deno_node::NodeExtInitServices<DenoNodeBridgeAdapter, DenoNodeBridgeAdapter, DenoNodeSys>;

/// Service bundle consumed by [`crate::node::extensions`].
#[derive(Clone)]
pub struct DenoNodeServices {
    pub node_ext_init: DenoNodeExtInitServices,
    pub fs: FileSystemRc,
    pub native_addon_loader: Option<DenoRtNativeAddonLoaderRc>,
}

/// The host-supplied bridge from meow's package selection model into Deno's
/// path-oriented Node services.
///
/// The concrete implementation lives outside `meow-runtime` to avoid a Cargo
/// cycle. It is expected to resolve packages from the shared graph/cache and to
/// expose paths in the strict symlinked `node_modules` / unpacked-store view.
pub trait DenoNodeBridge:
    NpmPackageFolderResolver + InNpmPackageChecker + NodeRequireLoader + 'static
{
}

impl<T> DenoNodeBridge for T where
    T: NpmPackageFolderResolver + InNpmPackageChecker + NodeRequireLoader + 'static
{
}

/// Cloneable adapter with concrete trait implementations for Deno's generics.
#[derive(Clone)]
pub struct DenoNodeBridgeAdapter {
    bridge: Rc<dyn DenoNodeBridge>,
}

impl DenoNodeBridgeAdapter {
    pub fn new(bridge: Rc<dyn DenoNodeBridge>) -> Self {
        Self { bridge }
    }
}

impl NpmPackageFolderResolver for DenoNodeBridgeAdapter {
    fn resolve_package_folder_from_package(
        &self,
        specifier: &str,
        referrer: &UrlOrPathRef,
    ) -> Result<PathBuf, PackageFolderResolveError> {
        NpmPackageFolderResolver::resolve_package_folder_from_package(
            self.bridge.as_ref(),
            specifier,
            referrer,
        )
    }

    fn resolve_types_package_folder(
        &self,
        types_package_name: &str,
        maybe_package_version: Option<&Version>,
        maybe_referrer: Option<&UrlOrPathRef>,
    ) -> Option<PathBuf> {
        NpmPackageFolderResolver::resolve_types_package_folder(
            self.bridge.as_ref(),
            types_package_name,
            maybe_package_version,
            maybe_referrer,
        )
    }
}

impl InNpmPackageChecker for DenoNodeBridgeAdapter {
    fn in_npm_package(&self, specifier: &Url) -> bool {
        InNpmPackageChecker::in_npm_package(self.bridge.as_ref(), specifier)
    }
}

impl NodeRequireLoader for DenoNodeBridgeAdapter {
    fn ensure_read_permission<'a>(
        &self,
        permissions: &mut PermissionsContainer,
        path: Cow<'a, Path>,
    ) -> Result<Cow<'a, Path>, JsErrorBox> {
        self.bridge.ensure_read_permission(permissions, path)
    }

    fn load_text_file_lossy(&self, path: &Path) -> Result<FastString, JsErrorBox> {
        self.bridge.load_text_file_lossy(path)
    }

    fn is_maybe_cjs(&self, specifier: &Url) -> Result<bool, PackageJsonLoadError> {
        self.bridge.is_maybe_cjs(specifier)
    }

    fn is_maybe_cjs_from_require(&self, specifier: &Url) -> Result<bool, PackageJsonLoadError> {
        self.bridge.is_maybe_cjs_from_require(specifier)
    }

    fn resolve_require_node_module_paths(&self, from: &Path) -> Vec<String> {
        self.bridge.resolve_require_node_module_paths(from)
    }

    fn resolve_package_folder_from_name(&self, package_name: &str) -> Option<PathBuf> {
        self.bridge.resolve_package_folder_from_name(package_name)
    }
}

/// Builder for Deno Node/N-API services using real filesystem access.
pub struct DenoNodeServicesBuilder {
    bridge: Rc<dyn DenoNodeBridge>,
    fs: FileSystemRc,
    native_addon_loader: Option<DenoRtNativeAddonLoaderRc>,
    node_resolver_options: NodeResolverOptions,
}

impl DenoNodeServicesBuilder {
    pub fn new(bridge: Rc<dyn DenoNodeBridge>) -> Self {
        Self {
            bridge,
            fs: real_file_system(),
            native_addon_loader: None,
            node_resolver_options: NodeResolverOptions::default(),
        }
    }

    pub fn with_file_system(mut self, fs: FileSystemRc) -> Self {
        self.fs = fs;
        self
    }

    pub fn with_native_addon_loader(mut self, loader: DenoRtNativeAddonLoaderRc) -> Self {
        self.native_addon_loader = Some(loader);
        self
    }

    pub fn with_node_resolver_options(mut self, options: NodeResolverOptions) -> Self {
        self.node_resolver_options = options;
        self
    }

    pub fn build(self) -> DenoNodeServices {
        let sys = real_node_sys();
        let adapter = DenoNodeBridgeAdapter::new(self.bridge);
        let pkg_json_resolver =
            deno_fs::sync::new_rc(node_resolver::PackageJsonResolver::new(sys.clone(), None));
        let node_resolver = deno_fs::sync::new_rc(deno_node::NodeResolver::new(
            adapter.clone(),
            node_resolver::DenoIsBuiltInNodeModuleChecker,
            adapter.clone(),
            pkg_json_resolver.clone(),
            node_resolver::cache::NodeResolutionSys::new(sys.clone(), None),
            self.node_resolver_options,
        ));
        let node_require_loader: NodeRequireLoaderRc = Rc::new(adapter);

        DenoNodeServices {
            node_ext_init: deno_node::NodeExtInitServices {
                node_require_loader,
                node_resolver,
                pkg_json_resolver,
                sys,
            },
            fs: self.fs,
            native_addon_loader: self.native_addon_loader,
        }
    }
}

pub fn real_file_system() -> FileSystemRc {
    let fs: FileSystemRc = deno_fs::sync::new_rc(deno_fs::RealFs);
    fs
}

pub fn real_node_sys() -> DenoNodeSys {
    DenoNodeSys::default()
}
