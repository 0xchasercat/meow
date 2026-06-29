//! `meow ls` / `types` / `sync` / `why-dep` / `why-large` / `why-slow` / `init` /
//! `doctor` — observability, project lifecycle and config-regeneration verbs.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use meow_ui::Ui;

use crate::cli::{
    cold_start, find_project_root, hiss, load_lockfile, meow_version, purr, ui, InitArgs, PathArgs,
    TypesArgs, WhyDepArgs,
};
use crate::host;

// === LS-001 ===
/// `meow ls` — list active dev servers and processes discovered via `lsof`.
/// Renders a clean table with PID, COMMAND, and PORT columns.
pub fn cmd_ls() -> ExitCode {
    let u = ui();
    let output = match std::process::Command::new("lsof")
        .args(["-i", "-P", "-n"])
        .output()
    {
        Ok(out) if out.status.success() => out.stdout,
        Ok(_) => {
            u.note("meow ls: lsof returned no data (no active servers?)");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            hiss(&format!("meow ls: cannot run lsof: {err}"));
            return ExitCode::FAILURE;
        }
    };

    let text = String::from_utf8_lossy(&output);
    let mut rows: Vec<(u32, String, u16)> = Vec::new();

    // PORT → known-dev-server label map
    let known_ports: std::collections::HashMap<u16, &str> = {
        let mut m = std::collections::HashMap::new();
        m.insert(3000, "Next.js / React");
        m.insert(4321, "Astro");
        m.insert(5173, "Vite");
        m.insert(4173, "Vite Preview");
        m.insert(8000, "Python / Caddy");
        m.insert(8080, "HTTP alt");
        m.insert(1420, "Tauri");
        m.insert(8787, "Wrangler");
        m
    };

    for line in text.lines().skip(1) {
        // lsof -i -P -n output: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 {
            continue;
        }
        let command = parts[0];
        let pid: u32 = match parts[1].parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let name = parts[8];

        // Extract port from "[::1]:5173" or "127.0.0.1:5173" or "*:3000" etc
        if let Some(colon) = name.rfind(':') {
            let port_str = &name[colon + 1..];
            if let Ok(port) = port_str.parse::<u16>() {
                if known_ports.contains_key(&port) {
                    let label = known_ports.get(&port).copied().unwrap_or(command);
                    rows.push((pid, label.to_string(), port));
                }
            }
        }
    }

    if rows.is_empty() {
        u.note("meow ls: no known dev servers detected.");
        return ExitCode::SUCCESS;
    }

    // Deduplicate by (pid, port)
    rows.sort();
    rows.dedup();

    let headers = &["PID", "SERVICE", "PORT"];
    let data: Vec<Vec<String>> = rows
        .iter()
        .map(|(pid, cmd, port)| vec![pid.to_string(), cmd.clone(), port.to_string()])
        .collect();
    let aligns = &[
        meow_ui::table::Align::Right,
        meow_ui::table::Align::Left,
        meow_ui::table::Align::Right,
    ];
    u.table(headers, &data, aligns);
    ExitCode::SUCCESS
}

// === RT-005 ===
fn shadow_type_files() -> Vec<(String, &'static str)> {
    let mut files = Vec::with_capacity(1 + meow_runtime::native::NATIVE_MODULES.len());
    files.push((
        meow_config::STRICT_WEB_DTS_FILE.to_owned(),
        meow_runtime::web::STRICT_WEB_DTS,
    ));
    for name in meow_runtime::native::NATIVE_MODULES {
        if let Some(decl) = meow_runtime::native::native_module_declaration(name) {
            files.push((format!("types/meow/{name}.d.ts"), decl));
        }
    }
    files
}

