//! `meow types` support (RT-005).
//!
//! Declaration emit is delegated to the reference TypeScript compiler (`tsc`):
//! meow owns orchestration, normalization, and the freshness check, not a native
//! `.d.ts` emitter (I-4 / I-9).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::native::NATIVE_MODULES;
use crate::web::STRICT_WEB_DTS;

pub const GENERATED_TYPES_HEADER: &str = "// GENERATED — do not edit (run: meow types)";

pub struct TypegenEnv<'a> {
    pub project_root: &'a Path,
    pub home_dir: &'a Path,
    pub meow_tsc: Option<&'a OsStr>,
}

pub struct TypegenLayout<'a> {
    pub source_dir: &'a Path,
    pub committed_types_dir: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum TypegenError {
    #[error("MEOW_TSC points to {path}, but that compiler does not exist")]
    InvalidCompilerOverride { path: PathBuf },
    #[error(
        "could not find the TypeScript compiler; set MEOW_TSC, install typescript in {project}, or use the npm npx cache under {cache}"
    )]
    MissingCompiler { project: PathBuf, cache: PathBuf },
    #[error("could not prepare {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the TypeScript compiler did not emit {path}")]
    MissingOutput { path: PathBuf },
    #[error("meow types: {stderr}")]
    TscFailed { stderr: String },
    #[error(
        "type declarations drifted for {files}; run `meow types --emit` and commit the result"
    )]
    Drift { files: String },
}

pub fn locate_tsc(env: &TypegenEnv<'_>) -> Result<PathBuf, TypegenError> {
    if let Some(raw) = env.meow_tsc {
        let path = PathBuf::from(raw);
        return path
            .is_file()
            .then_some(path.clone())
            .ok_or(TypegenError::InvalidCompilerOverride { path });
    }

    let project_tsc = env
        .project_root
        .join("node_modules")
        .join(".bin")
        .join("tsc");
    if project_tsc.is_file() {
        return Ok(project_tsc);
    }

    let npx_root = env.home_dir.join(".npm").join("_npx");
    if let Ok(entries) = fs::read_dir(&npx_root) {
        let mut candidates = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("node_modules").join(".bin").join("tsc"))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(path) = candidates.into_iter().next() {
            return Ok(path);
        }
    }

    Err(TypegenError::MissingCompiler {
        project: env.project_root.to_path_buf(),
        cache: npx_root,
    })
}

pub fn emit_to_dir(
    env: &TypegenEnv<'_>,
    layout: &TypegenLayout<'_>,
    output_dir: &Path,
) -> Result<Vec<PathBuf>, TypegenError> {
    let emitted = generate_module_types(env, layout)?;
    fs::create_dir_all(output_dir).map_err(|source| TypegenError::CreateDir {
        path: output_dir.to_path_buf(),
        source,
    })?;

    let mut written = Vec::with_capacity(emitted.len());
    for (name, contents) in emitted {
        let path = output_dir.join(format!("{name}.d.ts"));
        fs::write(&path, contents).map_err(|source| TypegenError::Write {
            path: path.clone(),
            source,
        })?;
        written.push(path);
    }
    Ok(written)
}

pub fn check_against_dir(
    env: &TypegenEnv<'_>,
    layout: &TypegenLayout<'_>,
) -> Result<(), TypegenError> {
    let emitted = generate_module_types(env, layout)?;
    let mut drifted = Vec::new();

    for (name, expected) in emitted {
        let path = layout.committed_types_dir.join(format!("{name}.d.ts"));
        let actual = fs::read_to_string(&path).map_err(|source| TypegenError::Read {
            path: path.clone(),
            source,
        })?;
        if actual != expected {
            drifted.push(path.display().to_string());
        }
    }

    if drifted.is_empty() {
        return Ok(());
    }

    drifted.sort();
    Err(TypegenError::Drift {
        files: drifted.join(", "),
    })
}

