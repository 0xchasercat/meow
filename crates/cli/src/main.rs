use std::process::ExitCode;

mod cli;
mod host;

fn main() -> ExitCode {
    // clap handles --version/--help (exit 0) and usage errors (exit 2) before we get here.
    let argv = cli::normalize_argv(std::env::args_os().collect());
    <cli::Cli as clap::Parser>::parse_from(argv).run()
}
