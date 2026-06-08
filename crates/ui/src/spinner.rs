//! Walking paw spinner. It is intentionally tiny: one background thread, five frames,
//! carriage-return repaint, and automatic no-op when animation is disabled. The CLI
//! owns when to start/stop; pure renderers stay thread-free.

use std::io::{self, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::theme::{Rgb, Style};

pub const WALKING_PAW_FRAMES: [&str; 5] = [
    "\u{1F43E}    ",
    " \u{1F43E}   ",
    "  \u{1F43E}  ",
    "   \u{1F43E} ",
    "    \u{1F43E}",
];
const TICK: Duration = Duration::from_millis(80);

/// A running spinner. Dropping it stops the background thread and clears the line.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    label: Arc<Mutex<String>>,
    handle: Option<JoinHandle<()>>,
    active: bool,
}

impl Spinner {
    /// Start a spinner on stderr. When `enabled` is false this returns an inert
    /// spinner (CI/pipes get no control characters, no background thread).
    pub fn start(style: Style, enabled: bool, label: impl Into<String>) -> Spinner {
        if !enabled {
            return Spinner {
                stop: Arc::new(AtomicBool::new(true)),
                label: Arc::new(Mutex::new(String::new())),
                handle: None,
                active: false,
            };
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let label = Arc::new(Mutex::new(label.into()));
        let thread_label = Arc::clone(&label);
        let handle = thread::spawn(move || {
            let mut stderr = io::stderr().lock();
            let mut idx = 0usize;
            while !thread_stop.load(Ordering::Relaxed) {
                let frame =
                    style.paint(Rgb::SKY, WALKING_PAW_FRAMES[idx % WALKING_PAW_FRAMES.len()]);
                let current = thread_label
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|_| "working".to_owned());
                let _ = write!(stderr, "\r{frame} {current}");
                let _ = stderr.flush();
                idx = idx.wrapping_add(1);
                thread::sleep(TICK);
            }
            let _ = write!(stderr, "\r\x1b[2K");
            let _ = stderr.flush();
        });
        Spinner {
            stop,
            label,
            handle: Some(handle),
            active: true,
        }
    }

    pub fn inert() -> Spinner {
        Spinner {
            stop: Arc::new(AtomicBool::new(true)),
            label: Arc::new(Mutex::new(String::new())),
            handle: None,
            active: false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn set_label(&self, label: impl Into<String>) {
        if !self.active {
            return;
        }
        if let Ok(mut slot) = self.label.lock() {
            *slot = label.into();
        }
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if !self.active {
            return;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.active = false;
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_the_requested_walking_paw_sequence() {
        assert_eq!(
            WALKING_PAW_FRAMES,
            [
                "\u{1F43E}    ",
                " \u{1F43E}   ",
                "  \u{1F43E}  ",
                "   \u{1F43E} ",
                "    \u{1F43E}"
            ]
        );
    }

    #[test]
    fn spinner_label_can_be_updated() {
        let spinner = Spinner::start(Style::plain(), false, "one");
        spinner.set_label("two");
        assert!(!spinner.is_active());
    }

    #[test]
    fn disabled_spinner_is_inert() {
        let spinner = Spinner::start(Style::plain(), false, "work");
        assert!(!spinner.is_active());
    }
}
