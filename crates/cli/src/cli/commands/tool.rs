//! `meow lint` / `fmt` / `check` / `bundle` — the quality gate + bundler verbs.
//! Lint/fmt/bundle delegate to `meow-tool`; `check` shells out to `tsc` over the
//! shadow config and renders diagnostics through meow-ui.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli::commands::run::build_runtime_context;
use crate::cli::{find_project_root, hiss, purr, ui, BundleArgs, FmtArgs, PathArgs};

// === TOOL-001 ===
pub fn cmd_lint(args: &PathArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow lint: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let report = match meow_tool::lint_paths(&root, &args.paths) {
        Ok(report) => report,
        Err(err) => {
            hiss(&format!("meow lint: {err}"));
            return ExitCode::FAILURE;
        }
    };

    for diag in &report.diagnostics {
        ui().diagnostic(&meow_ui::SourceDiagnostic {
            path: diag.path.to_str().unwrap_or("<invalid path>"),
            source: diag.source.as_ref(),
            span: diag.span,
            message: diag.message.as_str(),
            label: diag.label.as_deref(),
            help: None,
            note: None,
        });
    }

    if report.diagnostics.is_empty() {
        purr("meow lint: no diagnostics");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

pub fn cmd_fmt(args: &FmtArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow fmt: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let report = match meow_tool::format_paths(
        &root,
        &args.paths,
        meow_tool::FormatOptions { check: args.check },
    ) {
        Ok(report) => report,
        Err(err) => {
            hiss(&format!("meow fmt: {err}"));
            return ExitCode::FAILURE;
        }
    };

    for diag in &report.diagnostics {
        ui().diagnostic(&meow_ui::SourceDiagnostic {
            path: diag.path.to_str().unwrap_or("<invalid path>"),
            source: diag.source.as_ref(),
            span: diag.span,
            message: diag.message.as_str(),
            label: diag.label.as_deref(),
            help: None,
            note: None,
        });
    }

    if !report.diagnostics.is_empty() || (args.check && !report.changed.is_empty()) {
        return ExitCode::FAILURE;
    }

    if report.changed.is_empty() {
        if args.check {
            purr("meow fmt: check passed");
        } else {
            purr("meow fmt: no files changed");
        }
    } else {
        purr("meow fmt: formatted files");
    }

    ExitCode::SUCCESS
}

pub fn cmd_bundle(args: &BundleArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow bundle: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    // Validate entries + determine output directory
    let plan = match meow_tool::plan_bundle(&root, &args.entries, args.out.clone()) {
        Ok(plan) => plan,
        Err(err) => {
            hiss(&format!("meow bundle: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let out_dir = plan.out.clone().unwrap_or_else(|| root.join("dist"));

    // Build a resolver from the project context
    let async_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            hiss(&format!("meow bundle: cannot start async runtime: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let result: Result<meow_tool::BundleReport, String> = async_rt.block_on(async {
        let ctx = build_runtime_context(&root, false).map_err(|e| e.to_string())?;
        let resolver = meow_loader::Resolver::from_resolution(
            &ctx.graph,
            ctx.cache.clone(),
            ctx.project_root.clone(),
            meow_runtime::native::native_module_registry(),
        );
        meow_tool::execute_bundle(resolver, &root, &plan.entries, &out_dir)
            .await
            .map_err(|e| e.to_string())
    });

    match result {
        Ok(report) => {
            let chunk_count = report.emitted.len();
            purr(&format!(
                "meow bundle: Emitted {} chunk{} to {}",
                chunk_count,
                if chunk_count == 1 { "" } else { "s" },
                report.out_dir.display(),
            ));
            ExitCode::SUCCESS
        }
        Err(err) => {
            hiss(&format!("meow bundle: {err}"));
            ExitCode::FAILURE
        }
    }
}
// === /TOOL-001 ===

// === ADR-5 ===
/// `meow check` — delegate to tsc over the shadow config and render diagnostics
/// through meow-ui. The shadow tsconfig is generated by `meow sync`.
pub fn cmd_check(args: &PathArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow check: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let root = find_project_root(&cwd);
    let shadow_tsconfig = root.join(".meow/tsconfig.json");

    if !shadow_tsconfig.exists() {
        hiss("meow check: no .meow/tsconfig.json found — run `meow sync` first");
        return ExitCode::FAILURE;
    }

    let targets: Vec<String> = if args.paths.is_empty() {
        Vec::new()
    } else {
        args.paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };

    // Dogfood our own omni-router: `meow x tsc` resolves tsc locally if installed
    // (via `meow add typescript`) or ephemerally if not. No node_modules/.bin or
    // $PATH search — the graph handles binary resolution natively. `--trust`: this
    // is meow's own typechecker on the user's own project (a trusted toolchain
    // path, like `meow run`), not an untrusted npx package, so it must not be
    // fs/net-sandboxed (SEC-001) — tsc reads the whole project + writes tsbuildinfo.
    let meow_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("meow"));
    let mut cmd = std::process::Command::new(meow_exe);
    cmd.arg("x")
        .arg("--trust")
        .arg("tsc")
        .arg("--")
        .arg("--project")
        .arg(&shadow_tsconfig)
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false");
    for target in &targets {
        cmd.arg(target);
    }

    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) => {
            hiss(&format!("meow check: failed to run tsc: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.is_empty() { &stdout } else { &stderr };

    let errors = parse_tsc_diagnostics(combined);

    if errors.is_empty() && output.status.success() {
        purr("meow check: no type errors");
        return ExitCode::SUCCESS;
    }

    for diag in &errors {
        let source = match std::fs::read_to_string(&diag.file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Convert 1-based line/col to byte offset
        let span = line_col_to_span(&source, diag.line, diag.col);
        ui().diagnostic(&meow_ui::diagnostic::SourceDiagnostic {
            path: diag.file_path.to_str().unwrap_or("<unknown>"),
            source: &source,
            span,
            message: &diag.message,
            label: Some(&diag.code),
            help: None,
            note: None,
        });
    }

    if errors.len() == 1 {
        hiss("meow check: found 1 type error");
    } else {
        hiss(&format!("meow check: found {} type errors", errors.len()));
    }
    ExitCode::FAILURE
}

struct TscDiagnostic {
    file_path: std::path::PathBuf,
    line: usize,
    col: usize,
    code: String,
    message: String,
}

/// Parse tsc's --pretty false output: `file(line,col): error TS{code}: {message}`
fn parse_tsc_diagnostics(output: &str) -> Vec<TscDiagnostic> {
    let re = regex::Regex::new(r"^(.+)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$")
        .expect("valid tsc diagnostic regex");
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        if let Some(caps) = re.captures(line) {
            let file_path = std::path::PathBuf::from(caps.get(1).unwrap().as_str());
            let line: usize = caps.get(2).unwrap().as_str().parse().unwrap_or(0);
            let col: usize = caps.get(3).unwrap().as_str().parse().unwrap_or(0);
            let code = caps.get(5).unwrap().as_str().to_string();
            let message = caps.get(6).unwrap().as_str().to_string();
            diagnostics.push(TscDiagnostic {
                file_path,
                line,
                col,
                code,
                message,
            });
        }
    }
    diagnostics
}

/// Convert a 1-based line/column to a byte offset (start, end) span.
/// The end is estimated as the end of the line.
fn line_col_to_span(source: &str, line: usize, col: usize) -> (usize, usize) {
    let mut current_line = 1usize;
    let mut line_start = 0usize;
    for (idx, ch) in source.char_indices() {
        if current_line == line {
            let start = (line_start + col.saturating_sub(1)).min(source.len());
            // Find end of the line for the span
            let end = source[start..]
                .find('\n')
                .map(|rel| start + rel)
                .unwrap_or(source.len());
            return (start, end);
        }
        if ch == '\n' {
            current_line += 1;
            line_start = idx + 1;
        }
    }
    // Fallback: if line is past the end, return (0, 0)
    if current_line == line {
        let start = (line_start + col.saturating_sub(1)).min(source.len());
        return (start, source.len());
    }
    (0, 0)
}
