//! Boxes, key/value rows, and rules. Frames use muted ink so the data stays the
//! star; widths are measured ANSI-aware so colored body lines never skew the
//! right edge. Everything caps to the terminal width.

use crate::caps::Caps;
use crate::palette::Rgb;
use crate::width;

/// A horizontal rule, `cols` wide (clamped to the terminal).
pub fn rule(caps: &Caps, cols: usize) -> String {
    let cols = cols.min(caps.width).max(4);
    caps.muted(&caps.glyphs.horizontal.repeat(cols))
}

/// A rounded, titled panel around pre-rendered (possibly colored) body lines.
pub fn panel(caps: &Caps, title: &str, lines: &[String]) -> String {
    let g = caps.glyphs;
    let max_body = lines.iter().map(|l| width::width(l)).max().unwrap_or(0);
    let title_w = width::width(title);
    let inner = max_body
        .max(title_w + 2)
        .min(caps.width.saturating_sub(4))
        .max(8);
    let ink = |s: &str| caps.muted(s);

    let mut out = String::new();
    let dashes = inner.saturating_sub(title_w + 1);
    out.push_str(&ink(g.top_left));
    out.push_str(&ink(g.horizontal));
    out.push(' ');
    out.push_str(&caps.bold(Rgb::FLOSS, title));
    out.push(' ');
    out.push_str(&ink(&g.horizontal.repeat(dashes)));
    out.push_str(&ink(g.top_right));
    out.push('\n');

    for line in lines {
        let pad = inner.saturating_sub(width::width(line));
        out.push_str(&ink(g.vertical));
        out.push(' ');
        out.push_str(line);
        out.push_str(&" ".repeat(pad));
        out.push(' ');
        out.push_str(&ink(g.vertical));
        out.push('\n');
    }

    out.push_str(&ink(g.bottom_left));
    out.push_str(&ink(&g.horizontal.repeat(inner + 2)));
    out.push_str(&ink(g.bottom_right));
    out
}

/// Align key/value pairs into `key   value` lines (keys muted, values as given).
pub fn kv(caps: &Caps, pairs: &[(String, String)]) -> Vec<String> {
    let kw = pairs
        .iter()
        .map(|(k, _)| width::width(k))
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(k, v)| format!("{}  {v}", caps.muted(&width::pad_end(k, kw))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_rows_share_one_width() {
        let out = panel(
            &Caps::plain(),
            "doctor",
            &["lockfile: ok".to_owned(), "cache: 33 packages".to_owned()],
        );
        let widths: Vec<usize> = out.lines().map(width::width).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }

    #[test]
    fn colored_body_does_not_skew_alignment() {
        let caps = Caps::truecolor_for_tests();
        let colored = caps.paint(Rgb::CATNIP, "ok");
        let out = panel(&Caps::plain(), "t", &[colored, "longer line".to_owned()]);
        let widths: Vec<usize> = out.lines().map(width::width).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }
}
