//! Shadow-config generation (ADR-8, CANON §18.1).
//!
//! The human edits only `meow.config.ts`; `meow` regenerates the legacy files the
//! ecosystem hardcodes as derived artifacts. CFG-001 owns the `tsconfig` shadow:
//! the gitignored `.meow/tsconfig.json` the delegated typechecker (ADR-5) consumes,
//! plus the committed one-line root `tsconfig.json` `extends` shim. Generated files
//! are byte-stable for identical input (I-9) and clearly marked so they are never
//! hand-edited.

use crate::load::ConfigError;
use crate::schema::MeowConfig;
use serde_json::json;
use std::path::Path;

/// First line of every generated, gitignored artifact. `tsconfig` is JSONC, so the
/// `//` comment is valid and `tsc`/`tsgo` ignore it. The committed root shim is the
/// one exception — it stays pure one-line JSON (see [`write_root_tsconfig_shim`]).
pub const GENERATED_HEADER: &str = "// GENERATED — do not edit (run: meow sync)";

/// Exact, byte-stable content of the committed root `tsconfig.json` shim (ADR-8).
/// Pure JSON (no header) so every legacy tool reads it; trailing newline included.
pub const ROOT_TSCONFIG_SHIM: &str = "{ \"extends\": \"./.meow/tsconfig.json\" }\n";

/// Deterministically render the `.meow/tsconfig.json` body from `cfg`.
///
/// `serde_json`'s default `Map` is a `BTreeMap`, so object keys serialize in a fixed
/// (alphabetical) order — identical `cfg` ⇒ byte-identical output. The result is the
/// [`GENERATED_HEADER`] line, the pretty-printed JSON, and a trailing newline.
///
/// `workspace.packages` deliberately produces no `paths`/`references` here — the
/// `paths` map into `.meow/deps/` is LSP-001's job (CANON §20.1, Q8). CFG-001 emits
/// only `compilerOptions` + `include` so the file is correct-and-minimal.
fn render_tsconfig(cfg: &MeowConfig) -> String {
    // Mapping table (the only mapping P0 owns):
    //   types.strict                       -> "strict"
    //   runtime.typescript: strip (always) -> verbatimModuleSyntax/isolatedModules/
    //                                          erasableSyntaxOnly/noEmit (I-3)
    //   fixed P0 baseline                  -> module/moduleResolution/target/
    //                                          allowImportingTsExtensions/skipLibCheck
    let value = json!({
        "compilerOptions": {
            "strict": cfg.types.strict,
            "verbatimModuleSyntax": true,
            "isolatedModules": true,
            "erasableSyntaxOnly": true,
            "noEmit": true,
            "module": "esnext",
            "moduleResolution": "bundler",
            "target": "esnext",
            "allowImportingTsExtensions": true,
            "skipLibCheck": true
        },
        "include": ["."]
    });

    // `to_string_pretty` cannot fail on this fully-owned, finite `Value`.
    let body = serde_json::to_string_pretty(&value).unwrap_or_default();
    let mut out = String::with_capacity(GENERATED_HEADER.len() + body.len() + 2);
    out.push_str(GENERATED_HEADER);
    out.push('\n');
    out.push_str(&body);
    out.push('\n');
    out
}

/// Regenerate `root/.meow/tsconfig.json` from `cfg`, creating `.meow/` if absent.
///
/// The file is a derived artifact: any prior content is overwritten. Output is
/// byte-identical for identical `cfg` (idempotent / I-9).
pub fn generate_shadow_tsconfig(cfg: &MeowConfig, root: &Path) -> Result<(), ConfigError> {
    let dir = root.join(".meow");
    std::fs::create_dir_all(&dir).map_err(|source| ConfigError::Io {
        action: "create directory",
        path: dir.clone(),
        source,
    })?;

    let path = dir.join("tsconfig.json");
    let contents = render_tsconfig(cfg);
    std::fs::write(&path, contents).map_err(|source| ConfigError::Io {
        action: "write",
        path,
        source,
    })
}

/// Write the committed root `tsconfig.json` shim iff missing or non-identical.
///
/// Byte-stable: writing twice yields the same bytes, and the second call is a no-op
/// on already-correct content (the existing file is read and compared first, leaving
/// its mtime untouched when nothing changed).
pub fn write_root_tsconfig_shim(root: &Path) -> Result<(), ConfigError> {
    let path = root.join("tsconfig.json");

    match std::fs::read(&path) {
        Ok(existing) if existing == ROOT_TSCONFIG_SHIM.as_bytes() => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(ConfigError::Io {
                action: "read",
                path,
                source,
            });
        }
    }

    std::fs::write(&path, ROOT_TSCONFIG_SHIM).map_err(|source| ConfigError::Io {
        action: "write",
        path,
        source,
    })
}
