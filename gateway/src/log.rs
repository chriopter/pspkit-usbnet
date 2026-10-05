//! The event log: one line per event, with the seconds since the first one.
//! Shown on stderr with `-v`, and while the screen of steps (`ui`) is
//! switched to it; the last lines are kept so that it has a past then.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const KEPT: usize = 500;

static SHOWN: AtomicBool = AtomicBool::new(true);
static PAST: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Show the log from here on, beginning with what was kept, or stop.
pub fn show(on: bool) {
    let past = PAST.lock().unwrap();
    if on && !SHOWN.load(Ordering::Relaxed) {
        for line in past.iter() {
            eprintln!("{line}");
        }
    }
    SHOWN.store(on, Ordering::Relaxed);
}

pub fn shown() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

pub fn line(text: std::fmt::Arguments) {
    static START: OnceLock<Instant> = OnceLock::new();
    let line = format!("{:9.3} {text}", START.get_or_init(Instant::now).elapsed().as_secs_f64());
    let mut past = PAST.lock().unwrap();
    if shown() {
        eprintln!("{line}");
    }
    if past.len() == KEPT {
        past.pop_front();
    }
    past.push_back(line);
}

#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        $crate::log::line(format_args!($($arg)*))
    };
}
