//! Source-pointing diagnostics, compiler-grade. A header line, a framed snippet
//! with a line gutter and a caret underline in error red, and optional `help:` /
//! `note:` lines. Pure over the source text + a byte span, so it needs no
//! external diagnostic crate and stays trivially testable.

use crate::caps::Caps;
use crate::palette::Rgb;
use crate::status::{self, Tone};

/// A code error to point at. `label` annotates the caret; `help`/`note` add the
/// actionable trailer compilers train users to look for.
#[derive(Debug, Clone)]
pub struct SourceDiagnostic<'a> {
    pub path: &'a str,
    pub source: &'a str,
    pub span: (usize, usize),
    pub message: &'a str,
    pub label: Option<&'a str>,
    pub help: Option<&'a str>,
    pub note: Option<&'a str>,
}

impl<'a> SourceDiagnostic<'a> {
    /// Minimal constructor; help/note default to none.
    pub fn new(path: &'a str, source: &'a str, span: (usize, usize), message: &'a str) -> Self {
        SourceDiagnostic {
            path,
            source,
            span,
            message,
            label: None,
            help: None,
            note: None,
        }
    }
}

struct Located {
    line_no: usize,
    col: usize,
    line_text: String,
    span_len: usize,
}

fn locate(source: &str, span: (usize, usize)) -> Located {
    let start = span.0.min(source.len());
    let end = span.1.clamp(start, source.len());
    let mut line_start = 0usize;
    let mut line_no = 1usize;
    for (idx, ch) in source.char_indices() {
        if idx >= start {
            break;
        }
        if ch == '\n' {
            line_no += 1;
            line_start = idx + 1;
        }
    }
    let line_end = source[line_start..]
        .find('\n')
        .map(|rel| line_start + rel)
        .unwrap_or(source.len());
    let line_text = source[line_start..line_end].to_owned();
    let col = source[line_start..start].chars().count() + 1;
    let span_on_line = source[start..end.min(line_end)].chars().count().max(1);
    Located {
        line_no,
        col,
        line_text,
        span_len: span_on_line,
    }
}

/// Render the full diagnostic block.
pub fn render(caps: &Caps, diag: &SourceDiagnostic<'_>) -> String {
    let g = caps.glyphs;
    let loc = locate(diag.source, diag.span);
    let ink = |s: &str| caps.muted(s);
    let gutter = loc.line_no.to_string();
    let pad = " ".repeat(gutter.len());

    let mut out = String::new();
    out.push_str(&status::sigil_line(caps, Tone::Hiss, diag.message));
    out.push('\n');
    out.push_str(&format!(
        "{pad}{}",
        ink(&format!(
            "{}{}[{}:{}:{}]",
            g.gutter_down, g.horizontal, diag.path, loc.line_no, loc.col
        ))
    ));
    out.push('\n');
    out.push_str(&format!(
        "{} {} {}",
        caps.bold(Rgb::WHISKER, &gutter),
        ink(g.vertical),
        loc.line_text
    ));
    out.push('\n');

    let lead = " ".repeat(loc.col.saturating_sub(1));
    let carets = caps.bold(Rgb::HISS, &g.caret.repeat(loc.span_len));
    out.push_str(&format!("{pad} {} {lead}{carets}", ink(g.bullet)));
    if let Some(label) = diag.label {
        out.push(' ');
        out.push_str(&caps.paint(Rgb::HISS, label));
    }
    out.push('\n');

    let mut tail: Vec<(Rgb, &str, &str)> = Vec::new();
    if let Some(help) = diag.help {
        tail.push((Rgb::SKY, "help:", help));
    }
    if let Some(note) = diag.note {
        tail.push((Rgb::WHISKER, "note:", note));
    }
    if tail.is_empty() {
        out.push_str(&format!(
            "{pad}{}",
            ink(&format!("{}{}", g.gutter_up, g.horizontal.repeat(4)))
        ));
    } else {
        let last = tail.len() - 1;
        for (i, (color, kw, text)) in tail.iter().enumerate() {
            let conn = if i == last { g.gutter_up } else { g.tee_right };
            out.push_str(&format!(
                "{pad}{} {} {text}",
                ink(&format!("{}{}", conn, g.horizontal)),
                caps.bold(*color, kw)
            ));
            if i != last {
                out.push('\n');
            }
        }
    }
    out
}

/// Levenshtein distance (small inputs only: command/flag names).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// The closest candidate to `input`, when one is near enough to suggest.
pub fn did_you_mean<'a>(input: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let mut best: Option<&str> = None;
    let mut best_d = usize::MAX;
    for c in candidates {
        let d = levenshtein(input, c);
        if d < best_d {
            best_d = d;
            best = Some(c);
        }
    }
    let threshold = (input.chars().count() / 2).max(2);
    best.filter(|_| best_d <= threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_at_the_span_with_help() {
        let src = "const a = 1;\nenum E { A }\n";
        let start = src.find("enum").unwrap();
        let mut d =
            SourceDiagnostic::new("app.ts", src, (start, start + 4), "enum is not erasable");
        d.label = Some("here");
        d.help = Some("use a const object instead");
        let out = render(&Caps::plain(), &d);
        assert!(out.contains("[app.ts:2:1]"));
        assert!(out.contains("enum E { A }"));
        assert!(out.contains("^^^^"));
        assert!(out.contains("help:"));
    }

    #[test]
    fn suggests_close_commands() {
        let cmds = ["install", "run", "test", "lint"];
        assert_eq!(did_you_mean("intall", &cmds), Some("install"));
        assert_eq!(did_you_mean("xyzzy", &cmds), None);
    }
}
