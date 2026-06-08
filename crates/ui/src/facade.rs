//! Thin IO facade over the pure renderers. The facade centralizes style detection
//! and stream choice; it deliberately does not capture user-program stdout/stderr.

use std::io::{self, IsTerminal, Write};

use crate::envelope::{self, Tone};
use crate::spinner::Spinner;
use crate::theme::Style;

#[derive(Debug, Clone, Copy)]
pub struct Ui {
    stdout_style: Style,
    stderr_style: Style,
    animate: bool,
}

impl Ui {
    /// Detect style/animation from stdout/stderr. Animations require both color
    /// permission and a terminal stderr; stdout colors require terminal stdout.
    pub fn auto_with_no_color(no_color: bool) -> Ui {
        let stdout = io::stdout();
        let stderr = io::stderr();
        let stdout_style = Style::auto(&stdout, no_color);
        let stderr_style = Style::auto(&stderr, no_color);
        Ui {
            stdout_style,
            stderr_style,
            animate: stderr.is_terminal() && !no_color,
        }
    }

    /// Conservative default for library callers: no ambient env reads. Binaries
    /// that can read the host environment should call `auto_with_no_color(...)`
    /// with their already-resolved no-color flag.
    pub fn auto() -> Ui {
        Ui::auto_with_no_color(false)
    }

    pub const fn plain() -> Ui {
        Ui {
            stdout_style: Style::plain(),
            stderr_style: Style::plain(),
            animate: false,
        }
    }

    pub const fn styled_for_tests() -> Ui {
        Ui {
            stdout_style: Style::ansi(),
            stderr_style: Style::ansi(),
            animate: false,
        }
    }

    pub const fn stdout_style(self) -> Style {
        self.stdout_style
    }

    pub const fn stderr_style(self) -> Style {
        self.stderr_style
    }

    pub const fn animations_enabled(self) -> bool {
        self.animate
    }

    pub fn purr_line(self, body: &str) -> String {
        envelope::render(self.stdout_style, Tone::Purr, body)
    }

    pub fn hiss_line(self, body: &str) -> String {
        envelope::render(self.stderr_style, Tone::Hiss, body)
    }

    pub fn pounce_line(self, body: &str) -> String {
        envelope::render(self.stdout_style, Tone::Pounce, body)
    }

    pub fn purr(self, body: &str) {
        let _ = writeln!(io::stdout(), "{}", self.purr_line(body));
    }

    pub fn hiss(self, body: &str) {
        let _ = writeln!(io::stderr(), "{}", self.hiss_line(body));
    }

    pub fn pounce(self, body: &str) {
        let _ = writeln!(io::stdout(), "{}", self.pounce_line(body));
    }

    pub fn spinner(self, label: impl Into<String>) -> Spinner {
        Spinner::start(self.stderr_style, self.animate, label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_facade_renders_expected_envelopes() {
        let ui = Ui::plain();
        assert_eq!(ui.purr_line("ok"), "\u{1F638} [Purrfect!] ok");
        assert_eq!(ui.hiss_line("bad"), "\u{1F640} [Bad Kitty!] bad");
        assert_eq!(ui.pounce_line("go"), "\u{1F43E} [Pouncing...] go");
    }

    #[test]
    fn plain_facade_disables_animation() {
        assert!(!Ui::plain().animations_enabled());
    }
}
