//! The CLI's sanctioned **host-boundary seam** (P16 / CONSTITUTION I-6).
//!
//! The binary edge — and ONLY here — may read ambient host configuration. Library
//! crates never read the environment; they receive already-resolved values from
//! this edge. Reading `$HOME` locates the global content-addressed cache; the
//! terminal env feeds the UI engine's capability resolution (UI-001).

use std::ffi::OsString;
use std::path::PathBuf;

/// The host home directory (to locate `~/.meow`), with `MEOW_HOME` overriding
/// `HOME` for child runtimes that must inherit the parent's cache root exactly.
pub(crate) fn host_home() -> PathBuf {
    std::env::var_os("MEOW_HOME")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Optional override for the reference TypeScript compiler path (`meow types`).
pub(crate) fn host_meow_tsc() -> Option<OsString> {
    std::env::var_os("MEOW_TSC")
}

/// Gather terminal-relevant environment once, at the host boundary, into the
/// plain-data `TermEnv` the UI engine resolves capabilities from (UI-001).
pub(crate) fn term_env() -> meow_ui::TermEnv {
    meow_ui::TermEnv {
        no_color: std::env::var_os("NO_COLOR").is_some(),
        force_color: parse_force_color(),
        clicolor_force: std::env::var("CLICOLOR_FORCE")
            .ok()
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false),
        ci: std::env::var_os("CI").is_some(),
        term: std::env::var("TERM").ok(),
        colorterm: std::env::var("COLORTERM").ok(),
        term_program: std::env::var("TERM_PROGRAM").ok(),
        width_hint: std::env::var("COLUMNS").ok().and_then(|v| v.parse().ok()),
        utf8: detect_utf8(),
    }
}

fn parse_force_color() -> Option<u8> {
    let raw = std::env::var("FORCE_COLOR").ok()?;
    Some(match raw.trim() {
        "" | "true" => 3,
        "false" | "0" => 0,
        "1" => 1,
        "2" => 2,
        _ => 3,
    })
}

fn detect_utf8() -> bool {
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(val) = std::env::var(key) {
            if val.is_empty() {
                continue;
            }
            let up = val.to_ascii_uppercase();
            return up.contains("UTF-8") || up.contains("UTF8");
        }
    }
    true
}
