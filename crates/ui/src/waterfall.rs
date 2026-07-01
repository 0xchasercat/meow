//! The module-load timeline (`meow why-slow`). Y axis = module, X axis = time;
//! each bar is a trail of paws offset by start time, tinted along the brand
//! gradient. One O(n) pass over the spans, so a 500+ module graph renders without
//! quadratic blowup or terminal thrash.

use crate::caps::Caps;
use crate::palette::Rgb;
use crate::width;

/// One module's load window, in microseconds from the run start.
#[derive(Debug, Clone)]
pub struct Span {
    pub module: String,
    pub start_us: u64,
    pub dur_us: u64,
}

/// Render a fixed-width waterfall. `track` is the number of time-axis cells.
pub fn render(caps: &Caps, spans: &[Span], track: usize) -> String {
    if spans.is_empty() {
        return caps.muted("(no modules timed)");
    }
    let track = track.min(caps.width.saturating_sub(24)).max(8);
    let total_end = spans
        .iter()
        .map(|s| s.start_us + s.dur_us)
        .max()
        .unwrap_or(1)
        .max(1);
    let label_w = spans
        .iter()
        .map(|s| width::width(&s.module))
        .max()
        .unwrap_or(0)
        .min(32);
    let scale = |us: u64| -> usize { ((us as u128 * track as u128) / total_end as u128) as usize };

    let mut out = String::new();
    for (i, s) in spans.iter().enumerate() {
        let off = scale(s.start_us).min(track.saturating_sub(1));
        let cells = scale(s.dur_us).max(1).min(track - off);
        let paws = cells.div_ceil(2).max(1);
        let bar = caps.gradient(&[Rgb::FLOSS, Rgb::VIOLET], &caps.glyphs.paw.repeat(paws));
        let ms = caps.muted(&format!("{:.2}ms", s.dur_us as f64 / 1000.0));
        out.push_str(&format!(
            "{} {}{} {}",
            width::pad_end(&s.module, label_w),
            " ".repeat(off),
            bar,
            ms
        ));
        if i + 1 < spans.len() {
            out.push('\n');
        }
    }
    out
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
        assert_eq!(render(&Caps::plain(), &[], 40), "(no modules timed)");
    }

    #[test]
    fn later_modules_sit_further_right() {
        let spans = vec![span("a", 0, 1000), span("b", 5000, 1000)];
        let out = render(&Caps::plain(), &spans, 20);
        let lines: Vec<&str> = out.lines().collect();
        let a = lines[0].find(":3").unwrap();
        let b = lines[1].find(":3").unwrap();
        assert!(b > a, "b starts further right: {a} vs {b}");
    }

    #[test]
    fn handles_many_modules() {
        let spans: Vec<Span> = (0..500).map(|i| span("m", i * 100, 250)).collect();
        assert_eq!(render(&Caps::plain(), &spans, 60).lines().count(), 500);
    }
}
