use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    // clap handles --version/--help (exit 0) and usage errors (exit 2) before we get here.
    <cli::Cli as clap::Parser>::parse().run()
}
