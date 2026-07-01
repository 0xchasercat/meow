//! Column-aligned table. Auto-sizes columns to their content (ANSI-aware),
//! caps to the terminal width, truncates overflow with an ellipsis, and prints a
//! dim rule under a muted-bold header. Used by `why-dep`, `doctor`, and lists.

use crate::caps::Caps;
use crate::palette::Rgb;
use crate::width;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Render `rows` under `headers`. `aligns` is optional per column (default left).
pub fn table(caps: &Caps, headers: &[&str], rows: &[Vec<String>], aligns: &[Align]) -> String {
    let cols = headers.len();
    if cols == 0 {
        return String::new();
    }
    let mut w = vec![0usize; cols];
    for (i, h) in headers.iter().enumerate() {
        w[i] = width::width(h);
    }
    for row in rows {
        for (i, c) in row.iter().enumerate().take(cols) {
            w[i] = w[i].max(width::width(c));
        }
    }
    let gutter = 2usize;
    let cap = caps.width.saturating_sub(gutter * cols.saturating_sub(1)) / cols.max(1);
    for x in w.iter_mut() {
        *x = (*x).min(cap.max(6));
    }
    let align = |i: usize| aligns.get(i).copied().unwrap_or(Align::Left);
    let cell = |i: usize, s: &str| -> String {
        let t = width::truncate(s, w[i]);
        match align(i) {
            Align::Right => width::pad_start(&t, w[i]),
            Align::Left => width::pad_end(&t, w[i]),
        }
    };

    let mut out = String::new();
    let header: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| caps.bold(Rgb::WHISKER, &cell(i, h)))
        .collect();
    out.push_str(&header.join("  "));
    out.push('\n');
    let total: usize = w.iter().sum::<usize>() + gutter * cols.saturating_sub(1);
    out.push_str(&caps.dim(&caps.glyphs.horizontal.repeat(total)));
    for row in rows {
        out.push('\n');
        let cells: Vec<String> = (0..cols)
            .map(|i| cell(i, row.get(i).map(String::as_str).unwrap_or("")))
            .collect();
        out.push_str(&cells.join("  "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_rows_present() {
        let out = table(
            &Caps::plain(),
            &["pkg", "version"],
            &[vec!["left-pad".into(), "1.3.0".into()]],
            &[Align::Left, Align::Right],
        );
        assert!(out.contains("pkg"));
        assert!(out.contains("left-pad"));
        assert!(out.contains("1.3.0"));
        assert_eq!(out.lines().count(), 3);
    }
}
