//! Palette + style decision. Colors are emitted as 24-bit (truecolor) ANSI so the
//! exact brand hexes render faithfully; when output is not a terminal (or `NO_COLOR`
//! is set) every paint collapses to plain text so pipes/CI stay clean (the brief's
//! `!atty` constraint, here via `std::io::IsTerminal`).

use std::io::IsTerminal;

/// A 24-bit color. The brand palette lives as associated constants so call sites
/// read as intent (`Rgb::CATNIP`) rather than as raw channel triples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Success / "Purr". `#98FF98`.
    pub const CATNIP: Rgb = Rgb {
        r: 0x98,
        g: 0xFF,
        b: 0x98,
    };
    /// Error / "Hiss" (also the diagnostic highlight). `#FFB7C5`.
    pub const MOCHI: Rgb = Rgb {
        r: 0xFF,
        g: 0xB7,
        b: 0xC5,
    };
    /// Action / "Pounce". `#89CFF0`.
    pub const SKY: Rgb = Rgb {
        r: 0x89,
        g: 0xCF,
        b: 0xF0,
    };
    /// Muted frame/gutter ink.
    pub const WHISKER: Rgb = Rgb {
        r: 0x8A,
        g: 0x8A,
        b: 0x8A,
    };
}

/// Whether ANSI styling is emitted. Resolved once at the binary edge and then
/// threaded through the pure renderers so they stay IO-free and unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    enabled: bool,
}

impl Style {
    /// Styling on (tests + callers that know they target a terminal).
    pub const fn ansi() -> Style {
        Style { enabled: true }
    }

    /// Styling off — every paint is identity. Pipes, CI, `NO_COLOR`.
    pub const fn plain() -> Style {
        Style { enabled: false }
    }

    pub const fn enabled(self) -> bool {
        self.enabled
    }

    /// Auto-detect against a stream: styled only when it is a TTY and `NO_COLOR`
    /// is unset. `NO_COLOR` presence (any value, including empty) disables color
    /// per the de-facto standard.
    pub fn auto(stream: &impl IsTerminal, no_color: bool) -> Style {
        Style {
            enabled: stream.is_terminal() && !no_color,
        }
    }

    /// Wrap `text` in a foreground color (reset at the end). Plain style returns
    /// `text` unchanged so the body is byte-identical to the no-color path.
    pub fn paint(self, color: Rgb, text: &str) -> String {
        if !self.enabled {
            return text.to_owned();
        }
        format!(
            "\x1b[38;2;{};{};{}m{text}\x1b[0m",
            color.r, color.g, color.b
        )
    }

    /// Bold + colored, for headers.
    pub fn paint_bold(self, color: Rgb, text: &str) -> String {
        if !self.enabled {
            return text.to_owned();
        }
        format!(
            "\x1b[1;38;2;{};{};{}m{text}\x1b[0m",
            color.r, color.g, color.b
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_style_is_identity() {
        assert_eq!(Style::plain().paint(Rgb::CATNIP, "hi"), "hi");
        assert_eq!(Style::plain().paint_bold(Rgb::MOCHI, "hi"), "hi");
    }

    #[test]
    fn ansi_style_wraps_truecolor_and_resets() {
        assert_eq!(
            Style::ansi().paint(Rgb::SKY, "x"),
            "\x1b[38;2;137;207;240mx\x1b[0m"
        );
    }

    #[test]
    fn auto_is_off_without_a_tty() {
        let file = std::fs::File::open("Cargo.toml").expect("workspace Cargo.toml");
        assert!(!Style::auto(&file, false).enabled());
    }
}
