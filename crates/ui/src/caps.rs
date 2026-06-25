//! Terminal capability resolution. The host edge gathers ambient inputs (env +
//! tty state) once; everything downstream is a pure function of a `Caps` value.

use std::io::IsTerminal;

use crate::glyph::Glyphs;
use crate::paint::{self, Attr, ColorLevel};
use crate::palette::{self, Rgb};

/// Ambient inputs gathered at the host boundary (the CLI `host/` seam). Keeping
/// these as plain data makes capability resolution a pure, testable function.
#[derive(Debug, Clone, Default)]
pub struct TermEnv {
    /// `NO_COLOR` present (any value): disables color per the de-facto standard.
    pub no_color: bool,
    /// `FORCE_COLOR`: `Some(0)` forces off; `Some(1|2|3)` forces 16/256/true.
    pub force_color: Option<u8>,
    /// `CLICOLOR_FORCE` truthy: force color even when not a tty.
    pub clicolor_force: bool,
    /// `CI` present: disables animation (still allows color).
    pub ci: bool,
    pub term: Option<String>,
    pub colorterm: Option<String>,
    pub term_program: Option<String>,
    /// `COLUMNS` width hint, if the host exported it.
    pub width_hint: Option<usize>,
    /// Whether the locale advertises UTF-8 (drives unicode vs ascii glyphs).
    pub utf8: bool,
}

impl TermEnv {
    /// A conservative environment: no color, ascii, 80 cols. The plain baseline.
    pub fn plain() -> TermEnv {
        TermEnv {
            no_color: true,
            force_color: Some(0),
            clicolor_force: false,
            ci: true,
            term: None,
            colorterm: None,
            term_program: None,
            width_hint: Some(80),
            utf8: false,
        }
    }
}

/// Resolved capabilities for one output stream.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub color: ColorLevel,
    pub unicode: bool,
    pub width: usize,
    pub links: bool,
    pub glyphs: Glyphs,
}

const DEFAULT_WIDTH: usize = 80;

impl Caps {
    /// Fully plain caps: no color, ascii glyphs, 80 columns. Pipes/CI/tests.
    pub const fn plain() -> Caps {
        Caps {
            color: ColorLevel::None,
            unicode: false,
            width: DEFAULT_WIDTH,
            links: false,
            glyphs: Glyphs::ASCII,
        }
    }

    /// Styled truecolor caps for tests/snapshots (deterministic width, no tty).
    pub const fn truecolor_for_tests() -> Caps {
        Caps {
            color: ColorLevel::TrueColor,
            unicode: true,
            width: DEFAULT_WIDTH,
            links: false,
            glyphs: Glyphs::UNICODE,
        }
    }

    /// Resolve capabilities for a stream from gathered env + live tty state.
    pub fn resolve(env: &TermEnv, is_tty: bool) -> Caps {
        let color = resolve_color(env, is_tty);
        let unicode = resolve_unicode(env);
        let width = resolve_width(env, is_tty);
        let links = is_tty && color == ColorLevel::TrueColor && known_link_term(env);
        Caps {
            color,
            unicode,
            width,
            links,
            glyphs: Glyphs::select(unicode),
        }
    }

    pub const fn styled(self) -> bool {
        self.color.is_on()
    }

    pub fn paint(self, color: Rgb, text: &str) -> String {
        paint::paint(self.color, color, Attr::NONE, text)
    }
    pub fn bold(self, color: Rgb, text: &str) -> String {
        paint::paint(self.color, color, Attr::bold(), text)
    }
    pub fn dim(self, text: &str) -> String {
        paint::paint(self.color, Rgb::WHISKER, Attr::dim(), text)
    }
    pub fn muted(self, text: &str) -> String {
        paint::paint(self.color, Rgb::WHISKER, Attr::NONE, text)
    }
    pub fn brand(self, text: &str) -> String {
        paint::gradient(self.color, &palette::BRAND_GRADIENT, Attr::bold(), text)
    }
    pub fn gradient(self, stops: &[Rgb], text: &str) -> String {
        paint::gradient(self.color, stops, Attr::NONE, text)
    }
    pub fn link(self, url: &str, text: &str) -> String {
        paint::hyperlink(self.links, url, text)
    }
}

