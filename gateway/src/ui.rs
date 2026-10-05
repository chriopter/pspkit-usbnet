//! What the user sees: four steps that say how far the connection is, then
//! one line of traffic. On a terminal the step in progress and the traffic
//! are rewritten in place, in colour; into a file or pipe every change is a
//! plain line of its own and traffic is left out. Enter switches between
//! this and the live log of events (which `-v` shows from the start).

use std::io::{IsTerminal, Write};
use std::net::UdpSocket;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const WAITING: &str = "waiting: choose \"Hi-Speed USB\" on the PSP";

const NETWORK: usize = 0;
const USB: usize = 1;
const PSP: usize = 2;
const CONNECTION: usize = 3;

/// The steps are on the screen (otherwise: the live log).
static STEPS: AtomicBool = AtomicBool::new(false);
static SCREEN: Mutex<Screen> = Mutex::new(Screen {
    version: "",
    steps: [const { None }; 4],
    traffic: None,
    open: false,
    drawn: None,
});

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Done,
    Failed,
    Pending,
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
}

/// Whether the live log is shown.
pub fn verbose() -> bool {
    !STEPS.load(Ordering::Relaxed)
}

fn paint(code: &str, text: &str) -> String {
    let plain = !std::io::stdout().is_terminal() || std::env::var_os("NO_COLOR").is_some();
    if plain { text.to_string() } else { format!("\x1b[{code}m{text}\x1b[0m") }
}

impl Screen {
    /// One line; `stays`: the next line goes below it.
    fn line(&mut self, text: &str, stays: bool) {
        if verbose() {
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
            let (sign, colour) = match mark {
                Mark::Done => ("\u{2713}", "32"),
                Mark::Failed => ("\u{2717}", "31"),
                Mark::Pending => ("\u{2022}", "33"),
            };
            let detail = if mark == Mark::Done { paint("2", &detail) } else { paint(colour, &detail) };
            let text = format!("  {}  {} {detail}", paint(colour, sign), paint("1", &format!("{name:<11}")));
            self.line(&text, mark != Mark::Pending);
        }
    }

    fn set(&mut self, i: usize, name: &'static str, mark: Mark, detail: &str) {
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

    /// Everything again, after the live log.
    fn redraw(&mut self) {
        self.open = false;
        let keys = "Enter: live log   Ctrl+C: quit";
        println!("\n  {}   {}\n", paint("1", &format!("pspkit-usbnet {}", self.version)), paint("2", keys));
        for i in 0..self.steps.len() {
            self.step_line(i);
        }
        self.traffic_line();
    }
}

/// The head and the two steps that do not need the PSP.
pub fn start(version: &'static str, usb_ok: bool) {
    STEPS.store(true, Ordering::Relaxed);
    let mut s = SCREEN.lock().unwrap();
    s.version = version;
    s.redraw();
    // No packet is sent: this only asks which address would be used.
    let lan = UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("1.1.1.1:53").and_then(|_| s.local_addr()));
    match lan {
        Ok(a) => s.set(NETWORK, "Network", Mark::Done, &format!("ok  {}", a.ip())),
        Err(_) => s.set(NETWORK, "Network", Mark::Failed, "offline: the PSP will only reach this computer"),
    }
    if usb_ok {
        s.set(USB, "USB", Mark::Done, "ok");
    } else {
        s.set(USB, "USB", Mark::Failed, "cannot be used");
    }
    s.set(PSP, "PSP", Mark::Pending, WAITING);
    drop(s);
    if std::io::stdin().is_terminal() {
        std::thread::spawn(keys);
    }
}

/// Enter switches between the steps and the live log.
fn keys() {
    let mut line = String::new();
    while std::io::stdin().read_line(&mut line).is_ok_and(|n| n > 0) {
        let mut s = SCREEN.lock().unwrap();
        if verbose() {
            STEPS.store(true, Ordering::Relaxed);
            s.redraw();
        } else {
            STEPS.store(false, Ordering::Relaxed);
            println!("\n  {}   {}\n", paint("1", "Live log"), paint("2", "Enter: back   Ctrl+C: quit"));
        }
        line.clear();
    }
}

pub fn psp_waiting(why: &str) {
    SCREEN.lock().unwrap().set(PSP, "PSP", Mark::Pending, why);
}

pub fn psp_found() {
    let mut s = SCREEN.lock().unwrap();
    s.set(PSP, "PSP", Mark::Done, "found");
    s.set(CONNECTION, "Connection", Mark::Pending, "handing out an address");
}

/// The connection ended or the cable was pulled: back to waiting. A traffic
/// line stays as the total; an unfinished step goes.
pub fn psp_lost() {
    let mut s = SCREEN.lock().unwrap();
    if s.open && s.traffic.is_some() && !verbose() {
        println!();
        s.open = false;
    }
    s.set(PSP, "PSP", Mark::Pending, WAITING);
}

pub fn connected(ip: &str) {
    let mut s = SCREEN.lock().unwrap();
    if !matches!(s.steps[CONNECTION], Some((_, Mark::Done, _))) {
        s.set(CONNECTION, "Connected", Mark::Done, ip);
    }
}

fn amount(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b >> 10),
    }
}

/// The traffic line, at most twice a second and only on a terminal.
pub fn traffic(down: u64, up: u64, connections: usize) {
    let mut s = SCREEN.lock().unwrap();
    let now = Instant::now();
    let connected = matches!(s.steps[CONNECTION], Some((_, Mark::Done, _)));
    if !connected || !std::io::stdout().is_terminal() {
        return;
    }
    if s.drawn.is_some_and(|t| now < t + Duration::from_millis(500)) {
        return;
    }
    let busy = match connections {
        0 => "idle".to_string(),
        1 => "1 connection".to_string(),
        n => format!("{n} connections"),
    };
    s.traffic = Some(format!("\u{2193} {}   \u{2191} {}   {busy}", amount(down), amount(up)));
    s.drawn = Some(now);
    s.traffic_line();
}
