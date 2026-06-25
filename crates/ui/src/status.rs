//! Status lines. Two registers share one tone model:
//! - the CLI's own chrome uses clean colored sigils (low-noise, enterprise-safe);
//! - the opt-in `meow:ui` JS API uses cat faces (overt personality).
//!
//! Both keep the message body plain so piped, greppable output is byte-stable.

use crate::caps::Caps;
use crate::palette::Rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Purr,
    Hiss,
    Pounce,
    Warn,
    Info,
}

impl Tone {
    pub fn color(self) -> Rgb {
        match self {
            Tone::Purr => Rgb::CATNIP,
            Tone::Hiss => Rgb::HISS,
            Tone::Pounce => Rgb::FLOSS,
            Tone::Warn => Rgb::HONEY,
            Tone::Info => Rgb::SKY,
        }
    }

    fn sigil(self, caps: &Caps) -> &'static str {
        let g = caps.glyphs;
        match self {
            Tone::Purr => g.ok,
            Tone::Hiss => g.err,
            Tone::Pounce => g.paw,
            Tone::Warn => g.warn,
            Tone::Info => g.info,
        }
    }

    fn face(self, caps: &Caps) -> &'static str {
        let g = caps.glyphs;
        match self {
            Tone::Purr => g.purr_face,
            Tone::Hiss => g.hiss_face,
            Tone::Pounce => g.pounce_face,
            Tone::Warn => g.warn,
            Tone::Info => g.info,
        }
    }
}

/// CLI chrome register: `<sigil> message` with a colored bold sigil, plain body.
pub fn sigil_line(caps: &Caps, tone: Tone, message: &str) -> String {
    let mark = caps.bold(tone.color(), tone.sigil(caps));
    if message.is_empty() {
        mark
    } else {
        format!("{mark} {message}")
    }
}

/// Cute register for `meow:ui`: `<cat-face> message`, face colored, body plain.
pub fn face_line(caps: &Caps, tone: Tone, message: &str) -> String {
    let mark = caps.paint(tone.color(), tone.face(caps));
    if message.is_empty() {
        mark
    } else {
        format!("{mark} {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_sigils_are_ascii_and_greppable() {
        let caps = Caps::plain();
        assert_eq!(sigil_line(&caps, Tone::Purr, "done"), "+ done");
        assert_eq!(sigil_line(&caps, Tone::Hiss, "boom"), "x boom");
    }

    #[test]
    fn cute_register_uses_cat_faces_when_unicode() {
        let caps = Caps::truecolor_for_tests();
        let line = face_line(&caps, Tone::Purr, "hello");
        assert!(line.contains("\u{1F638}"));
        assert!(line.ends_with(" hello"));
    }

    #[test]
    fn empty_body_renders_only_the_mark() {
        assert_eq!(sigil_line(&Caps::plain(), Tone::Info, ""), "*");
    }
}
