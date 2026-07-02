#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;
mod host;

// V8 startup snapshot + residual lazy extension sources, generated at build
// time by build.rs (OUT_DIR/snapshot_data.rs): SNAPSHOT_BLOB, RESIDUAL_LAZY_ESM,
// RESIDUAL_LAZY_JS. If snapshot generation fails, the build fails.
include!(concat!(env!("OUT_DIR"), "/snapshot_data.rs"));

fn main() -> std::process::ExitCode {
    cli::mark_start();
    let raw_argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if let Some(exit) = cli::maybe_print_command_suggestion(&raw_argv) {
        return exit;
    }
    let argv = cli::normalize_argv(raw_argv);
    <cli::Cli as clap::Parser>::parse_from(argv).run()
}
