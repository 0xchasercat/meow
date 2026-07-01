//! Color-level-aware ANSI emission. Pure: string in, string out. Renderers never
//! branch on capabilities; they ask `Caps` to paint and it emits truecolor, 256,
//! 16, or nothing accordingly.

use crate::palette::{self, Rgb};

/// How much color a stream can show. `None` emits no escape codes at all (pipes,
/// `NO_COLOR`, dumb terminals): fully plain, attributes included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorLevel {
    None,
    Ansi16,
    Ansi256,
    TrueColor,
}

impl ColorLevel {
    pub const fn is_on(self) -> bool {
        !matches!(self, ColorLevel::None)
    }
}

/// Text attributes. Independent of color; only emitted when color is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attr {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Attr {
    pub const NONE: Attr = Attr {
        bold: false,
        dim: false,
        italic: false,
        underline: false,
    };
    pub const fn bold() -> Attr {
        Attr {
            bold: true,
            dim: false,
            italic: false,
            underline: false,
        }
    }
    pub const fn dim() -> Attr {
        Attr {
            bold: false,
            dim: true,
            italic: false,
            underline: false,
        }
    }
    pub const fn italic() -> Attr {
        Attr {
            bold: false,
            dim: false,
            italic: true,
            underline: false,
        }
    }
}

fn attr_prefix(attr: Attr) -> String {
    let mut s = String::new();
    if attr.bold {
        s.push_str("1;");
    }
    if attr.dim {
        s.push_str("2;");
    }
    if attr.italic {
        s.push_str("3;");
    }
    if attr.underline {
        s.push_str("4;");
    }
    s
}

fn fg(level: ColorLevel, c: Rgb) -> String {
    match level {
        ColorLevel::None => String::new(),
        ColorLevel::TrueColor => format!("38;2;{};{};{}", c.r, c.g, c.b),
        ColorLevel::Ansi256 => format!("38;5;{}", palette::to_ansi256(c)),
        ColorLevel::Ansi16 => palette::to_ansi16(c).to_string(),
    }
}

/// Paint `text` with `color` + `attr` at `level`. Plain passthrough when off.
pub fn paint(level: ColorLevel, color: Rgb, attr: Attr, text: &str) -> String {
    if !level.is_on() {
        return text.to_owned();
    }
    let codes = format!("{}{}", attr_prefix(attr), fg(level, color));
    if codes.is_empty() {
        return text.to_owned();
    }
    format!("\x1b[{codes}m{text}\x1b[0m")
}

/// Paint each visible character along a multi-stop gradient (truecolor only).
/// Below truecolor it collapses to the middle stop so it still reads on-brand.
pub fn gradient(level: ColorLevel, stops: &[Rgb], attr: Attr, text: &str) -> String {
    if !level.is_on() {
        return text.to_owned();
    }
    if level != ColorLevel::TrueColor || stops.len() < 2 {
        let mid = stops[stops.len() / 2];
        return paint(level, mid, attr, text);
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    let prefix = attr_prefix(attr);
    let mut out = String::with_capacity(text.len() + n * 19);
    for (i, ch) in chars.iter().enumerate() {
        let t = if n == 1 {
            0.0
        } else {
            i as f32 / (n - 1) as f32
        };
        let c = palette::sample(stops, t);
        out.push_str(&format!("\x1b[{prefix}38;2;{};{};{}m{ch}", c.r, c.g, c.b));
    }
    out.push_str("\x1b[0m");
    out
}

/// OSC-8 hyperlink. When unsupported, returns the visible text unchanged.
pub fn hyperlink(enabled: bool, url: &str, text: &str) -> String {
    if !enabled {
        return text.to_owned();
    }
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_is_identity() {
        assert_eq!(
            paint(ColorLevel::None, Rgb::FLOSS, Attr::bold(), "hi"),
            "hi"
        );
        assert_eq!(
            gradient(ColorLevel::None, &palette::BRAND_GRADIENT, Attr::NONE, "hi"),
            "hi"
        );
    }

    #[test]
    fn truecolor_wraps_and_resets() {
        assert_eq!(
            paint(ColorLevel::TrueColor, Rgb::SKY, Attr::NONE, "x"),
            "\x1b[38;2;124;199;255mx\x1b[0m"
        );
    }

    #[test]
    fn ansi256_uses_indexed_form() {
        let s = paint(ColorLevel::Ansi256, Rgb::FLOSS, Attr::NONE, "x");
        assert!(s.starts_with("\x1b[38;5;"));
        assert!(s.ends_with("x\x1b[0m"));
    }

    #[test]
    fn gradient_below_truecolor_collapses_to_one_run() {
        let s = gradient(
            ColorLevel::Ansi256,
            &palette::BRAND_GRADIENT,
            Attr::NONE,
            "meow",
        );
        assert_eq!(s.matches("\x1b[").count(), 2);
    }

    #[test]
    fn gradient_truecolor_is_per_char() {
        let s = gradient(
            ColorLevel::TrueColor,
            &palette::BRAND_GRADIENT,
            Attr::NONE,
            "meow",
        );
        assert_eq!(s.matches("38;2;").count(), 4);
    }
}
