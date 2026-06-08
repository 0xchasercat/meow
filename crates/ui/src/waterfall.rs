//! The "Waterfall of Cuteness" — the §17 module-load timeline. Y axis = module,
//! X axis = time; each bar is a run of paws (`\u{1F43E}`) offset by start time.
//! Rendering is a single O(n) pass over the spans (string building only), so a
//! 500+ dependency graph renders without quadratic blowup or terminal thrash.

use crate::theme::{Rgb, Style};

/// One module's load window, in microseconds from the run start.
#[derive(Debug, Clone)]
pub struct Span {
    pub module: String,
    pub start_us: u64,
    pub dur_us: u64,
}

/// Render a fixed-width waterfall. `track` is the number of cells on the time axis
/// (e.g. terminal width minus the labels). Bars shorter than the per-cell duration
/// still get one paw so nothing renders as invisible.
pub fn render(style: Style, spans: &[Span], track: usize) -> String {
    if spans.is_empty() {
        return style.paint(Rgb::WHISKER, "(no modules timed)");
    }
    let track = track.max(8);
    let total_end = spans
        .iter()
        .map(|s| s.start_us + s.dur_us)
        .max()
        .unwrap_or(1)
        .max(1);
    let label_w = spans
        .iter()
        .map(|s| s.module.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);

    let scale = |us: u64| -> usize {
        // Map a microsecond offset onto the cell track.
        ((us as u128 * track as u128) / total_end as u128) as usize
    };

    let mut out = String::new();
    for (i, s) in spans.iter().enumerate() {
        let off = scale(s.start_us).min(track.saturating_sub(1));
        let cells = scale(s.dur_us).max(1).min(track - off);
        // One paw is two columns wide; pack the bar with paws, halved cell count.
        let paws = cells.div_ceil(2).max(1);
        let bar = style.paint(Rgb::SKY, &"\u{1F43E}".repeat(paws));
        let lead = " ".repeat(off);
        let label = truncate(&s.module, label_w);
        let ms = format!("{:.2}ms", s.dur_us as f64 / 1000.0);
        out.push_str(&format!(
            "{:<label_w$} {}{} {}",
            label,
            lead,
            bar,
            style.paint(Rgb::WHISKER, &ms)
        ));
        if i + 1 < spans.len() {
            out.push('\n');
        }
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let keep = max.saturating_sub(1);
    let mut t: String = s.chars().take(keep).collect();
    t.push('\u{2026}');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(module: &str, start: u64, dur: u64) -> Span {
        Span {
            module: module.to_owned(),
            start_us: start,
            dur_us: dur,
        }
    }

    #[test]
    fn empty_is_honest() {
        assert_eq!(render(Style::plain(), &[], 40), "(no modules timed)");
    }

    #[test]
    fn later_modules_are_offset_further_right() {
        let spans = vec![span("a", 0, 1000), span("b", 5000, 1000)];
        let out = render(Style::plain(), &spans, 20);
        let lines: Vec<&str> = out.lines().collect();
        let a_paw = lines[0].find('\u{1F43E}').unwrap();
        let b_paw = lines[1].find('\u{1F43E}').unwrap();
        assert!(b_paw > a_paw, "b starts later: {a_paw} vs {b_paw}");
    }

    #[test]
    fn handles_five_hundred_modules_quickly() {
        let spans: Vec<Span> = (0..500)
            .map(|i| span(&format!("mod-{i}"), i * 100, 250))
            .collect();
        let out = render(Style::plain(), &spans, 60);
        assert_eq!(out.lines().count(), 500);
    }

    #[test]
    fn long_labels_are_truncated_with_ellipsis() {
        let spans = vec![span(&"x".repeat(100), 0, 10)];
        let out = render(Style::plain(), &spans, 20);
        assert!(out.contains('\u{2026}'));
    }
}
