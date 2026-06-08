use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use deno_core::url::Url;
use deno_core::{
    ModuleLoadOptions, ModuleLoadResponse, ModuleLoader, ModuleSourceCode, ModuleSpecifier,
    RequestedModuleType,
};
use meow_loader::{MeowModuleLoader, ModuleLocator};
use meow_pkg::{ContentHash, LockEntry, PackageName, RegistryProvenance, Version, VersionReq};

pub fn unique_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("meow-lsp-tests-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[allow(dead_code)]
pub fn dir_url(dir: &Path) -> Url {
    Url::from_directory_path(dir).expect("directory URL")
}

pub fn parsed_version(text: &str) -> Version {
    Version::parse(text).expect("valid version")
}

pub fn root_deps(entries: &[(&str, &str)]) -> BTreeMap<PackageName, Version> {
    entries
        .iter()
        .map(|(name, ver)| (PackageName::new(*name), parsed_version(ver)))
        .collect()
}

pub fn lock_entry(
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

pub fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
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

#[allow(dead_code)]
pub fn sync_options() -> ModuleLoadOptions {
    ModuleLoadOptions {
        is_dynamic_import: false,
        is_synchronous: true,
        requested_module_type: RequestedModuleType::None,
    }
}

#[allow(dead_code)]
pub fn load_result(loader: &MeowModuleLoader, spec: &ModuleSpecifier) -> Result<String, String> {
    match loader.load(spec, None, sync_options()) {
        ModuleLoadResponse::Sync(Ok(source)) => match source.code {
            ModuleSourceCode::String(code) => Ok(code.as_str().to_owned()),
            ModuleSourceCode::Bytes(_) => panic!("expected string source"),
        },
        ModuleLoadResponse::Sync(Err(err)) => Err(err.to_string()),
        _ => panic!("expected synchronous load"),
    }
}

#[allow(dead_code)]
pub fn locator_key(locator: &ModuleLocator) -> String {
    match locator {
        ModuleLocator::LocalFile(path) => format!("file:{}", path.display()),
        ModuleLocator::Cached { package, member } => {
            format!("cache:{}:{member}", package.to_sri())
        }
        ModuleLocator::Native { name } => format!("native:{name}"),
    }
}
