//! Bento boxes: thin Unicode frames that hold dense, organized diagnostics. Used
//! for structured output (`meow doctor`, `meow why-dep`). The frame ink is muted so
//! the data stays the star.

use crate::theme::{Rgb, Style};

/// Visible width of `s`, counting most emoji/CJK as width 2. Good enough to keep
/// the box right edge from drifting on the strings meow renders (paws, labels).
fn display_width(s: &str) -> usize {
    let mut width = 0usize;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
            continue;
        }
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        width += char_width(ch);
    }
    width
}

fn char_width(ch: char) -> usize {
    let cp = ch as u32;
    let wide = matches!(cp,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF |
        0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF |
        0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 |
        0x1F300..=0x1FAFF | 0x1F000..=0x1F0FF | 0x2600..=0x27BF);
    if wide {
        2
    } else {
        1
    }
}

/// Render a framed box with a title row and body lines. Body lines may already be
/// colored; widths are computed ignoring ANSI so the right border stays aligned.
pub fn render(style: Style, title: &str, lines: &[String]) -> String {
    let inner = lines
        .iter()
        .map(|l| display_width(l))
        .chain(std::iter::once(display_width(title) + 2))
        .max()
        .unwrap_or(0)
        .max(8);

    let ink = |s: &str| style.paint(Rgb::WHISKER, s);
    let mut out = String::new();

    // Top: ╭─ title ─...─╮
    let title_painted = style.paint_bold(Rgb::SKY, title);
    let title_cells = display_width(title);
    let dashes = inner.saturating_sub(title_cells + 1);
    out.push_str(&ink("\u{256D}\u{2500} "));
    out.push_str(&title_painted);
    out.push(' ');
    out.push_str(&ink(&"\u{2500}".repeat(dashes)));
    out.push_str(&ink("\u{256E}"));
    out.push('\n');

    for line in lines {
        let pad = inner.saturating_sub(display_width(line));
        out.push_str(&ink("\u{2502} "));
        out.push_str(line);
        out.push_str(&" ".repeat(pad));
        out.push_str(&ink(" \u{2502}"));
        out.push('\n');
    }

    // Bottom: ╰─...─╯
    out.push_str(&ink("\u{2570}"));
    out.push_str(&ink(&"\u{2500}".repeat(inner + 2)));
    out.push_str(&ink("\u{256F}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_align_to_the_widest_row() {
        let out = render(
            Style::plain(),
            "doctor",
            &["lockfile: ok".to_owned(), "cache: 33 packages".to_owned()],
        );
        let lines: Vec<&str> = out.lines().collect();
        let widths: Vec<usize> = lines.iter().map(|l| display_width(l)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all rows same width: {widths:?}"
        );
        assert!(lines[0].starts_with('\u{256D}'));
        assert!(lines.last().unwrap().starts_with('\u{2570}'));
    }

    #[test]
    fn ansi_in_a_row_does_not_break_alignment() {
        let colored = Style::ansi().paint(Rgb::CATNIP, "ok");
        let out = render(
            Style::plain(),
            "t",
            &[colored, "longer line here".to_owned()],
        );
        let widths: Vec<usize> = out.lines().map(display_width).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }
}