fn resolve_color(env: &TermEnv, is_tty: bool) -> ColorLevel {
    if let Some(level) = env.force_color {
        return match level {
            0 => ColorLevel::None,
            1 => ColorLevel::Ansi16,
            2 => ColorLevel::Ansi256,
            _ => ColorLevel::TrueColor,
        };
    }
    if env.no_color {
        return ColorLevel::None;
    }
    if !is_tty && !env.clicolor_force {
        return ColorLevel::None;
    }
    let term = env.term.as_deref().unwrap_or("");
    if term == "dumb" {
        return ColorLevel::None;
    }
    let colorterm = env.colorterm.as_deref().unwrap_or("");
    if colorterm.eq_ignore_ascii_case("truecolor") || colorterm.eq_ignore_ascii_case("24bit") {
        return ColorLevel::TrueColor;
    }
    if term.contains("direct") || known_truecolor_program(env) {
        return ColorLevel::TrueColor;
    }
    if term.contains("256") {
        return ColorLevel::Ansi256;
    }
    // A tty with an unknown-but-not-dumb TERM: assume 256 (the modern floor).
    ColorLevel::Ansi256
}

fn known_truecolor_program(env: &TermEnv) -> bool {
    let p = env.term_program.as_deref().unwrap_or("");
    matches!(
        p,
        "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper" | "rio" | "kitty" | "alacritty"
    )
}

fn known_link_term(env: &TermEnv) -> bool {
    let p = env.term_program.as_deref().unwrap_or("");
    matches!(
        p,
        "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "kitty" | "rio"
    )
}

fn resolve_unicode(env: &TermEnv) -> bool {
    if env.term.as_deref() == Some("dumb") {
        return false;
    }
    env.utf8
}

fn resolve_width(env: &TermEnv, is_tty: bool) -> usize {
    if is_tty {
        if let Some(w) = tty_width() {
            return w;
        }
    }
    env.width_hint.filter(|w| *w > 0).unwrap_or(DEFAULT_WIDTH)
}

#[cfg(unix)]
fn tty_width() -> Option<usize> {
    #[repr(C)]
    struct WinSize {
        rows: u16,
        cols: u16,
        xpix: u16,
        ypix: u16,
    }
    extern "C" {
        fn ioctl(fd: i32, request: u64, ...) -> i32;
    }
    #[cfg(target_os = "linux")]
    const TIOCGWINSZ: u64 = 0x5413;
    #[cfg(not(target_os = "linux"))]
    const TIOCGWINSZ: u64 = 0x4008_7468;

    for fd in [1i32, 2i32] {
        let mut ws = WinSize {
            rows: 0,
            cols: 0,
            xpix: 0,
            ypix: 0,
        };
        let rc = unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut WinSize) };
        if rc == 0 && ws.cols > 0 {
            return Some(ws.cols as usize);
        }
    }
    None
}

#[cfg(not(unix))]
fn tty_width() -> Option<usize> {
    None
}

/// Resolve caps for stdout + stderr together from the live process streams.
pub fn detect(env: &TermEnv) -> (Caps, Caps) {
    let stdout = Caps::resolve(env, std::io::stdout().is_terminal());
    let stderr = Caps::resolve(env, std::io::stderr().is_terminal());
    (stdout, stderr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> TermEnv {
        TermEnv {
            utf8: true,
            ..TermEnv::default()
        }
    }

    #[test]
    fn no_color_forces_plain_even_on_a_tty() {
        let env = TermEnv {
            no_color: true,
            ..base()
        };
        assert_eq!(Caps::resolve(&env, true).color, ColorLevel::None);
    }

    #[test]
    fn pipe_is_plain_without_clicolor_force() {
        assert_eq!(Caps::resolve(&base(), false).color, ColorLevel::None);
    }

    #[test]
    fn colorterm_truecolor_upgrades() {
        let env = TermEnv {
            colorterm: Some("truecolor".into()),
            ..base()
        };
        assert_eq!(Caps::resolve(&env, true).color, ColorLevel::TrueColor);
    }

    #[test]
    fn dumb_terminal_is_plain_ascii() {
        let env = TermEnv {
            term: Some("dumb".into()),
            ..base()
        };
        let caps = Caps::resolve(&env, true);
        assert_eq!(caps.color, ColorLevel::None);
        assert!(!caps.unicode);
    }

    #[test]
    fn force_color_overrides_no_color() {
        let env = TermEnv {
            no_color: true,
            force_color: Some(3),
            ..base()
        };
        assert_eq!(Caps::resolve(&env, false).color, ColorLevel::TrueColor);
    }

    #[test]
    fn tty_floor_is_256() {
        let env = TermEnv {
            term: Some("xterm".into()),
            ..base()
        };
        assert_eq!(Caps::resolve(&env, true).color, ColorLevel::Ansi256);
    }
}
