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
///
/// MINIMAL on purpose — `extends` only, NO `include`/`files`. The shadow base
/// (`.meow/tsconfig.json`) owns the entire file set: project sources via its
/// `include` and the ambient strict-web decl via its `files`. A derived config's
/// own `include`/`files` do NOT merge with the base's (they replace per key), so
/// any list here would shadow the base's; keeping the shim list-free lets the
/// generated base govern both. See [`render_tsconfig`] for why the decl needs an
/// explicit `files` entry (TS globs skip dot-dirs like `.meow/`, RT-004).
pub const ROOT_TSCONFIG_SHIM: &str = "{ \"extends\": \"./.meow/tsconfig.json\" }\n";

// === RT-004 ===
/// File name of the curated strict-web ambient decl inside `.meow/`, referenced
/// from the shadow tsconfig's top-level `files` (relative to `.meow/`). The decl
/// CONTENT lives with the runtime that implements those globals
/// (`meow_runtime::web::STRICT_WEB_DTS`) and is threaded in by the CLI via
/// [`write_shadow_types`], so this light config crate keeps NO `meow-runtime`
/// dependency (no V8 here). Deterministic: same meow version ⇒ same path + bytes.
pub const STRICT_WEB_DTS_FILE: &str = "strict-web.d.ts";
// === /RT-004 ===

/// Deterministically render the `.meow/tsconfig.json` body from `cfg`.
///
/// `serde_json`'s default `Map` is a `BTreeMap`, so object keys serialize in a fixed
/// (alphabetical) order — identical `cfg` ⇒ byte-identical output. The result is the
/// [`GENERATED_HEADER`] line, the pretty-printed JSON, and a trailing newline.
///
/// `workspace.packages` deliberately produces no `paths`/`references` here — the
/// `paths` map into `.meow/deps/` is LSP-001's job (CANON §20.1, Q8). CFG-001 emits
/// `compilerOptions` + the RT-004 `files`/`include` set so the file is correct-and-minimal.
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
            "skipLibCheck": true,
            // === RT-004 ===
            // strict-web ambient globals (CANON §8.1): drop the default DOM lib so
            // the typed surface matches the runtime (meow is never a browser). The
            // §8.1 globals come from the curated strict-web decl, loaded via the
            // top-level `files` below — NOT `compilerOptions.types`, which takes
            // @types package names, not a file path (I-1, I-9).
            "lib": ["esnext"],
            // === /RT-004 ===
            // === RT-005 ===
            // Bundled `meow:*` declarations live under `.meow/types/meow/*.d.ts`;
            // `meow sync` refreshes those files so editors resolve `meow:http`
            // without any install step.
            "paths": {
                "meow:*": ["./types/meow/*"]
            }
            // === /RT-005 ===
        },
        // === RT-004 ===
        // The file set lives HERE (the base), not in the committed root shim — the
        // shim is a list-free `extends`, so this base governs the whole program.
        //
        // `files`: load the ambient strict-web decl into every program. `meow sync`
        // writes it next to this file (write_shadow_types). The path is relative to
        // THIS config's dir (`.meow/`), so `./strict-web.d.ts` resolves correctly. It
        // MUST be a `files` entry, not picked up by `include`: TS file globs skip
        // dot-directories, so `.meow/strict-web.d.ts` would never be globbed.
        //
        // `include: [".."]`: the project root, relative to `.meow/`. TS recurses it
        // for sources (excluding dot-dirs + node_modules), so the user's own files
        // are typechecked — without the shim needing its own `include`. The base's
        // `files` + `include` both flow into the minimal root via `extends`.
        "files": [format!("./{STRICT_WEB_DTS_FILE}")],
        "include": [".."]
        // === /RT-004 ===
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

// === RT-004 ===
/// Write each `(relative_path, content)` into `root/.meow/<relative_path>` verbatim,
/// creating parent directories. The shadow tsconfig references these: the ambient
/// strict-web decl via top-level `files` (RT-004), and `meow:*` module declarations
/// via `compilerOptions.paths` (RT-005). The CONTENT is supplied by the caller (the
/// CLI, which owns both `meow-config` and `meow-runtime`), keeping this crate light.
/// `.meow/` is a gitignored, regenerable artifact (CANON §24.5); byte-stable for
/// identical content (I-9).
pub fn write_shadow_types(root: &Path, files: &[(&str, &str)]) -> Result<(), ConfigError> {
    let dir = root.join(".meow");
    for (rel, content) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                action: "create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&path, content).map_err(|source| ConfigError::Io {
            action: "write",
            path,
            source,
        })?;
    }
    Ok(())
}
// === /RT-004 ===

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
