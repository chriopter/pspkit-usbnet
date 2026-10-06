//! The event log: one line per event, with the seconds since the first one.
//! Shown on stderr with `-v`, and while the screen of steps (`ui`) is
//! switched to it; the last lines are kept so that it has a past then.
//!
//! With several PSPs a line says whose it is, after the time: each PSP's
//! gateway has its own threads, and a thread carries that PSP's name.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const KEPT: usize = 500;

static SHOWN: AtomicBool = AtomicBool::new(true);
static PAST: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// The name of the PSP a thread works for; empty while there is none. Its
/// USB link writes it, so it changes with the PSP on the cable.
pub type Label = Arc<Mutex<String>>;

thread_local! {
    static LABEL: RefCell<Option<Label>> = const { RefCell::new(None) };
}

/// Whose lines this thread writes from here on. None: nobody's.
pub fn set_label(label: Option<Label>) {
    LABEL.with(|l| *l.borrow_mut() = label);
}

/// This thread's label, to hand on to a thread it starts.
pub fn label() -> Option<Label> {
    LABEL.with(|l| l.borrow().clone())
}

fn compose(seconds: f64, label: &str, text: std::fmt::Arguments) -> String {
    if label.is_empty() { format!("{seconds:9.3} {text}") } else { format!("{seconds:9.3} {label} | {text}") }
}

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
    let seconds = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    let line = match label() {
        Some(label) => compose(seconds, &label.lock().unwrap(), text),
        None => compose(seconds, "", text),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_says_whose_it_is() {
        assert_eq!(compose(12.345, "PSP Go", format_args!("dhcp: lease")), "   12.345 PSP Go | dhcp: lease");
        assert_eq!(compose(12.345, "", format_args!("usb: waiting")), "   12.345 usb: waiting");
    }

    /// What the last kept line with `mark` in it says after the time.
    fn kept(mark: &str) -> String {
        let past = PAST.lock().unwrap();
        let line = past.iter().rev().find(|l| l.contains(mark)).expect("the line is kept");
        line[10..].to_string()
    }

    #[test]
    fn the_label_belongs_to_the_thread() {
        let name = Label::default();
        set_label(Some(name.clone()));
        crate::logln!("mark-a: no PSP yet");
        assert_eq!(kept("mark-a"), "mark-a: no PSP yet");
        *name.lock().unwrap() = "PSP-1000 (2)".into();
        crate::logln!("mark-b: found");
        assert_eq!(kept("mark-b"), "PSP-1000 (2) | mark-b: found");
        // A thread started here has no label unless it is handed on.
        let handed = label();
        std::thread::spawn(move || {
            crate::logln!("mark-c: alone");
            set_label(handed);
            crate::logln!("mark-d: handed on");
        })
        .join()
        .unwrap();
        assert_eq!(kept("mark-c"), "mark-c: alone");
        assert_eq!(kept("mark-d"), "PSP-1000 (2) | mark-d: handed on");
        set_label(None);
        crate::logln!("mark-e: nobody's");
        assert_eq!(kept("mark-e"), "mark-e: nobody's");
    }
}
