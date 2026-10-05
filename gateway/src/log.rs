//! The event log: one line per event on stderr, with the seconds since the
//! first line. Shown with `-v`, and while the screen of steps (`ui`) is
//! switched to it.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static SHOWN: AtomicBool = AtomicBool::new(true);

pub fn show(on: bool) {
    SHOWN.store(on, Ordering::Relaxed);
}

pub fn shown() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

pub fn timestamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    format!("{:9.3}", START.get_or_init(Instant::now).elapsed().as_secs_f64())
}

#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        if $crate::log::shown() {
            eprintln!("{} {}", $crate::log::timestamp(), format_args!($($arg)*))
        }
    };
}
