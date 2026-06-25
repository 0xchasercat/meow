//! Braille spinner on stderr. Tiny: one background thread, brand-pink frames, a
//! min-show delay so fast work never flashes, and a final status line (tick or
//! cross + elapsed). Inert (no thread, no control bytes) whenever animation is
//! disabled, so callers can always start one regardless of environment.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::caps::Caps;
use crate::fmt;
use crate::palette::Rgb;
use crate::status::{self, Tone};

const TICK: Duration = Duration::from_millis(80);
const MIN_SHOW: Duration = Duration::from_millis(120);

pub struct Spinner {
    caps: Caps,
    stop: Arc<AtomicBool>,
    label: Arc<Mutex<String>>,
    handle: Option<JoinHandle<()>>,
    start: Instant,
}

impl Spinner {
    /// Start a spinner. When `animate` is false this is inert: no background
    /// thread and no control characters, but `succeed`/`fail` still print the
    /// final result line so CI logs keep the outcome.
    pub fn start(caps: Caps, animate: bool, label: impl Into<String>) -> Spinner {
        let label = Arc::new(Mutex::new(label.into()));
        let stop = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        if !animate {
            return Spinner {
                caps,
                stop,
                label,
                handle: None,
                start,
            };
        }
        let thread_stop = Arc::clone(&stop);
        let thread_label = Arc::clone(&label);
        let frames = caps.glyphs.spinner;
        let painter = caps;
        let handle = thread::spawn(move || {
            let spun_up = Instant::now();
            while spun_up.elapsed() < MIN_SHOW {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(15));
            }
            let mut stderr = io::stderr().lock();
            let mut idx = 0usize;
            while !thread_stop.load(Ordering::Relaxed) {
                let frame = painter.bold(Rgb::FLOSS, frames[idx % frames.len()]);
                let current = thread_label.lock().map(|s| s.clone()).unwrap_or_default();
                let _ = write!(stderr, "\r\x1b[2K{frame} {current}");
                let _ = stderr.flush();
                idx = idx.wrapping_add(1);
                thread::sleep(TICK);
            }
            let _ = write!(stderr, "\r\x1b[2K");
            let _ = stderr.flush();
        });
        Spinner {
            caps,
            stop,
            label,
            handle: Some(handle),
            start,
        }
    }

    pub fn set_label(&self, label: impl Into<String>) {
        if let Ok(mut slot) = self.label.lock() {
            *slot = label.into();
        }
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    /// Stop without a final line; the caller prints its own richer output.
    pub fn clear(mut self) {
        self.halt();
    }

    /// Stop and print a success line with elapsed time.
    pub fn succeed(self, message: &str) {
        self.finish(Tone::Purr, message);
    }

    /// Stop and print an error line with elapsed time.
    pub fn fail(self, message: &str) {
        self.finish(Tone::Hiss, message);
    }

    fn finish(mut self, tone: Tone, message: &str) {
        let caps = self.caps;
        let elapsed = self.start.elapsed();
        self.halt();
        let suffix = caps.dim(&format!(" ({})", fmt::duration(elapsed)));
        let _ = writeln!(
            io::stderr(),
            "{}{}",
            status::sigil_line(&caps, tone, message),
            suffix
        );
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.halt();
    }
}
