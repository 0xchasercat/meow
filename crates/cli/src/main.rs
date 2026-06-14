mod cli;
mod host;

/// Embedded V8 startup snapshot blob (~8 MB), produced by `meow-snapshot`.
/// When empty, the runtime falls back to eager initialization.
static SNAPSHOT_BLOB: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/meow-snapshot.bin"));

fn main() -> std::process::ExitCode {
    let argv = cli::normalize_argv(std::env::args_os().collect());
    <cli::Cli as clap::Parser>::parse_from(argv).run()
}