fn find_runtime_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        if current.join("Cargo.toml").is_file()
            && current.join("crates/runtime/Cargo.toml").is_file()
        {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

pub fn cmd_types(args: &TypesArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow types: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let workspace_root = match find_runtime_workspace_root(&cwd) {
        Some(root) => root,
        None => {
            hiss(&format!(
                "meow types: cannot find the meow workspace root from {}; run this inside the repo checkout",
                cwd.display()
            ));
            return ExitCode::FAILURE;
        }
    };
    let runtime_root = workspace_root.join("crates/runtime");
    let source_dir = runtime_root.join("src/js/meow");
    let committed_types_dir = runtime_root.join("types/meow");

    // Dogfood our own omni-router: by default, `meow x tsc` resolves the
    // TypeScript compiler locally (if installed via `meow add typescript`) or
    // ephemerally. The `MEOW_TSC` override is the escape hatch for CI/builds
    // that have a specific tsc binary available.
    let tsc_command = match host::host_meow_tsc() {
        Some(raw) => {
            let path = PathBuf::from(raw);
            if !path.is_file() {
                hiss(&format!(
                    "meow types: MEOW_TSC points to {}, but that compiler does not exist",
                    path.display()
                ));
                return ExitCode::FAILURE;
            }
            meow_runtime::typegen::TscCommand::Direct(path)
        }
        None => {
            let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("meow"));
            meow_runtime::typegen::TscCommand::MeowX { exe }
        }
    };
    let env = meow_runtime::typegen::TypegenEnv {
        project_root: &workspace_root,
        tsc_command,
    };
    let layout = meow_runtime::typegen::TypegenLayout {
        source_dir: &source_dir,
        committed_types_dir: &committed_types_dir,
    };

    let result = if args.emit {
        meow_runtime::typegen::emit_to_dir(&env, &layout, &committed_types_dir).map(|_| ())
    } else {
        meow_runtime::typegen::check_against_dir(&env, &layout)
    };

    match result {
        Ok(()) => {
            if args.emit {
                purr("meow types: regenerated crates/runtime/types/meow/*.d.ts");
            } else {
                purr("meow types: declarations are fresh");
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            hiss(&format!("meow types: {err}"));
            ExitCode::FAILURE
        }
    }
}

// === CFG-001 ===
/// `meow sync` — regenerate the shadow configs from `meow.config.json` (ADR-8).
/// The binary edge owns host access (cwd) and error rendering; the library
/// (`meow-config`) stays free of ambient reads (I-6).
pub fn cmd_sync() -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow sync: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let cfg = match meow_config::MeowConfig::load(&root) {
        Ok(cfg) => cfg,
        Err(err) => {
            hiss(&format!("meow sync: {err}"));
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = meow_config::generate_shadow_tsconfig(&cfg, &root) {
        hiss(&format!("meow sync: {err}"));
        return ExitCode::FAILURE;
    }
    if let Err(err) = meow_config::write_root_tsconfig_shim(&root) {
        hiss(&format!("meow sync: {err}"));
        return ExitCode::FAILURE;
    }
    // === RT-004 ===
    // Drop the curated strict-web ambient decl into `.meow/` so editors + `meow check`
    // resolve the §8.1 globals (fetch/URL/crypto.subtle/…) with nothing installed. The
    // runtime owns the content (I-9, curated-from-upstream); config owns the shadow dir.
    // === RT-005 ===
    // `meow sync` also refreshes the shipped `meow:*` declarations into `.meow/types/`
    // so editors resolve modules like `meow:http` and `meow:ui` without any install step.
    let shadow_types = shadow_type_files();
    let shadow_refs = shadow_types
        .iter()
        .map(|(path, content)| (path.as_str(), *content))
        .collect::<Vec<_>>();
    if let Err(err) = meow_config::write_shadow_types(&root, &shadow_refs) {
        hiss(&format!("meow sync: {err}"));
        return ExitCode::FAILURE;
    }
    // === /RT-005 ===
    // === /RT-004 ===
    // === CFG-003 ===
    // `package.json` is user-owned after CANON Amendment 001; sync refreshes only
    // the tsconfig/type shadows and never rewrites package.json.
    // === /CFG-003 ===
    purr(
        "meow sync: regenerated .meow/tsconfig.json + .meow/strict-web.d.ts + .meow/types/meow/*.d.ts + tsconfig.json shim",
    );
    ExitCode::SUCCESS
}
// === /CFG-001 ===

// === OBS-001 ===
/// `meow why-dep <name>` — trace the dependency path(s) from the project's direct
/// deps to <name>, read from meow.lock.jsonl (PKG-001). The binary edge owns the
/// one ambient read (cwd); meow-obs stays host-pure + does no resolution (I-6, I-1).
pub fn cmd_why_dep(args: &WhyDepArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => find_project_root(&dir),
        Err(err) => {
            hiss(&format!(
                "meow why-dep: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let lockfile = match load_lockfile(&root) {
        Ok(lf) => lf,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    // === CFG-003 ===
    let package_json = match meow_config::PackageJson::read(&root) {
        Ok(package_json) => package_json,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    let direct = match package_json.direct_dependencies() {
        Ok(direct) => direct,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    let roots = match meow_pkg::resolve_roots(&direct, &lockfile) {
        Ok(roots) => roots,
        Err(err) => {
            hiss(&format!("meow why-dep: {err}"));
            return ExitCode::FAILURE;
        }
    };
    // === /CFG-003 ===
    let mode = if args.shortest {
        meow_obs::PathMode::Shortest
    } else {
        meow_obs::PathMode::All
    };
    let target = meow_pkg::PackageName::new(args.pkg.clone());
    let report = meow_obs::why_dep(&lockfile, &roots, &target, mode, args.limit);

    if args.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(err) => {
                hiss(&format!("meow why-dep: {err}"));
                return ExitCode::FAILURE;
            }
        }
        return if report.found {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    if !report.found {
        hiss(&format!(
            "meow: `{}` is not in the dependency tree (no path from any direct dependency in package.json)",
            args.pkg
        ));
        return ExitCode::FAILURE;
    }
    render_why_dep(&report);
    ExitCode::SUCCESS
}

/// Render a found `why-dep` report as prose chains (stdout).
fn render_why_dep(report: &meow_obs::WhyDep) {
    let mut lines = Vec::new();
    lines.push(format!(
        "{} is in the dependency tree — {} version(s).",
        report.target,
        report.versions.len()
    ));
    lines.push("(each chain starts at a project direct dependency)".to_owned());
    for (idx, tv) in report.versions.iter().enumerate() {
        if idx > 0 {
            lines.push(String::new());
        }
        let integrity = match &tv.integrity {
            Some(hash) => hash.to_sri(),
            None => "none — referenced but not in lockfile".to_string(),
        };
        let direct = if tv.direct {
            "  (direct dependency)"
        } else {
            ""
        };
        lines.push(format!(
            "{}@{}  integrity {integrity}{direct}",
            tv.node.name, tv.node.version
        ));
        for path in &tv.paths {
            let chain = path
                .nodes
                .iter()
                .map(|n| format!("{}@{}", n.name, n.version))
                .collect::<Vec<_>>()
                .join(" → ");
            lines.push(format!("  {chain}"));
        }
        if tv.truncated {
            lines.push(format!(
                "  … showing first {} of more chains (raise with --limit)",
                tv.paths.len()
            ));
        }
    }
    let title = format!("why-dep {}", report.target);
    ui().panel(&title, &lines);
}
// === /OBS-001 ===

// === WHY-LARGE ===
/// `meow why-large` — list packages in the lockfile sorted by cached size,
/// so the user can see which dependencies are the heaviest.
pub fn cmd_why_large(_args: &PathArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow why-large: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let root = find_project_root(&cwd);
    let lockfile = match load_lockfile(&root) {
        Ok(lf) => lf,
        Err(err) => {
            hiss(&format!("meow why-large: {err}"));
            return ExitCode::FAILURE;
        }
    };

    if lockfile.is_empty() {
        purr("meow why-large: lockfile is empty");
        return ExitCode::SUCCESS;
    }

    let cache = meow_pkg::Cache::in_home(host::host_home());
    let mut entries: Vec<(String, u64)> = Vec::new();

    for entry in lockfile.iter() {
        let path = cache.path_for(&entry.integrity);
        let size = match std::fs::metadata(&path) {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        let label = format!("{}@{}", entry.name.as_str(), entry.version.as_str());
        entries.push((label, size));
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.1));

    let total: u64 = entries.iter().map(|(_, s)| s).sum();
    let u = ui();

    let mut lines = vec![u.sigil(
        meow_ui::Tone::Info,
        &format!(
            "{} packages · {} total",
            entries.len(),
            meow_ui::fmt::bytes(total),
        ),
    )];

    // Show top packages
    let max_show = entries.len().min(20);
    for (label, size) in &entries[..max_show] {
        let pct = if total > 0 {
            (*size as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };
        lines.push(format!(
            "{}  {:>6}  {:>3}%",
            u.stdout_caps().muted(&meow_ui::width::pad_end(label, 35)),
            meow_ui::fmt::bytes(*size),
            pct,
        ));
    }

    if entries.len() > max_show {
        lines.push(u.stdout_caps().muted(&format!(
            "… {} more packages not shown",
            entries.len() - max_show
        )));
    }

    u.panel("meow why-large", &lines);
    ExitCode::SUCCESS
}

// === WHY-SLOW ===
/// `meow why-slow [target]` — measure and report cold-start timing: process
/// init, module resolution, and total elapsed.
pub fn cmd_why_slow(args: &PathArgs) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow why-slow: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let cold = cold_start();
    let root = find_project_root(&cwd);
    let u = ui();

    let mut lines = vec![u.sigil(
        meow_ui::Tone::Info,
        &format!("cold start · {}", meow_ui::fmt::duration(cold)),
    )];
    lines.push(u.stdout_caps().muted(&format!(
        "{} process init · {} meow version {}",
        meow_ui::fmt::duration(cold),
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    )));

    // If a target path was provided, measure module resolution time
    if let Some(target) = args.paths.first() {
        let resolve_start = std::time::Instant::now();
        let lockfile = load_lockfile(&root).ok();

        // Quick stat to measure filesystem access

        // Quick stat to measure filesystem access
        let resolve_time = resolve_start.elapsed();
        let meta = std::fs::metadata(target);

        lines.push(String::new());
        match meta {
            Ok(m) => {
                let file_size = meow_ui::fmt::bytes(m.len());
                lines.push(format!(
                    "{}  {}  {}",
                    u.stdout_caps()
                        .muted(&meow_ui::width::pad_end("module", 12)),
                    meow_ui::fmt::duration(resolve_time),
                    file_size,
                ));
                lines.push(format!(
                    "{}  {}",
                    u.stdout_caps().muted(&meow_ui::width::pad_end("path", 12)),
                    target.display(),
                ));
            }
            Err(_) => {
                lines.push(format!("target not found: {}", target.display()));
            }
        }

        if let Some(ref lf) = lockfile {
            if !lf.is_empty() {
                let lock_start = std::time::Instant::now();
                let dep_count = lf.len();
                let lock_time = lock_start.elapsed();
                lines.push(format!(
                    "{}  {}  {} packages",
                    u.stdout_caps()
                        .muted(&meow_ui::width::pad_end("lockfile", 12)),
                    meow_ui::fmt::duration(lock_time),
                    dep_count,
                ));
            }
        }
    }

    u.panel("meow why-slow", &lines);
    ExitCode::SUCCESS
}

// === INIT-001 ===
/// `meow init` — scaffold a new meow project in the current directory.
pub fn cmd_init(args: &InitArgs) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            hiss(&format!(
                "meow init: cannot resolve the current directory: {err}"
            ));
            return ExitCode::FAILURE;
        }
    };

    let config_path = root.join("meow.config.json");
    let package_path = root.join("package.json");

    // Guard: refuse to overwrite unless --force.
    if !args.force {
        if config_path.exists() {
            hiss("meow init: meow.config.json already exists (use --force to overwrite)");
            return ExitCode::FAILURE;
        }
        if package_path.exists() {
            hiss("meow init: package.json already exists (use --force to overwrite)");
            return ExitCode::FAILURE;
        }
    }

    // Write meow.config.json.
    let config_content = match args.mode.as_str() {
        "strict-web" => {
            r#"{ "mode": "strict-web" }
"#
        }
        "node-compat" => {
            r#"{ "mode": "node-compat" }
"#
        }
        _ => {
            hiss(&format!(
                "meow init: unknown mode '{}', expected strict-web or node-compat",
                args.mode
            ));
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = std::fs::write(&config_path, config_content) {
        hiss(&format!("meow init: cannot write meow.config.json: {err}"));
        return ExitCode::FAILURE;
    }

    // Write a minimal package.json if none exists.
    if !package_path.exists() || args.force {
        let dir_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "my-project".to_owned());
        let pkg_content = format!(
            r#"{{
  "name": "{}",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {{
    "dev": "meow run main.ts"
  }}
}}
"#,
            dir_name
        );
        if let Err(err) = std::fs::write(&package_path, pkg_content) {
            hiss(&format!("meow init: cannot write package.json: {err}"));
            return ExitCode::FAILURE;
        }
    }

    // Scaffold a starter main.ts demonstrating native TypeScript + Web APIs.
    let main_ts_path = root.join("main.ts");
    if !main_ts_path.exists() || args.force {
        if let Err(err) = std::fs::write(
            &main_ts_path,
            "\
import { serve } from \"meow:http\";
import { ui } from \"meow:ui\";

ui.purr(\"meow runtime started!\");

serve((req) => {
  return new Response(\" 🎀 🐾 Hello from meow! 🐾 🎀\\n\");
}, { port: 3000 });
",
        ) {
            hiss(&format!("meow init: cannot write main.ts: {err}"));
            return ExitCode::FAILURE;
        }
    }

    // Run `meow sync` to generate shadow configs.
    let sync_status = std::process::Command::new(
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("meow")),
    )
    .arg("sync")
    .current_dir(&root)
    .status();
    match sync_status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            hiss(&format!(
                "meow init: meow sync exited with {}",
                status.code().unwrap_or(1)
            ));
            return ExitCode::FAILURE;
        }
        Err(err) => {
            hiss(&format!("meow init: cannot run meow sync: {err}"));
            return ExitCode::FAILURE;
        }
    }

    // Optionally run `meow install`.
    if !args.no_install {
        let install_status = std::process::Command::new(
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("meow")),
        )
        .arg("install")
        .current_dir(&root)
        .status();
        match install_status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                hiss(&format!(
                    "meow init: meow install exited with {}",
                    status.code().unwrap_or(1)
                ));
                return ExitCode::FAILURE;
            }
            Err(err) => {
                hiss(&format!("meow init: cannot run meow install: {err}"));
                return ExitCode::FAILURE;
            }
        }
    }

    ui().pounce("Project initialized! Run `meow dev` to start the server.");
    ExitCode::SUCCESS
}
// === /INIT-001 ===

