//! What the user sees: four steps that say how far the connection is, then
//! one line of traffic. On a terminal the step in progress and the traffic
//! are rewritten in place, in colour; into a file or pipe every change is a
//! plain line of its own and traffic is left out. Enter switches between
//! this and the event log.

use crate::log;
use crate::status::{Event, Status};
use std::io::{IsTerminal, Write};
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const NETWORK: usize = 0;
const USB: usize = 1;
const PSP: usize = 2;
const CONNECTION: usize = 3;
const WAITING: &str = "waiting: choose \"Hi-Speed USB\" on the PSP";

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
            let (sign, colour) = match mark {
                Mark::Done => ("\u{2713}", "32"),
                Mark::Failed => ("\u{2717}", "31"),
                Mark::Pending => ("\u{2022}", "33"),
            };
            let detail = paint(if mark == Mark::Done { "2" } else { colour }, &detail);
            let name = paint("1", &format!("{name:<11}"));
            self.line(&format!("  {}  {name} {detail}", paint(colour, sign)), mark != Mark::Pending);
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

    /// Everything, at the start and when coming back from the event log.
    fn draw(&mut self) {
        self.open = false;
        let head = paint("1", &format!("pspkit-usbnet {}", self.version));
        println!("\n  {head}   {}\n", paint("2", "Enter: event log   Ctrl+C: quit"));
        for i in 0..self.steps.len() {
            self.step_line(i);
        }
        self.traffic_line();
    }

    fn connected(&self) -> bool {
        matches!(self.steps[CONNECTION], Some((_, Mark::Done, _)))
    }

    fn on(&mut self, event: Event) {
        match event {
            Event::Waiting => self.set(PSP, "PSP", Mark::Pending, WAITING),
            Event::NoAccess => self.set(PSP, "PSP", Mark::Pending, "found, but no access to USB (try sudo)"),
            Event::Found => {
                self.set(PSP, "PSP", Mark::Done, "found");
                self.set(CONNECTION, "Connection", Mark::Pending, "handing out an address");
            }
            Event::Lost => {
                // A traffic line stays as the total; an unfinished step goes.
                if self.open && self.traffic.is_some() && !log::shown() {
                    println!();
                    self.open = false;
                }
                self.set(PSP, "PSP", Mark::Pending, WAITING);
            }
            Event::Connected(ip) if !self.connected() => {
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
                let busy = match connections {
                    0 => "idle".to_string(),
                    1 => "1 connection".to_string(),
                    n => format!("{n} connections"),
                };
                self.traffic = Some(format!("\u{2193} {}   \u{2191} {}   {busy}", amount(down), amount(up)));
                self.drawn = Some(now);
                self.traffic_line();
            }
        }
    }
}

/// Puts the screen up, with the two steps that do not need the PSP, and
/// returns where the rest is told to. `usb`: whether USB can be used at all.
pub fn start(version: &'static str, usb: impl FnOnce(&Status) -> bool) -> Status {
    log::show(false);
    let screen = Arc::new(Mutex::new(Screen {
        version,
        steps: [const { None }; 4],
        traffic: None,
        open: false,
        drawn: None,
    }));
    let listener = screen.clone();
    let status = Status::to(move |event| listener.lock().unwrap().on(event));
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
    let usb_ok = usb(&status);
    {
        let mut s = screen.lock().unwrap();
        if usb_ok {
            s.set(USB, "USB", Mark::Done, "ok");
        } else {
            s.set(USB, "USB", Mark::Failed, "cannot be used");
        }
        s.on(Event::Waiting);
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
            log::show(true);
            println!("\n  {}   {}\n", paint("1", "Event log"), paint("2", "Enter: back   Ctrl+C: quit"));
        }
        line.clear();
    }
}
