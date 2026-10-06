//! What the user sees: four steps that say how far the connection is, then
//! one line of traffic. On a terminal the step in progress and the traffic
//! are rewritten in place, in colour; into a file or pipe every change is a
//! plain line of its own and traffic is left out. Enter switches between
//! this and the event log.
//!
//! That is one PSP. With several on USB each has a block instead of the last
//! two steps: its name with its address, and its traffic below. On a
//! terminal the blocks are rewritten together.

use crate::log;
use crate::status::{Event, Status};
use crate::usb::LINKS;
use std::io::{IsTerminal, Write};
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const NETWORK: usize = 0;
const USB: usize = 1;
const PSP: usize = 2;
const CONNECTION: usize = 3;
const WAITING: &str = "waiting: choose \"Hi-Speed USB\" on the PSP";
const FOUND: &str = "found, handing out an address";
/// The width a step's name is written in.
const NAME: usize = 11;

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Done,
    Failed,
    Pending,
}

/// A PSP on USB, as its block shows it.
struct Psp {
    label: String,
    ip: Option<Ipv4Addr>,
    traffic: Option<String>,
}

struct Screen {
    version: &'static str,
    /// Name, how it stands, the words behind it. None: not reached yet.
    steps: [Option<(&'static str, Mark, String)>; 4],
    traffic: Option<String>,
    /// The last line on the terminal is rewritten by the next one.
    open: bool,
    /// When the traffic line was last drawn.
    drawn: Option<Instant>,
    /// The PSPs on USB, by link. One of them is shown by the steps, more
    /// by their blocks.
    psps: [Option<Psp>; LINKS],
    /// The lines of the blocks on the terminal, rewritten by the next ones.
    blocks: usize,
}

/// The terminal's width, so that no line is longer: a wrapped line cannot be
/// rewritten in place and would stay, once for every change.
fn columns() -> usize {
    #[cfg(unix)]
    {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0 {
            // One less: a line that fills the last column wraps on some terminals.
            return usize::from(size.ws_col) - 1;
        }
    }
    200
}

/// `text` cut to `room` characters, the last one an ellipsis where it was cut.
fn fit(text: &str, room: usize) -> String {
    if text.chars().count() <= room {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(room.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

fn paint(code: &str, text: &str) -> String {
    let plain = !std::io::stdout().is_terminal() || std::env::var_os("NO_COLOR").is_some();
    if plain { text.to_string() } else { format!("\x1b[{code}m{text}\x1b[0m") }
}

fn amount(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b >> 10),
    }
}

fn traffic(down: u64, up: u64, connections: usize) -> String {
    let busy = match connections {
        0 => "idle".to_string(),
        1 => "1 connection".to_string(),
        n => format!("{n} connections"),
    };
    format!("\u{2193} {}   \u{2191} {}   {busy}", amount(down), amount(up))
}

/// A step, or a PSP among several: its mark, its name, the words behind it.
fn row(name: &str, width: usize, mark: Mark, detail: &str) -> String {
    let (sign, colour) = match mark {
        Mark::Done => ("\u{2713}", "32"),
        Mark::Failed => ("\u{2717}", "31"),
        Mark::Pending => ("\u{2022}", "33"),
    };
    // Two spaces, the mark, two spaces, the name, one space: then the words.
    let detail = fit(detail, columns().saturating_sub(6 + width.max(name.chars().count())));
    let detail = paint(if mark == Mark::Done { "2" } else { colour }, &detail);
    let name = paint("1", &format!("{name:<width$}"));
    format!("  {}  {name} {detail}", paint(colour, sign))
}

impl Screen {
    /// One line; `stays`: the next line goes below it.
    fn line(&mut self, text: &str, stays: bool) {
        if log::shown() {
            return;
        }
        let mut out = std::io::stdout().lock();
        let tty = out.is_terminal();
        if self.open && tty {
            let _ = write!(out, "\r\x1b[2K");
        }
        let _ = write!(out, "{text}");
        self.open = tty && !stays;
        if !self.open {
            let _ = writeln!(out);
        }
        let _ = out.flush();
    }

    fn step_line(&mut self, i: usize) {
        if let Some((name, mark, detail)) = self.steps[i].clone() {
            self.line(&row(name, NAME, mark, &detail), mark != Mark::Pending);
        }
    }

    /// A step stands differently; the ones after it are open again.
    fn set(&mut self, i: usize, name: &'static str, mark: Mark, detail: &str) {
        if matches!(&self.steps[i], Some((n, m, d)) if (*n, *m, d.as_str()) == (name, mark, detail)) {
            return;
        }
        self.steps[i] = Some((name, mark, detail.to_string()));
        self.steps[i + 1..].fill(None);
        self.traffic = None;
        self.drawn = None;
        self.step_line(i);
    }

    fn traffic_line(&mut self) {
        if let Some(t) = self.traffic.clone() {
            self.line(&format!("     {}", paint("36", &t)), false);
        }
    }

    fn present(&self) -> usize {
        self.psps.iter().flatten().count()
    }

    /// The names line up, also with one too long for a step's.
    fn name_width(&self) -> usize {
        self.psps.iter().flatten().map(|p| p.label.chars().count()).fold(NAME, usize::max)
    }

    /// A PSP's first line: found, or with its address.
    fn psp_row(&self, p: &Psp) -> String {
        match p.ip {
            Some(ip) => row(&p.label, self.name_width(), Mark::Done, &ip.to_string()),
            None => row(&p.label, self.name_width(), Mark::Pending, FOUND),
        }
    }

    /// The block of every PSP, in place of the ones drawn before. Only on
    /// a terminal.
    fn draw_blocks(&mut self) {
        if log::shown() {
            return;
        }
        let mut lines = Vec::new();
        for p in self.psps.iter().flatten() {
            lines.push(self.psp_row(p));
            if let Some(t) = &p.traffic {
                lines.push(format!("     {}", paint("36", t)));
            }
        }
        let mut out = std::io::stdout().lock();
        if self.blocks > 0 {
            // Up to the first of them, and everything from there away.
            let _ = write!(out, "\x1b[{}A\x1b[J", self.blocks);
        }
        for l in &lines {
            let _ = writeln!(out, "{l}");
        }
        let _ = out.flush();
        self.blocks = lines.len();
    }

    /// Everything, at the start and when coming back from the event log.
    fn draw(&mut self) {
        self.open = false;
        self.blocks = 0;
        let head = format!("pspkit-usbnet {}", self.version);
        let keys = fit("Enter: event log   Ctrl+C: quit", columns().saturating_sub(head.len() + 5));
        println!("\n  {}   {}\n", paint("1", &head), paint("2", &keys));
        for i in 0..self.steps.len() {
            self.step_line(i);
        }
        if self.present() > 1 && std::io::stdout().is_terminal() {
            self.draw_blocks();
        } else {
            self.traffic_line();
        }
    }

    fn connected(&self) -> bool {
        matches!(self.steps[CONNECTION], Some((_, Mark::Done, _)))
    }

    fn on(&mut self, link: usize, event: Event) {
        let was = self.present();
        let mut gone = None;
        match &event {
            Event::Found(label) => {
                self.psps[link] = Some(Psp { label: label.clone(), ip: None, traffic: None })
            }
            Event::Lost => gone = self.psps[link].take(),
            // While a PSP is there, the search for a further one is not shown.
            Event::Waiting | Event::Busy | Event::NoAccess | Event::Broken if was > 0 => return,
            Event::Waiting | Event::Busy | Event::NoAccess | Event::Broken => {}
            // A gateway also tells the totals of a PSP that has left.
            Event::Connected(ip) => match &mut self.psps[link] {
                Some(p) => p.ip = Some(*ip),
                None => return,
            },
            Event::Traffic { down, up, connections } => match &mut self.psps[link] {
                Some(p) if p.ip.is_some() => p.traffic = Some(traffic(*down, *up, *connections)),
                _ => return,
            },
        }
        let now = self.present();
        if was.max(now) > 1 {
            self.several(link, event, gone, was, now);
        } else {
            self.one(event);
        }
    }

    /// One PSP, or none: the steps.
    fn one(&mut self, event: Event) {
        match event {
            Event::Waiting => self.set(PSP, "PSP", Mark::Pending, WAITING),
            Event::Busy => self.set(PSP, "PSP", Mark::Pending, "found, but another program is using it"),
            Event::NoAccess => self.set(PSP, "PSP", Mark::Pending, crate::access::STEP),
            Event::Broken => self.set(PSP, "PSP", Mark::Pending, "found, but it cannot be opened: see the event log (Enter)"),
            // Not a step done yet: the PSP may leave again before it has an address.
            Event::Found(_) => self.set(PSP, "PSP", Mark::Pending, FOUND),
            Event::Lost => {
                // A traffic line stays as the total; an unfinished step goes.
                if self.open && self.traffic.is_some() && !log::shown() {
                    println!();
                    self.open = false;
                }
                self.set(PSP, "PSP", Mark::Pending, WAITING);
            }
            Event::Connected(ip) if !self.connected() => {
                self.set(PSP, "PSP", Mark::Done, "found");
                self.set(CONNECTION, "Connected", Mark::Done, &ip.to_string());
            }
            Event::Connected(_) => {}
            // At most twice a second, and only on a terminal.
            Event::Traffic { down, up, connections } => {
                let now = Instant::now();
                let soon = self.drawn.is_some_and(|t| now < t + Duration::from_millis(500));
                if !self.connected() || soon || !std::io::stdout().is_terminal() {
                    return;
                }
                self.traffic = Some(traffic(down, up, connections));
                self.drawn = Some(now);
                self.traffic_line();
            }
        }
    }

    /// More than one PSP, before or after this event: the blocks. `gone`:
    /// the PSP that this event took away.
    fn several(&mut self, link: usize, event: Event, gone: Option<Psp>, was: usize, now: usize) {
        let tty = std::io::stdout().is_terminal();
        if was == 1 || now == 1 {
            if was == 1 {
                // The second PSP: the steps of the first give way to its block.
                self.steps[PSP..].fill(None);
                self.traffic = None;
            } else if let Some(p) = self.psps.iter().flatten().next() {
                // One is left: its steps again, as far as it is.
                self.steps[PSP] = Some(match p.ip {
                    Some(_) => ("PSP", Mark::Done, "found".to_string()),
                    None => ("PSP", Mark::Pending, FOUND.to_string()),
                });
                self.steps[CONNECTION] = p.ip.map(|ip| ("Connected", Mark::Done, ip.to_string()));
                self.traffic = p.traffic.clone();
            }
            self.drawn = None;
            if tty && !log::shown() {
                // Below a line still being rewritten, like everything else.
                if self.open {
                    println!();
                }
                self.draw();
                return;
            }
        }
        if !tty {
            // Into a file: a line for what changed, under the PSP's name.
            let line = match (&event, &self.psps[link], &gone) {
                (Event::Found(_) | Event::Connected(_), Some(p), _) => self.psp_row(p),
                (Event::Lost, _, Some(p)) => row(&p.label, self.name_width(), Mark::Pending, "gone"),
                _ => return,
            };
            self.line(&line, true);
            return;
        }
        // Traffic at most twice a second, as with one PSP.
        if let Event::Traffic { .. } = event {
            let at = Instant::now();
            if self.drawn.is_some_and(|t| at < t + Duration::from_millis(500)) {
                return;
            }
            self.drawn = Some(at);
        }
        self.draw_blocks();
    }
}

/// Puts the screen up, with the two steps that do not need the PSP, and
/// returns where the rest is told to. `usb`: whether USB can be used at all.
pub fn start(version: &'static str, usb: impl FnOnce() -> bool) -> Status {
    log::show(false);
    let screen = Arc::new(Mutex::new(Screen {
        version,
        steps: [const { None }; 4],
        traffic: None,
        open: false,
        drawn: None,
        psps: [const { None }; LINKS],
        blocks: 0,
    }));
    let listener = screen.clone();
    let status = Status::to(move |link, event| listener.lock().unwrap().on(link, event));
    {
        let mut s = screen.lock().unwrap();
        s.draw();
        // No packet is sent: this only asks which address would be used.
        let lan = UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| s.connect("1.1.1.1:53").and_then(|_| s.local_addr()));
        match lan {
            Ok(a) => s.set(NETWORK, "Network", Mark::Done, &format!("ok  {}", a.ip())),
            Err(_) => s.set(NETWORK, "Network", Mark::Failed, "offline: the PSP will only reach this computer"),
        }
    }
    let usb_ok = usb();
    {
        let mut s = screen.lock().unwrap();
        if usb_ok {
            s.set(USB, "USB", Mark::Done, "ok");
        } else {
            s.set(USB, "USB", Mark::Failed, "cannot be used");
        }
        s.on(0, Event::Waiting);
    }
    if std::io::stdin().is_terminal() {
        std::thread::spawn(move || keys(&screen));
    }
    status
}

/// Enter switches between the steps and the event log.
fn keys(screen: &Mutex<Screen>) {
    let mut line = String::new();
    while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
        let mut s = screen.lock().unwrap();
        if log::shown() {
            log::show(false);
            s.draw();
        } else {
            println!("\n  {}   {}\n", paint("1", "Event log"), paint("2", "Enter: back   Ctrl+C: quit"));
            log::show(true);
        }
        line.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen that draws nothing: the event log is shown, as in a test.
    fn screen() -> Screen {
        assert!(log::shown());
        Screen {
            version: "test",
            steps: [const { None }; 4],
            traffic: None,
            open: false,
                drawn: None,
            psps: [const { None }; LINKS],
            blocks: 0,
        }
    }

    fn step(s: &Screen, i: usize) -> Option<(&'static str, Mark, &str)> {
        s.steps[i].as_ref().map(|(n, m, d)| (*n, *m, d.as_str()))
    }

    impl std::fmt::Debug for Mark {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(match self {
                Mark::Done => "done",
                Mark::Failed => "failed",
                Mark::Pending => "pending",
            })
        }
    }

    const IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 2);

