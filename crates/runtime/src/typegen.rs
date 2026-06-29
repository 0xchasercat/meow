//! `meow types` support (RT-005).
//!
//! Declaration emit is delegated to the reference TypeScript compiler (`tsc`):
//! meow owns orchestration, normalization, and the freshness check, not a native
//! `.d.ts` emitter (I-4 / I-9).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::native::NATIVE_MODULES;
use crate::web::STRICT_WEB_DTS;

pub const GENERATED_TYPES_HEADER: &str = "// GENERATED — do not edit (run: meow types)";

/// How the CLI edge wants the library to invoke the TypeScript compiler.
///
/// The library stays free of host reads (I-6): it does not search
/// `node_modules/.bin`, `$PATH`, or any npm cache. The CLI edge picks the
/// strategy and hands it in.
#[derive(Clone)]
pub enum TscCommand {
    /// Run a specific `tsc` binary directly (e.g. an explicit `MEOW_TSC` path).
    Direct(PathBuf),
    /// Dogfood the meow omni-router: `<exe> x tsc -- <tsc flags…>`.
    /// `meow x tsc` resolves the compiler locally (if installed via
    /// `meow add typescript`) or ephemerally — no `.bin` or `$PATH` search.
    MeowX { exe: PathBuf },
}

impl TscCommand {
    fn into_command(self) -> Command {
        match self {
            TscCommand::Direct(path) => Command::new(path),
            TscCommand::MeowX { exe } => {
                let mut cmd = Command::new(exe);
                cmd.arg("x").arg("tsc").arg("--");
                cmd
            }
        }
    }
}

pub struct TypegenEnv<'a> {
    pub project_root: &'a Path,
    pub tsc_command: TscCommand,
}

pub struct TypegenLayout<'a> {
    pub source_dir: &'a Path,
    pub committed_types_dir: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum TypegenError {
    #[error("could not spawn the TypeScript compiler: {source}")]
    Spawn { #[source] source: io::Error },
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
    let mut compiler = env.tsc_command.clone().into_command();
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

    let output = compiler
        .arg("-p")
        .arg(&tsconfig)
        .current_dir(scratch)
        .output()
        .map_err(|source| TypegenError::Spawn { source })?;
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