fn generate_module_types(
    env: &TypegenEnv<'_>,
    layout: &TypegenLayout<'_>,
) -> Result<Vec<(&'static str, String)>, TypegenError> {
    let compiler = locate_tsc(env)?;
    let scratch_guard = make_scratch_dir();
    let scratch = scratch_guard.0.as_path();
    let src_dir = scratch.join("src");
    let native_dir = src_dir.join("meow");
    let out_dir = scratch.join("out");

    fs::create_dir_all(&native_dir).map_err(|source| TypegenError::CreateDir {
        path: native_dir.clone(),
        source,
    })?;
    fs::create_dir_all(&out_dir).map_err(|source| TypegenError::CreateDir {
        path: out_dir.clone(),
        source,
    })?;

    let strict_web = src_dir.join("strict-web.d.ts");
    fs::write(&strict_web, STRICT_WEB_DTS).map_err(|source| TypegenError::Write {
        path: strict_web.clone(),
        source,
    })?;

    for name in NATIVE_MODULES {
        let source_path = layout.source_dir.join(format!("{name}.ts"));
        let source = fs::read_to_string(&source_path).map_err(|source| TypegenError::Read {
            path: source_path,
            source,
        })?;
        let scratch_source = native_dir.join(format!("{name}.ts"));
        fs::write(&scratch_source, source).map_err(|source| TypegenError::Write {
            path: scratch_source,
            source,
        })?;
    }

    let tsconfig = scratch.join("tsconfig.json");
    fs::write(&tsconfig, render_tsconfig()).map_err(|source| TypegenError::Write {
        path: tsconfig.clone(),
        source,
    })?;

    let output = Command::new(&compiler)
        .arg("-p")
        .arg(&tsconfig)
        .current_dir(scratch)
        .output()
        .map_err(|source| TypegenError::Read {
            path: compiler.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(TypegenError::TscFailed {
            stderr: decode_output(&output.stdout, &output.stderr),
        });
    }

    let mut emitted = Vec::with_capacity(NATIVE_MODULES.len());
    for name in NATIVE_MODULES {
        let emitted_path = out_dir.join("meow").join(format!("{name}.d.ts"));
        let raw = fs::read_to_string(&emitted_path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                TypegenError::MissingOutput {
                    path: emitted_path.clone(),
                }
            } else {
                TypegenError::Read {
                    path: emitted_path.clone(),
                    source,
                }
            }
        })?;
        emitted.push((*name, normalize_declaration(&raw)));
    }

    Ok(emitted)
}

fn render_tsconfig() -> String {
    let mut files = Vec::with_capacity(NATIVE_MODULES.len() + 1);
    files.push(json_string("./src/strict-web.d.ts"));
    for name in NATIVE_MODULES {
        files.push(json_string(&format!("./src/meow/{name}.ts")));
    }

    [
        "{\n",
        "  \"compilerOptions\": {\n",
        "    \"allowImportingTsExtensions\": true,\n",
        "    \"declaration\": true,\n",
        "    \"emitDeclarationOnly\": true,\n",
        "    \"lib\": [\"esnext\"],\n",
        "    \"module\": \"esnext\",\n",
        "    \"moduleResolution\": \"bundler\",\n",
        "    \"outDir\": \"./out\",\n",
        "    \"paths\": {\n",
        "      \"meow:*\": [\"./src/meow/*\"]\n",
        "    },\n",
        "    \"rootDir\": \"./src\",\n",
        "    \"skipLibCheck\": true,\n",
        "    \"strict\": true,\n",
        "    \"target\": \"esnext\"\n",
        "  },\n",
        "  \"files\": [\n",
        &format!("    {}\n", files.join(",\n    ")),
        "  ]\n",
        "}\n",
    ]
    .concat()
}

fn normalize_declaration(raw: &str) -> String {
    let body = raw.replace("\r\n", "\n").replace('\r', "\n");
    let body = body.trim_start_matches('\u{feff}');
    let body = body.trim_end_matches('\n');
    let mut out = String::with_capacity(GENERATED_TYPES_HEADER.len() + body.len() + 3);
    out.push_str(GENERATED_TYPES_HEADER);
    out.push('\n');
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }
    out
}

fn decode_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr_text = String::from_utf8_lossy(stderr);
    let stderr_trimmed = stderr_text.trim();
    if !stderr_trimmed.is_empty() {
        return stderr_trimmed.to_owned();
    }
    let stdout_text = String::from_utf8_lossy(stdout);
    let stdout_trimmed = stdout_text.trim();
    if !stdout_trimmed.is_empty() {
        return stdout_trimmed.to_owned();
    }
    "tsc failed without stdout or stderr output".to_owned()
}

/// A scratch dir under the SYSTEM temp (never the user's repo) for the throwaway
/// declaration emit. The returned [`ScratchDir`] removes it on drop, so every exit
/// path (success or error / `?`) cleans up — no `.meow-types-*` litter in the project.
fn make_scratch_dir() -> ScratchDir {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("meow-types-{}-{n}", std::process::id()));
    ScratchDir(path)
}

/// RAII guard: removes the throwaway typegen scratch dir on drop.
struct ScratchDir(PathBuf);

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
