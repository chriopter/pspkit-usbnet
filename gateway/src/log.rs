//! The event log (`-v`): one line per event on stderr, with the seconds since
//! the first line.

use std::sync::OnceLock;
use std::time::Instant;

pub fn timestamp() -> String {
    static START: OnceLock<Instant> = OnceLock::new();
    format!("{:9.3}", START.get_or_init(Instant::now).elapsed().as_secs_f64())
}

#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        if $crate::ui::verbose() {
            eprintln!("{} {}", $crate::log::timestamp(), format_args!($($arg)*))
        }
    };
}
