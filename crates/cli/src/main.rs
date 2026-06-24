mod cli;
mod host;

// V8 startup snapshot + residual lazy extension sources, generated at build
// time by build.rs (OUT_DIR/snapshot_data.rs): SNAPSHOT_BLOB, RESIDUAL_LAZY_ESM,
// RESIDUAL_LAZY_JS. If snapshot generation fails, the build fails.
include!(concat!(env!("OUT_DIR"), "/snapshot_data.rs"));

fn main() -> std::process::ExitCode {
    let argv = cli::normalize_argv(std::env::args_os().collect());
    <cli::Cli as clap::Parser>::parse_from(argv).run()
}
