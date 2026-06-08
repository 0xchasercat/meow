//! Source-pointing diagnostics — the "Honest Kitten". A `miette`-style snippet
//! (gutter + the offending line + a caret underline) with the highlight in Mochi
//! Pink. No external diagnostic crate: the renderer is a pure function over the
//! source text + a byte span, which keeps the binary lean and the output testable.

use crate::envelope::{self, Tone};
use crate::theme::{Rgb, Style};

/// A code error to point at: the source, a byte range within it, and the human
/// message. `label` annotates the underline; `path` names the file in the gutter.
#[derive(Debug, Clone)]
pub struct SourceDiagnostic<'a> {
    pub path: &'a str,
    pub source: &'a str,
    /// Byte range `[start, end)` into `source`. Clamped defensively.
    pub span: (usize, usize),
    pub message: &'a str,
    pub label: Option<&'a str>,
}

struct Located {
    line_no: usize,
    col: usize,
    line_text: String,
    span_len: usize,
}

/// Resolve a byte offset to 1-based line/column and pull the line text. Never
/// panics on an out-of-range span — it clamps to the source end.
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
    // Underline length in columns, kept on the offending line.
    let span_on_line = source[start..end.min(line_end)].chars().count().max(1);
    Located {
        line_no,
        col,
        line_text,
        span_len: span_on_line,
    }
}

/// Render the full diagnostic block (header envelope + framed snippet).
pub fn render(style: Style, diag: &SourceDiagnostic<'_>) -> String {
    let loc = locate(diag.source, diag.span);
    let header = envelope::render(style, Tone::Hiss, diag.message);
    let gutter_w = loc.line_no.to_string().len();
    let pad = " ".repeat(gutter_w);
    let ink = |s: &str| style.paint(Rgb::WHISKER, s);

    let mut out = String::new();
    out.push_str(&header);
    out.push('\n');
    out.push_str(&format!(
        "{pad}{}",
        ink(&format!(
            "\u{256D}\u{2500}[{}:{}:{}]",
            diag.path, loc.line_no, loc.col
        ))
    ));
    out.push('\n');
    out.push_str(&format!(
        "{} {} {}",
        style.paint(Rgb::WHISKER, &loc.line_no.to_string()),
        ink("\u{2502}"),
        loc.line_text
    ));
    out.push('\n');

    let caret = style.paint_bold(Rgb::MOCHI, &"^".repeat(loc.span_len));
    let lead = " ".repeat(loc.col.saturating_sub(1));
    out.push_str(&format!("{pad} {} {lead}{caret}", ink("\u{00B7}")));
    if let Some(label) = diag.label {
        out.push(' ');
        out.push_str(&style.paint(Rgb::MOCHI, label));
    }
    out.push('\n');
    out.push_str(&format!(
        "{pad}{}",
        ink("\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}")
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag<'a>(source: &'a str, span: (usize, usize)) -> SourceDiagnostic<'a> {
        SourceDiagnostic {
            path: "app.ts",
            source,
            span,
            message: "enum is not erasable",
            label: Some("here"),
        }
    }

    #[test]
    fn locates_line_and_column_on_the_second_line() {
        let src = "const a = 1;\nenum E { A }\n";
        let start = src.find("enum").unwrap();
        let loc = locate(src, (start, start + 4));
        assert_eq!(loc.line_no, 2);
        assert_eq!(loc.col, 1);
        assert_eq!(loc.line_text, "enum E { A }");
        assert_eq!(loc.span_len, 4);
    }

    #[test]
    fn out_of_range_span_does_not_panic() {
        let src = "x";
        let loc = locate(src, (999, 1000));
        assert_eq!(loc.line_no, 1);
        assert_eq!(loc.span_len, 1);
    }

    #[test]
    fn plain_render_shows_line_message_and_caret_under_the_span() {
        let src = "const a = 1;\nenum E { A }\n";
        let start = src.find("enum").unwrap();
        let out = render(Style::plain(), &diag(src, (start, start + 4)));
        assert!(out.contains("[app.ts:2:1]"));
        assert!(out.contains("enum E { A }"));
        assert!(out.contains("^^^^"));
        assert!(out.contains("enum is not erasable"));
        assert!(out.contains("here"));
    }

    #[test]
    fn caret_is_offset_to_the_column() {
        let src = "let x = bad;";
        let start = src.find("bad").unwrap();
        let out = render(Style::plain(), &diag(src, (start, start + 3)));
        let caret_line = out.lines().find(|l| l.contains("^^^")).unwrap();
        // 3 carets, indented past the gutter to column 9.
        assert!(caret_line.contains("        ^^^"));
    }
}
