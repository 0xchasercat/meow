//! The IO facade. Owns capability detection + stream choice and exposes every
//! surface as a method. It never captures the user program's stdout/stderr; it
//! only writes meow's own chrome. Pure renderers stay in their modules; this is
//! the one place that touches real streams + the animation gate.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use crate::banner::{self, CommandGroup};
use crate::caps::{Caps, TermEnv};
use crate::diagnostic::{self, SourceDiagnostic};
use crate::glyph::Glyphs;
use crate::paint::ColorLevel;
use crate::progress::ProgressBar;
use crate::spinner::Spinner;
use crate::status::{self, Tone};
use crate::{panel, table, waterfall};

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    stdout: Caps,
    stderr: Caps,
    animate: bool,
}

impl Ui {
    /// The CLI path: resolve from gathered env + live tty state.
    pub fn from_env(env: &TermEnv) -> Ui {
        let stdout = Caps::resolve(env, io::stdout().is_terminal());
        let stderr = Caps::resolve(env, io::stderr().is_terminal());
        let animate = io::stderr().is_terminal() && stderr.color.is_on() && !env.ci;
        Ui {
            stdout,
            stderr,
            animate,
        }
    }

    /// Env-free detection (live tty only, assume UTF-8). Used by the runtime
    /// bridge, which must not read ambient host env.
    pub fn auto() -> Ui {
        let env = TermEnv {
            utf8: true,
            ..TermEnv::default()
        };
        let stdout = Caps::resolve(&env, io::stdout().is_terminal());
        let stderr = Caps::resolve(&env, io::stderr().is_terminal());
        let animate = io::stderr().is_terminal() && stderr.color.is_on();
        Ui {
            stdout,
            stderr,
            animate,
        }
    }

    /// Fully plain: ascii, no color, no animation. Pipes/CI/test baseline.
    pub const fn plain() -> Ui {
        Ui {
            stdout: Caps::plain(),
            stderr: Caps::plain(),
            animate: false,
        }
    }

    /// No color, but unicode faces survive: for `meow:ui` writing to a sink.
    pub const fn captured() -> Ui {
        let caps = Caps {
            color: ColorLevel::None,
            unicode: true,
            width: 80,
            links: false,
            glyphs: Glyphs::UNICODE,
        };
        Ui {
            stdout: caps,
            stderr: caps,
            animate: false,
        }
    }

    pub const fn styled_for_tests() -> Ui {
        Ui {
            stdout: Caps::truecolor_for_tests(),
            stderr: Caps::truecolor_for_tests(),
            animate: false,
        }
    }

    pub const fn stdout_caps(&self) -> Caps {
        self.stdout
    }
    pub const fn stderr_caps(&self) -> Caps {
        self.stderr
    }
    pub const fn animate(&self) -> bool {
        self.animate
    }

    pub fn success_line(&self, m: &str) -> String {
        status::sigil_line(&self.stdout, Tone::Purr, m)
    }
    pub fn error_line(&self, m: &str) -> String {
        status::sigil_line(&self.stderr, Tone::Hiss, m)
    }
    pub fn warn_line(&self, m: &str) -> String {
        status::sigil_line(&self.stderr, Tone::Warn, m)
    }
    pub fn info_line(&self, m: &str) -> String {
        status::sigil_line(&self.stdout, Tone::Info, m)
    }
    pub fn step_line(&self, m: &str) -> String {
        status::sigil_line(&self.stdout, Tone::Pounce, m)
    }
    pub fn purr_line(&self, m: &str) -> String {
        status::face_line(&self.stdout, Tone::Purr, m)
    }
    pub fn hiss_line(&self, m: &str) -> String {
        status::face_line(&self.stderr, Tone::Hiss, m)
    }
    pub fn pounce_line(&self, m: &str) -> String {
        status::face_line(&self.stdout, Tone::Pounce, m)
    }

    pub fn success(&self, m: &str) {
        let _ = writeln!(io::stdout(), "{}", self.success_line(m));
    }
    pub fn error(&self, m: &str) {
        let _ = writeln!(io::stderr(), "{}", self.error_line(m));
    }
    pub fn warn(&self, m: &str) {
        let _ = writeln!(io::stderr(), "{}", self.warn_line(m));
    }
    pub fn info(&self, m: &str) {
        let _ = writeln!(io::stdout(), "{}", self.info_line(m));
    }
    pub fn step(&self, m: &str) {
        let _ = writeln!(io::stdout(), "{}", self.step_line(m));
    }
    pub fn note(&self, m: &str) {
        let _ = writeln!(io::stdout(), "  {}", self.stdout.dim(m));
    }

    pub fn purr(&self, m: &str) {
        let _ = writeln!(io::stdout(), "{}", self.purr_line(m));
    }
    pub fn hiss(&self, m: &str) {
        let _ = writeln!(io::stderr(), "{}", self.hiss_line(m));
    }
    pub fn pounce(&self, m: &str) {
        let _ = writeln!(io::stdout(), "{}", self.pounce_line(m));
    }

    pub fn out(&self, s: &str) {
        let _ = writeln!(io::stdout(), "{s}");
    }
    pub fn eout(&self, s: &str) {
        let _ = writeln!(io::stderr(), "{s}");
    }
    pub fn blank(&self) {
        let _ = writeln!(io::stdout());
    }

    pub fn spinner(&self, label: impl Into<String>) -> Spinner {
        Spinner::start(self.stderr, self.animate, label)
    }
    pub fn progress(&self, total: u64, label: impl Into<String>) -> ProgressBar {
        ProgressBar::start(self.stderr, self.animate, total, label)
    }

    pub fn diagnostic(&self, d: &SourceDiagnostic<'_>) {
        let _ = writeln!(io::stderr(), "{}", diagnostic::render(&self.stderr, d));
    }
    pub fn panel(&self, title: &str, lines: &[String]) {
        let _ = writeln!(io::stdout(), "{}", panel::panel(&self.stdout, title, lines));
    }
    pub fn table(&self, headers: &[&str], rows: &[Vec<String>], aligns: &[table::Align]) {
        let _ = writeln!(
            io::stdout(),
            "{}",
            table::table(&self.stdout, headers, rows, aligns)
        );
    }
    pub fn waterfall(&self, spans: &[waterfall::Span], track: usize) {
        let _ = writeln!(
            io::stdout(),
            "{}",
            waterfall::render(&self.stdout, spans, track)
        );
    }

    pub fn landing(&self, version: &str, groups: &[CommandGroup<'_>]) {
        let _ = write!(
            io::stdout(),
            "{}",
            banner::landing(&self.stdout, version, groups)
        );
    }
    pub fn version(&self, version: &str, extras: &[(&str, &str)]) {
        let _ = writeln!(
            io::stdout(),
            "{}",
            banner::version_block(&self.stdout, version, extras)
        );
    }
    pub fn dev_banner(&self, version: &str, mode: &str, target: &str, elapsed: Duration) {
        let _ = write!(
            io::stderr(),
            "{}",
            banner::dev_banner(&self.stderr, version, mode, target, elapsed)
        );
    }

    pub fn kv(&self, pairs: &[(String, String)]) -> Vec<String> {
        panel::kv(&self.stdout, pairs)
    }
    pub fn sigil(&self, tone: Tone, m: &str) -> String {
        status::sigil_line(&self.stdout, tone, m)
    }
    pub fn brand(&self, text: &str) -> String {
        self.stdout.brand(text)
    }
}