    #[test]
    fn one_psp_is_the_steps_whichever_link_has_it() {
        let mut s = screen();
        s.on(0, Event::Waiting);
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Pending, WAITING)));
        s.on(2, Event::Found("PSP Go".into()));
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Pending, FOUND)));
        // The other links go on looking: that does not show.
        s.on(0, Event::Waiting);
        s.on(1, Event::Busy);
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Pending, FOUND)));
        s.on(2, Event::Connected(IP));
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Done, "found")));
        assert_eq!(step(&s, CONNECTION), Some(("Connected", Mark::Done, "10.77.0.2")));
        // Nor do the totals of a link whose PSP has left.
        s.on(1, Event::Traffic { down: 1 << 30, up: 0, connections: 9 });
        assert!(s.traffic.is_none() && s.psps[1].is_none());
        s.on(2, Event::Lost);
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Pending, WAITING)));
        assert_eq!(step(&s, CONNECTION), None);
        assert_eq!(s.present(), 0);
    }

    #[test]
    fn several_psps_are_blocks_and_one_left_is_the_steps_again() {
        let mut s = screen();
        s.on(0, Event::Found("PSP Go".into()));
        s.on(0, Event::Connected(IP));
        s.on(0, Event::Traffic { down: 3 << 20, up: 1 << 10, connections: 1 });
        s.on(1, Event::Found("PSP-3000 (2)".into()));
        // The steps end with USB; the PSPs have their blocks.
        assert_eq!((step(&s, PSP), step(&s, CONNECTION)), (None, None));
        assert_eq!(s.present(), 2);
        assert_eq!(s.name_width(), 12);
        let first = s.psps[0].as_ref().unwrap();
        assert_eq!(first.ip, Some(IP));
        assert_eq!(first.traffic.as_deref(), Some("\u{2193} 3.0 MB   \u{2191} 1 KB   1 connection"));
        assert!(s.psp_row(first).contains("PSP Go") && s.psp_row(first).contains("10.77.0.2"));
        let second = s.psps[1].as_ref().unwrap();
        assert!(s.psp_row(second).contains("PSP-3000 (2)") && s.psp_row(second).contains(FOUND));
        s.on(1, Event::Connected(IP));
        assert_eq!(step(&s, PSP), None);

        // The first leaves: the second is the one PSP, as far as it is.
        s.on(0, Event::Lost);
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Done, "found")));
        assert_eq!(step(&s, CONNECTION), Some(("Connected", Mark::Done, "10.77.0.2")));
        assert!(s.traffic.is_none());
        s.on(1, Event::Lost);
        assert_eq!(step(&s, PSP), Some(("PSP", Mark::Pending, WAITING)));
    }
}
