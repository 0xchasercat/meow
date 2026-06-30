//! Command dispatchers — split out of `cli.rs` by verb family.
//!
//! Each submodule owns the implementation for one group of `meow` subcommands.
//! `cli.rs` retains only the `clap` definitions, the Omni-Router (`normalize_argv`),
//! the shared host-edge helpers (`ui`/`purr`/`hiss`/`find_project_root`/…), and the
//! `Cli::run` match that dispatches into these entry points.

pub mod info;
pub mod install;
pub mod run;
pub mod search;
pub mod test;
pub mod tool;
pub mod worker;
pub mod x;

pub use info::{
    cmd_doctor, cmd_init, cmd_ls, cmd_sync, cmd_types, cmd_why_dep, cmd_why_large, cmd_why_slow,
};
pub use install::{cmd_add, cmd_install, cmd_remove};
pub use run::{cmd_dev, cmd_node_eval, cmd_run, cmd_task};
pub use search::cmd_search;
pub use test::cmd_test;
pub use tool::{cmd_bundle, cmd_check, cmd_fmt, cmd_lint};
pub use x::cmd_x;