/// `meow doctor` — environment, config, and lockfile health as a panel.
pub fn cmd_doctor() -> ExitCode {
    let u = ui();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = find_project_root(&cwd);

    let mut lines = vec![u.sigil(meow_ui::Tone::Info, &format!("meow {}", meow_version()))];

    let has_pkg = root.join("package.json").is_file();
    lines.push(doctor_row(
        &u,
        has_pkg,
        "package.json",
        if has_pkg { "found" } else { "missing" },
    ));

    let lock = root.join("meow.lock.jsonl");
    let (lock_ok, lock_status) = if lock.is_file() {
        match load_lockfile(&root) {
            Ok(lf) => (true, format!("{} packages", lf.len())),
            Err(err) => (false, format!("unreadable: {err}")),
        }
    } else {
        (false, "none — run `meow install`".to_owned())
    };
    lines.push(doctor_row(&u, lock_ok, "lockfile", &lock_status));

    let nm = root.join("node_modules").is_dir();
    lines.push(doctor_row(
        &u,
        nm,
        "node_modules",
        if nm {
            "materialized"
        } else {
            "not materialized"
        },
    ));

    let cache = host::host_home().join(".meow").join("cache");
    lines.push(doctor_row(
        &u,
        cache.is_dir(),
        "cache",
        &cache.display().to_string(),
    ));

    u.panel("meow doctor", &lines);
    ExitCode::SUCCESS
}

fn doctor_row(u: &Ui, ok: bool, key: &str, value: &str) -> String {
    let tone = if ok {
        meow_ui::Tone::Purr
    } else {
        meow_ui::Tone::Warn
    };
    format!(
        "{} {}  {}",
        u.sigil(tone, ""),
        u.stdout_caps().muted(&meow_ui::width::pad_end(key, 13)),
        value
    )
}
