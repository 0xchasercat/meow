//! `meow x <package>` — ephemeral package execution (npx/bunx equivalent):
//! install the named package into a transient workspace, run its binary, then
//! discard the workspace. Reuses the runtime driver from `run` and the registry
//! client from `install`.

use std::collections::BTreeMap;
use std::process::ExitCode;

use crate::cli::commands::install::{
    dist_tag_requirement, load_install_package_json, resolve_requested_requirement,
    runtime_meow_requirement, split_package_arg, NpmRegistry,
};
use crate::cli::commands::run::{host_env_map, run_native_request, NativeRunRequest, RunFlagView};
use crate::cli::{hiss, ui, XArgs};
use crate::host;

// === EPHEMERAL-X ===
/// `meow x <package> [-- <args>]` — install a package into a transient temp
/// directory, run its binary immediately, then discard the workspace.
pub fn cmd_x(args: &XArgs) -> ExitCode {
    let u = ui();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(err) => {
            hiss(&format!("meow x: cannot resolve cwd: {err}"));
            return ExitCode::FAILURE;
        }
    };

    // 1. Parse the package spec, handling flags that might be trailing in argv.
    //    This lets users put --trust at the end like `meow x wrangler deploy --trust`.
    //    Also checks MEOW_DANGEROUSLY_DISABLE_SECURITY env var for persistent opt-out.
    let env_trust = std::env::var("MEOW_DANGEROUSLY_DISABLE_SECURITY").is_ok_and(|v| v == "1");
    let mut trust = args.trust || env_trust;
    let mut allow_clock = args.allow_clock || env_trust;
    let mut allow_random = args.allow_random || env_trust;
    let mut allow_env = if env_trust {
        Some(String::new())
    } else {
        args.allow_env.clone()
    };
    let mut package_argv: Vec<String> = Vec::with_capacity(args.argv.len());
    {
        let mut i = 0;
        while i < args.argv.len() {
            let arg = &args.argv[i];
            match arg.as_str() {
                "--trust" => {
                    trust = true;
                    i += 1;
                    continue;
                }
                "--allow-clock" => {
                    allow_clock = true;
                    i += 1;
                    continue;
                }
                "--allow-random" => {
                    allow_random = true;
                    i += 1;
                    continue;
                }
                "--allow-env" => {
                    allow_env = Some(String::new());
                    i += 1;
                    continue;
                }
                a if a.starts_with("--allow-env=") => {
                    allow_env = Some(a.strip_prefix("--allow-env=").unwrap_or("").to_string());
                    i += 1;
                    continue;
                }
                _ => {}
            }
            package_argv.push(args.argv[i].clone());
            i += 1;
        }
    }

    let (name, maybe_req) = match split_package_arg(&args.package) {
        Ok(tuple) => tuple,
        Err(err) => {
            hiss(&format!("meow x: {err}"));
            return ExitCode::FAILURE;
        }
    };

    // 2. Generate a temp workspace path
    let pid = std::process::id();
    let temp_dir = std::env::temp_dir().join(format!("meow-x-{pid}-{}", name.as_str()));
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
    if let Err(err) = std::fs::create_dir_all(&temp_dir) {
        hiss(&format!(
            "meow x: cannot create temp workspace {}: {err}",
            temp_dir.display()
        ));
        return ExitCode::FAILURE;
    }

    // 3. Create a minimal package.json
    let pkg_json_path = temp_dir.join("package.json");
    if let Err(err) = std::fs::write(&pkg_json_path, b"{}\n") {
        hiss(&format!(
            "meow x: cannot write {}: {err}",
            pkg_json_path.display()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        return ExitCode::FAILURE;
    }

    // 4. Resolve the version requirement and add the dependency
    let result: Result<(), String> = (|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("cannot start async runtime: {err}"))?;
        runtime.block_on(async {
            let registry = NpmRegistry::npm()?;
            let req = resolve_dependency_req(&registry, &name, maybe_req).await?;
            meow_config::add_dependency(&temp_dir, name.clone(), req)
                .map_err(|err| err.to_string())?;
            // 5. Install into the temp dir
            u.note(&format!("resolving {}...", name.as_str()));
            let dep_map: BTreeMap<meow_pkg::PackageName, meow_pkg::DepSpec> = {
                let pj = load_install_package_json(&temp_dir)?;
                pj.direct_dependencies().map_err(|e| format!("{e}"))?
            };
            let overrides: BTreeMap<meow_pkg::PackageName, meow_pkg::DepSpec> = {
                let pj = load_install_package_json(&temp_dir)?;
                pj.package_overrides().map_err(|e| format!("{e}"))?
            };
            let cache = std::sync::Arc::new(meow_pkg::Cache::in_home(host::host_home()));
            let meow_req = runtime_meow_requirement()?;

            let installer =
                meow_pkg::Installer::new(registry.clone(), &cache, registry.base_url(), meow_req)
                    .with_overrides(overrides);
            let u = ui();
            // Branded paw spinner in a TTY; inert (no thread, no control chars)
            // when piped/CI, so ephemeral installs never leak ANSI into logs.
            let spinner = u.spinner("resolving dependencies");
            let lockfile = installer
                .resolve_with_progress_async(&dep_map, |progress| {
                    let label = match progress {
                        meow_pkg::InstallProgress::MetadataFetched { package, .. } => {
                            format!("resolving {package}")
                        }
                        meow_pkg::InstallProgress::PackageDownloaded { package, .. } => {
                            format!("downloading {package}")
                        }
                        meow_pkg::InstallProgress::PackageCached { package, .. } => {
                            format!("linking {package}")
                        }
                    };
                    spinner.set_label(label);
                })
                .await
                .map_err(|err| format!("{err}"))?;
            spinner.clear();
            let lock_path = temp_dir.join("meow.lock.jsonl");
            lockfile
                .write_canonical(&lock_path)
                .map_err(|err| format!("{err}"))?;

            let roots =
                meow_pkg::resolve_roots(&dep_map, &lockfile).map_err(|err| format!("{err}"))?;
            let graph = meow_pkg::ResolutionGraph::assemble(std::sync::Arc::new(lockfile), roots)
                .map_err(|err| format!("{err}"))?;
            let projection = meow_pkg::MaterializeOptions::node_modules();
            meow_pkg::Materializer::new(&cache, &graph, &temp_dir)
                .materialize_async(&projection)
                .await
                .map_err(|err| format!("{err}"))?;
            Ok(())
        })
    })();
    if let Err(err) = result {
        hiss(&format!(
            "meow x: failed to install {}: {err}",
            args.package
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        return ExitCode::FAILURE;
    }

    // 6. Look up the binary
    let bin_name = name.as_str().rsplit('/').next().unwrap_or(name.as_str());
    let bin_path = {
        let pkg_json_path = temp_dir
            .join("node_modules")
            .join(name.as_str())
            .join("package.json");
        let bytes = match std::fs::read(&pkg_json_path) {
            Ok(b) => b,
            Err(err) => {
                hiss(&format!(
                    "meow x: cannot find installed package `{}` (expected at {}): {err}",
                    name.as_str(),
                    pkg_json_path.display(),
                ));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        };
        let pkg_json: meow_loader::package::PackageJson = match serde_json::from_slice(&bytes) {
            Ok(pj) => pj,
            Err(err) => {
                hiss(&format!(
                    "meow x: cannot parse manifest for `{}`: {err}",
                    name.as_str(),
                ));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        };
        match pkg_json.bin_entry(bin_name) {
            Some(entry) => pkg_json_path.parent().unwrap().join(entry),
            None => {
                // Fall back to main / index.js
                hiss(&format!(
                    "meow x: package `{}` has no bin entry for `{bin_name}`",
                    name.as_str(),
                ));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        }
    };

    // 7. Print the security envelope
    if trust {
        u.warn(&format!(
            "Executing {} with full host access (--trust).",
            args.package,
        ));
    } else if allow_clock || allow_random || allow_env.is_some() {
        u.purr(&format!(
            "Executing ephemeral package {} with partial host access.",
            args.package,
        ));
    } else {
        u.pounce(&format!(
            "Executing {} in strict isolation. Set MEOW_DANGEROUSLY_DISABLE_SECURITY=1 or pass --trust to bypass.",
            args.package,
        ));
    }

    // 8. Construct and run the request
    let spec = match meow_runtime::ModuleSpecifier::from_file_path(&bin_path) {
        Ok(s) => s,
        Err(_) => {
            hiss(&format!("meow x: invalid bin path: {}", bin_path.display(),));
            let _ = std::fs::remove_dir_all(&temp_dir);
            return ExitCode::FAILURE;
        }
    };
    let project_dir = temp_dir.clone();
    let request = NativeRunRequest {
        project_dir,
        process_cwd: cwd,
        spec,
        main_module: Some(bin_path.to_string_lossy().into_owned()),
        argv1: Some(bin_path.to_string_lossy().into_owned()),
        argv: package_argv.clone(),
    };
    let flags = RunFlagView {
        argv: &package_argv,
        allow_clock,
        allow_random,
        allow_env: &allow_env,
        trust,
        max_old_space_size: args.max_old_space_size,
        no_snapshot: args.no_snapshot,
        v8_flags: args.v8_flags.as_deref(),
    };

    let code = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        // `node:worker_threads` workers run on their own OS threads
        // (`commands::worker`), so the main module drives on a plain
        // current-thread runtime — no `LocalSet` needed.
        Ok(rt) => match rt.block_on(run_native_request(
            &request,
            flags,
            host_env_map(flags.allow_env, meow_runtime::node::NodeMode::Enabled),
        )) {
            Ok(code) => code,
            Err(err) => {
                hiss(&format!("meow x: execution failed: {err}"));
                let _ = std::fs::remove_dir_all(&temp_dir);
                return ExitCode::FAILURE;
            }
        },
        Err(err) => {
            hiss(&format!("meow x: cannot start V8 runtime: {err}"));
            let _ = std::fs::remove_dir_all(&temp_dir);
            return ExitCode::FAILURE;
        }
    };

    // 9. Clean up after the process exits
    if let Err(err) = std::fs::remove_dir_all(&temp_dir) {
        u.out(&u.stdout_caps().muted(&format!(
            "meow x: could not clean up temp workspace {}: {err}",
            temp_dir.display(),
        )));
    }

    code
}

/// Resolve a version requirement from the registry, defaulting to `latest`.
async fn resolve_dependency_req(
    registry: &NpmRegistry,
    name: &meow_pkg::PackageName,
    maybe_req: Option<&str>,
) -> Result<meow_pkg::VersionReq, String> {
    match maybe_req {
        Some(req) => resolve_requested_requirement(registry, name, req).await,
        None => dist_tag_requirement(registry, name, "latest").await,
    }
}
// === /EPHEMERAL-X ===
