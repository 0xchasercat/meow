//! Progress bar. A pure `bar()` renderer (brand-gradient fill) plus an
//! event-driven `ProgressBar` that repaints stderr in place. No background
//! thread: it repaints on each `set`, driven by real install/download events.
//! Inert when animation is off; `succeed`/`fail` still print the outcome line.

use std::io::{self, Write};
use std::time::Instant;

use crate::caps::Caps;
use crate::palette::{self, Rgb};
use crate::status::{self, Tone};
use crate::{fmt, width};

/// Render a fixed-width bar at `frac` (0..=1) across `cells` columns. The filled
/// run is painted along the brand gradient (truecolor); below that it collapses
/// to a solid brand color, and plain output is just full/empty glyphs.
pub fn bar(caps: &Caps, frac: f32, cells: usize) -> String {
    let cells = cells.max(4);
    let frac = frac.clamp(0.0, 1.0);
    let filled = (frac * cells as f32).round() as usize;
    let g = caps.glyphs;
    let mut s = String::new();
    for i in 0..cells {
        if i < filled {
            let t = i as f32 / (cells.max(2) - 1) as f32;
            s.push_str(&caps.paint(palette::sample(&palette::BRAND_GRADIENT, t), g.bar_full));
        } else {
            s.push_str(&caps.paint(Rgb::SMOKE, g.bar_empty));
        }
    }
    s
}

pub struct ProgressBar {
    caps: Caps,
    animate: bool,
    total: u64,
    current: u64,
    label: String,
    start: Instant,
    cells: usize,
}

impl ProgressBar {
    pub fn start(caps: Caps, animate: bool, total: u64, label: impl Into<String>) -> ProgressBar {
        let cells = (caps.width / 4).clamp(10, 28);
        let pb = ProgressBar {
            caps,
            animate,
            total,
            current: 0,
            label: label.into(),
            start: Instant::now(),
            cells,
        };
        pb.repaint();
        pb
    }

    pub fn set(&mut self, current: u64, label: impl Into<String>) {
        self.current = current;
        self.label = label.into();
        self.repaint();
    }

    pub fn set_total(&mut self, total: u64) {
        self.total = total;
        self.repaint();
    }

    /// Replace the trailing label without changing the counts.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
        self.repaint();
    }

    /// Set current, total, and label together (one repaint).
    pub fn update(&mut self, current: u64, total: u64, label: impl Into<String>) {
        self.current = current;
        self.total = total;
        self.label = label.into();
        self.repaint();
    }

    /// Set current and total without allocating a label string.
    /// Used when animation is off (non-tty) to avoid per-event `format!` cost.
    pub fn update_counts(&mut self, current: u64, total: u64) {
        self.current = current;
        self.total = total;
        self.repaint();
    }

    /// Whether this bar is animating (tty). Callers use this to skip
    /// label-string allocation when the bar won't repaint anyway.
    pub fn animate(&self) -> bool {
        self.animate
    }

    fn repaint(&self) {
        if !self.animate {
            return;
        }
        let frac = if self.total == 0 {
            0.0
        } else {
            self.current as f32 / self.total as f32
        };
        let bar = bar(&self.caps, frac, self.cells);
        let pct = self
            .caps
            .bold(Rgb::FLOSS, &format!("{:>3}%", (frac * 100.0) as u32));
        let counts = self.caps.dim(&format!(
            "{}/{}",
            fmt::count(self.current),
            fmt::count(self.total)
        ));
        let fixed = width::width(&bar) + width::width(&pct) + width::width(&counts) + 4;
        let room = self.caps.width.saturating_sub(fixed).max(8);
        let label = width::truncate(&self.label, room);
        let _ = write!(
            io::stderr(),
            "\r\x1b[2K{bar} {pct} {} {counts}",
            self.caps.muted(&label)
        );
        let _ = io::stderr().flush();
    }

    fn erase(&self) {
        if self.animate {
            let _ = write!(io::stderr(), "\r\x1b[2K");
            let _ = io::stderr().flush();
        }
    }

    /// Stop and leave no trace; the caller prints its own summary.
    pub fn clear(self) {
        self.erase();
    }

    /// Stop and print a success line with elapsed time.
    pub fn succeed(self, message: &str) {
        self.finish(Tone::Purr, message);
    }

    /// Stop and print an error line with elapsed time.
    pub fn fail(self, message: &str) {
        self.finish(Tone::Hiss, message);
    }

    fn finish(self, tone: Tone, message: &str) {
        self.erase();
        let suffix = self
            .caps
            .dim(&format!(" ({})", fmt::duration(self.start.elapsed())));
        let _ = writeln!(
            io::stderr(),
            "{}{}",
            status::sigil_line(&self.caps, tone, message),
            suffix
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_full_bars_have_expected_fill() {
        let caps = Caps::plain();
        assert_eq!(bar(&caps, 0.0, 10), "----------");
        assert_eq!(bar(&caps, 1.0, 10), "##########");
    }

    #[test]
    fn half_bar_is_half_full() {
        let caps = Caps::plain();
        let b = bar(&caps, 0.5, 10);
        assert_eq!(b.matches('#').count(), 5);
    }
}
