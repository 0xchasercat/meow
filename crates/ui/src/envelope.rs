//! The "envelope": a cute, colored header that frames a precise, plain body (the
//! "letter"). The envelope sets the vibe; the body never sacrifices technical
//! clarity. Renderers are pure (`-> String`) so they are trivially testable and so
//! the IO facade can decide stdout vs stderr.

use crate::theme::{Rgb, Style};

/// The three tones. Each owns its icon, label, and palette color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Success.
    Purr,
    /// Error.
    Hiss,
    /// In-progress action.
    Pounce,
}

impl Tone {
    pub const fn icon(self) -> &'static str {
        match self {
            Tone::Purr => "\u{1F638}",   // grinning cat
            Tone::Hiss => "\u{1F640}",   // weary cat
            Tone::Pounce => "\u{1F43E}", // paw prints
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Tone::Purr => "Purrfect!",
            Tone::Hiss => "Bad Kitty!",
            Tone::Pounce => "Pouncing...",
        }
    }

    pub const fn color(self) -> Rgb {
        match self {
            Tone::Purr => Rgb::CATNIP,
            Tone::Hiss => Rgb::MOCHI,
            Tone::Pounce => Rgb::SKY,
        }
    }
}

/// Render `<icon> [<Label>] <body>` with the header colored per tone and the body
/// left plain. The icon + bracketed label are the only decorated text, so the
/// body that tests and humans read is byte-identical with or without color.
pub fn render(style: Style, tone: Tone, body: &str) -> String {
    let header = style.paint_bold(tone.color(), &format!("{} [{}]", tone.icon(), tone.label()));
    if body.is_empty() {
        header
    } else {
        format!("{header} {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_envelope_keeps_the_body_verbatim() {
        let line = render(Style::plain(), Tone::Purr, "installed 6 packages");
        assert_eq!(line, "\u{1F638} [Purrfect!] installed 6 packages");
    }

    #[test]
    fn each_tone_has_its_own_label_and_color() {
        assert_eq!(Tone::Hiss.label(), "Bad Kitty!");
        assert_eq!(Tone::Pounce.color(), Rgb::SKY);
        assert_ne!(Tone::Purr.icon(), Tone::Hiss.icon());
    }

    #[test]
    fn colored_header_wraps_only_the_header() {
        let line = render(Style::ansi(), Tone::Hiss, "boom");
        // Body text appears outside any reset, verbatim.
        assert!(line.ends_with(" boom"));
        assert!(line.contains("\x1b[1;38;2;255;183;197m"));
    }
}
