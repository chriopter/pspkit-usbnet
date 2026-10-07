//! The USB transport: Ethernet frames on the PSP's usbnet interface (class
//! 0xfd of the composite device 054c:01c9).
//!
//! A bulk transfer carries one or more frames, each behind its length (u16,
//! little endian); a length of zero ends the transfer early. One frame per
//! transfer costs a USB round trip per frame, which held a download at
//! 4.8 MB/s; whatever has queued up while the last transfer was on the wire
//! now goes out together.
//!
//! Only that interface is claimed; interface 0 belongs to usbhostfs_pc.
//!
//! Several PSPs, each on its own cable, each have a link of their own. The
//! links share who holds which device, so that each takes a different one.

use crate::device::FrameDevice;
use crate::log;
use crate::logln;
use crate::status::{Event, Status};
use rusb::{Direction, TransferType, UsbContext};
use std::fmt;
use std::io;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const VENDOR_ID: u16 = 0x054c;
pub const PRODUCT_ID: u16 = 0x01c9;
pub const INTERFACE_CLASS: u8 = 0xfd;
/// PSPs served at once: so many links look for one.
pub const LINKS: usize = 4;

const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// The PSP receives into a buffer of this size; one transfer stays below it.
const BATCH: usize = 16 * 1024;
/// Frames waiting for the wire beyond this make `send` wait.
const QUEUE_LIMIT: usize = 256 * 1024;

/// What `send` and the writer thread share.
struct Tx {
    queue: Mutex<VecDeque<Vec<u8>>>,
    queued: Mutex<usize>,
    wake: Condvar,
    room: Condvar,
    failed: AtomicBool,
    stop: AtomicBool,
}
const RETRY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// No 054c:01c9 on the bus.
    Absent,
    /// The device is there, but has no class 0xfd interface with two bulk
    /// endpoints (usbnet.prx not loaded).
    NoInterface,
    /// Another program has the interface.
    Busy,
    /// This user may not open the device (Linux: permissions; Windows: no
    /// WinUSB driver on the interface).
    Denied,
    /// Opening or claiming failed otherwise.
    Failed(String),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Absent => write!(f, "no PSP ({VENDOR_ID:04x}:{PRODUCT_ID:04x}) on USB"),
            OpenError::NoInterface => write!(
                f,
                "PSP present, but without the usbnet interface (class 0x{INTERFACE_CLASS:02x})"
            ),
            OpenError::Busy => write!(f, "another program is using the usbnet interface"),
            OpenError::Denied => write!(f, "PSP present, but no access to it: {}", crate::access::HINT),
            OpenError::Failed(e) => write!(f, "cannot use the usbnet interface: {e}"),
        }
    }
}

/// Where a device is plugged in: its bus and the ports down to it. Where
/// the ports are not known, its address on the bus stands in.
type Place = (u8, Vec<u8>);

/// What the links of one process share: libusb, and which PSP each holds.
pub struct Bus {
    ctx: Option<rusb::Context>,
    /// The devices held and what they are called. A link looks for a
    /// device under this lock, so two never go for the same one.
    held: Mutex<Vec<(Place, String)>>,
    /// The last reason for not having a device, so each is logged once.
    last_problem: Mutex<Option<OpenError>>,
}

impl Bus {
    pub fn new() -> Bus {
        let ctx = match rusb::Context::new() {
            Ok(c) => Some(c),
            Err(e) => {
                logln!("usb: libusb does not start: {e}");
                None
            }
        };
        Bus { ctx, held: Mutex::new(Vec::new()), last_problem: Mutex::new(None) }
    }

    /// Whether libusb started.
    pub fn usable(&self) -> bool {
        self.ctx.is_some()
    }
}

impl Default for Bus {
    fn default() -> Bus {
        Bus::new()
    }
}

/// A held device's entry in the bus; it goes with the connection.
struct Claim {
    bus: Arc<Bus>,
    place: Place,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.bus.held.lock().unwrap().retain(|(p, _)| *p != self.place);
    }
}

/// The model in the string the plugin gives its interface ("PSP Go"). None
/// for anything else: a plugin before 0.1.5 has no string, one that does not
/// know its model says "PSP", and beside PSPLink the string may be another's.
pub fn model(told: &str) -> Option<&str> {
    let told = told.trim_matches(|c: char| c == '\0' || c == ' ');
    let sane = told.len() <= 16 && told.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-');
    (sane && told.len() > 3 && told.starts_with("PSP")).then_some(told)
}

/// What a PSP is called: its model, with a number behind the second of a
/// kind; without a model "PSP 1", "PSP 2". The first name not in `taken`,
/// so the only PSP is the first whichever link has it.
pub fn label(model: Option<&str>, taken: &[String]) -> String {
    (1..)
        .map(|n| match model {
            Some(m) if n == 1 => m.to_string(),
            Some(m) => format!("{m} ({n})"),
            None => format!("PSP {n}"),
        })
        .find(|name| !taken.contains(name))
        .unwrap()
}

/// The usbnet interface in a device's configuration.
struct Found {
    interface: u8,
    ep_in: u8,
    ep_out: u8,
    max_packet: usize,
    /// Where its string is, if it has one.
    string: Option<u8>,
}

fn usbnet_interface(dev: &rusb::Device<rusb::Context>) -> Option<Found> {
    let cfg = dev.active_config_descriptor().ok()?;
    for intf in cfg.interfaces() {
        let Some(d) = intf.descriptors().next() else { continue };
        if d.class_code() != INTERFACE_CLASS {
            continue;
        }
        let (mut ep_in, mut ep_out) = (None, None);
        for e in d.endpoint_descriptors() {
            if e.transfer_type() != TransferType::Bulk {
                continue;
            }
            match e.direction() {
                Direction::In => ep_in = ep_in.or(Some(e.address())),
                Direction::Out => ep_out = ep_out.or(Some((e.address(), e.max_packet_size()))),
            }
        }
        let (Some(ep_in), Some((ep_out, mps))) = (ep_in, ep_out) else { continue };
        return Some(Found {
            interface: d.interface_number(),
            ep_in,
            ep_out,
            max_packet: usize::from(mps).max(1),
            string: d.description_string_index(),
        });
    }
    None
}

/// The claimed usbnet interface of one enumerated device.
pub struct UsbConn {
    handle: Arc<rusb::DeviceHandle<rusb::Context>>,
    tx: Arc<Tx>,
    /// Frames of a received transfer not handed out yet.
    rx: Mutex<VecDeque<Vec<u8>>>,
    interface: u8,
    ep_in: u8,
    ep_out: u8,
    max_packet: usize,
    place: String,
    /// The interface's string as the PSP gave it.
    told: Option<String>,
    label: String,
    /// Lives as long as this does: the device is another link's to take
    /// only once the interface has been given up.
    _claim: Claim,
}

impl UsbConn {
    /// The first PSP, for a program with one link.
    pub fn open() -> Result<UsbConn, OpenError> {
        Self::open_in(&Arc::new(Bus::new()))
    }

    /// A PSP that no other link of `bus` holds. One that cannot be had is
    /// passed over for the next; its reason is returned if none is left.
    pub fn open_in(bus: &Arc<Bus>) -> Result<UsbConn, OpenError> {
        let fail = |what: &str, e: rusb::Error| match e {
            rusb::Error::Busy => OpenError::Busy,
            rusb::Error::Access => OpenError::Denied,
            // What libusb says on Windows where no WinUSB driver is bound.
            rusb::Error::NotSupported | rusb::Error::NotFound if cfg!(windows) => OpenError::Denied,
            e => OpenError::Failed(format!("{what}: {e}")),
        };
        let Some(ctx) = &bus.ctx else {
            return Err(OpenError::Failed("libusb did not start".into()));
        };
        // The list is taken under the lock too: libusb on Windows does not
        // survive several threads listing devices at the same time.
        let mut held = bus.held.lock().unwrap();
        let devices = ctx.devices().map_err(|e| fail("listing devices", e))?;
        let mut result = OpenError::Absent;
        for dev in devices.iter() {
            let Ok(dd) = dev.device_descriptor() else { continue };
            if dd.vendor_id() != VENDOR_ID || dd.product_id() != PRODUCT_ID {
                continue;
            }
            let place: Place = match dev.port_numbers() {
                Ok(ports) if !ports.is_empty() => (dev.bus_number(), ports),
                _ => (dev.bus_number(), vec![dev.address()]),
            };
            // Another link's PSP is not there for this one. libusb would
            // not say so: within one process a second claim may succeed.
            if held.iter().any(|(p, _)| *p == place) {
                continue;
            }
            let Some(found) = usbnet_interface(&dev) else {
                if result == OpenError::Absent {
                    result = OpenError::NoInterface;
                }
                continue;
            };
            let handle = match dev.open() {
                Ok(h) => h,
                Err(e) => {
                    result = fail("open", e);
                    continue;
                }
            };
            if let Err(e) = handle.claim_interface(found.interface) {
                result = fail("claim interface", e);
                continue;
            }
            // Asked once, here; a PSP that does not answer has no model.
            let told = found.string.and_then(|i| handle.read_string_descriptor_ascii(i).ok());
            let taken: Vec<String> = held.iter().map(|(_, name)| name.clone()).collect();
            let label = label(told.as_deref().and_then(model), &taken);
            held.push((place.clone(), label.clone()));
            let handle = Arc::new(handle);
            let tx = Arc::new(Tx {
                queue: Mutex::new(VecDeque::new()),
                queued: Mutex::new(0),
                wake: Condvar::new(),
                room: Condvar::new(),
                failed: AtomicBool::new(false),
                stop: AtomicBool::new(false),
            });
            {
                let (handle, tx) = (handle.clone(), tx.clone());
                let (ep_out, max_packet) = (found.ep_out, found.max_packet);
                std::thread::spawn(move || writer(&handle, &tx, ep_out, max_packet));
            }
            return Ok(UsbConn {
                handle,
                tx,
                rx: Mutex::new(VecDeque::new()),
                interface: found.interface,
                ep_in: found.ep_in,
                ep_out: found.ep_out,
                max_packet: found.max_packet,
                place: format!("bus {} device {}", dev.bus_number(), dev.address()),
                told,
                label,
                _claim: Claim { bus: bus.clone(), place },
            });
        }
        // A PSP alone on the bus without a driver is not in libusb's list.
        if result == OpenError::Absent && crate::access::driverless() {
            result = OpenError::Denied;
        }
        Err(result)
    }

    /// Gives the interface up now, whoever still holds this connection.
    pub fn invalidate(&self) {
        self.tx.stop.store(true, Ordering::SeqCst);
        self.tx.wake.notify_all();
        let _ = self.handle.release_interface(self.interface);
    }

    pub fn describe(&self) -> String {
        let told = match &self.told {
            Some(t) => format!("says \"{t}\""),
            None => "says no model".to_string(),
        };
        format!(
            "{}, interface {}, in 0x{:02x} out 0x{:02x}, max packet {}, {told}",
            self.place, self.interface, self.ep_in, self.ep_out, self.max_packet
        )
    }

    /// What this PSP is called among the ones on the bus.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl Drop for UsbConn {
    fn drop(&mut self) {
        self.tx.stop.store(true, Ordering::SeqCst);
        self.tx.wake.notify_all();
        let _ = self.handle.release_interface(self.interface);
    }
}

/// Sends what has queued up, as many whole frames as one transfer holds.
/// Sleeps on the condition variable while there is nothing.
fn writer(handle: &rusb::DeviceHandle<rusb::Context>, tx: &Tx, ep_out: u8, max_packet: usize) {
    let mut batch = Vec::with_capacity(BATCH);
    // USBNET_DEBUG=1: what the transfers look like, every two seconds.
    let debug = std::env::var_os("USBNET_DEBUG").is_some();
    let (mut transfers, mut bytes, mut busy, mut since) = (0usize, 0usize, Duration::ZERO, Instant::now());
    loop {
        batch.clear();
        {
            let mut q = tx.queue.lock().unwrap();
            while q.is_empty() {
                if tx.stop.load(Ordering::SeqCst) {
                    return;
                }
                q = tx.wake.wait(q).unwrap();
            }
            let mut taken = 0;
            while let Some(f) = q.front() {
                if batch.len() + 2 + f.len() > BATCH - 4 {
                    break;
                }
                batch.extend_from_slice(&(f.len() as u16).to_le_bytes());
                batch.extend_from_slice(f);
                taken += f.len();
                q.pop_front();
            }
            *tx.queued.lock().unwrap() -= taken;
        }
        tx.room.notify_all();
        // A transfer ends with a short packet.
        if batch.len() % max_packet == 0 {
            batch.extend_from_slice(&[0, 0]);
        }
        let started = Instant::now();
        let result = handle.write_bulk(ep_out, &batch, WRITE_TIMEOUT);
        if debug {
            transfers += 1;
            bytes += batch.len();
            busy += started.elapsed();
            if since.elapsed() >= Duration::from_secs(2) {
                eprintln!(
                    "usb out: {} transfers, {} bytes, mean {} B, {:.0} us each, {:.0}% of the time writing",
                    transfers,
                    bytes,
                    bytes / transfers.max(1),
                    busy.as_micros() as f64 / transfers.max(1) as f64,
                    100.0 * busy.as_secs_f64() / since.elapsed().as_secs_f64()
                );
                transfers = 0;
                bytes = 0;
                busy = Duration::ZERO;
                since = Instant::now();
            }
        }
        match result {
            Ok(n) if n == batch.len() => {}
            _ => {
                tx.failed.store(true, Ordering::SeqCst);
                tx.room.notify_all();
                return;
            }
        }
    }
}

fn io_err(e: rusb::Error) -> io::Error {
    let kind = match e {
        rusb::Error::Timeout => io::ErrorKind::TimedOut,
        rusb::Error::NoDevice => io::ErrorKind::NotConnected,
        rusb::Error::Pipe => io::ErrorKind::BrokenPipe,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, e.to_string())
}

impl FrameDevice for UsbConn {
    fn recv(&self, buf: &mut [u8], timeout: Option<Duration>) -> io::Result<usize> {
        let mut pending = self.rx.lock().unwrap();
        if pending.is_empty() {
            // Zero means "no timeout" to libusb, so a given timeout is at
            // least a millisecond.
            let t = timeout.map_or(Duration::ZERO, |t| t.max(Duration::from_millis(1)));
            let mut raw = [0u8; 2 * BATCH];
            let n = self.handle.read_bulk(self.ep_in, &mut raw, t).map_err(io_err)?;
            let mut at = 0;
            while at + 2 <= n {
                let len = usize::from(u16::from_le_bytes([raw[at], raw[at + 1]]));
                at += 2;
                if len == 0 || at + len > n {
                    break;
                }
                pending.push_back(raw[at..at + len].to_vec());
                at += len;
            }
        }
        match pending.pop_front() {
            Some(f) if f.len() <= buf.len() => {
                buf[..f.len()].copy_from_slice(&f);
                Ok(f.len())
            }
            Some(_) => Ok(0),
            None => Ok(0),
        }
    }

    fn send(&self, frame: &[u8]) -> io::Result<()> {
        let broken = || io::Error::new(io::ErrorKind::BrokenPipe, "USB write failed");
        if frame.is_empty() || frame.len() > 1600 {
            return Ok(());
        }
        let mut queued = self.tx.queued.lock().unwrap();
        while *queued > QUEUE_LIMIT {
            if self.tx.failed.load(Ordering::SeqCst) {
                return Err(broken());
            }
            queued = self.tx.room.wait_timeout(queued, WRITE_TIMEOUT).unwrap().0;
        }
        if self.tx.failed.load(Ordering::SeqCst) {
            return Err(broken());
        }
        *queued += frame.len();
        drop(queued);
        self.tx.queue.lock().unwrap().push_back(frame.to_vec());
        self.tx.wake.notify_one();
        Ok(())
    }
}

/// The USB device as the daemon sees it: always there. While the PSP is
/// absent `recv` waits for it (looking once a second) and `send` loses the
/// frame; when the device goes away the link is dropped and found again.
/// It keeps nothing of a PSP that went: the next one may be any, and the
/// one that went may come back on another link.
pub struct UsbLink {
    bus: Arc<Bus>,
    conn: Mutex<Option<Arc<UsbConn>>>,
    /// The name of the PSP it has, for the log.
    label: log::Label,
    status: Status,
}

impl UsbLink {
    pub fn new(bus: Arc<Bus>, status: Status) -> UsbLink {
        UsbLink { bus, conn: Mutex::new(None), label: log::Label::default(), status }
    }

    /// For the threads that work for this link's PSP (`log::set_label`).
    pub fn label(&self) -> log::Label {
        self.label.clone()
    }

    fn current(&self) -> Option<Arc<UsbConn>> {
        self.conn.lock().unwrap().clone()
    }

    fn drop_conn(&self, conn: &Arc<UsbConn>, why: &io::Error) {
        let mut cur = self.conn.lock().unwrap();
        if cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
            *cur = None;
            logln!("usb: device lost ({why})");
            self.label.lock().unwrap().clear();
            // What keeps the next one away is worth saying again.
            *self.bus.last_problem.lock().unwrap() = None;
            self.status.tell(Event::Lost);
        }
    }

    /// The connection, opening it if need be. None after one failed attempt.
    fn connect(&self) -> Option<Arc<UsbConn>> {
        if let Some(c) = self.current() {
            return Some(c);
        }
        match UsbConn::open_in(&self.bus) {
            Ok(c) => {
                *self.label.lock().unwrap() = c.label().to_string();
                logln!("usb: device found ({})", c.describe());
                self.status.tell(Event::Found(c.label().to_string()));
                *self.bus.last_problem.lock().unwrap() = None;
                let c = Arc::new(c);
                *self.conn.lock().unwrap() = Some(c.clone());
                Some(c)
            }
            Err(e) => {
                // Every idle link runs into the same thing: said by one.
                // And "no PSP" is not said while other links have theirs.
                let others = e == OpenError::Absent && !self.bus.held.lock().unwrap().is_empty();
                let mut last = self.bus.last_problem.lock().unwrap();
                if !others && last.as_ref() != Some(&e) {
                    logln!("usb: waiting for the device: {e}");
                    self.status.tell(match &e {
                        OpenError::Busy => Event::Busy,
                        OpenError::Denied => Event::NoAccess,
                        OpenError::Failed(_) => Event::Broken,
                        _ => Event::Waiting,
                    });
                    let denied = e == OpenError::Denied;
                    *last = Some(e);
                    drop(last);
                    if denied {
                        crate::access::elevate();
                    }
                }
                None
            }
        }
    }
}

impl FrameDevice for UsbLink {
    fn recv(&self, buf: &mut [u8], timeout: Option<Duration>) -> io::Result<usize> {
        let deadline = timeout.map(|t| Instant::now() + t);
        let left = || deadline.map(|d| d.saturating_duration_since(Instant::now()));
        // Sleep, but not past the deadline. False when the time is up.
        let pause = || match left() {
            Some(l) if l.is_zero() => false,
            Some(l) => {
                std::thread::sleep(l.min(RETRY));
                true
            }
            None => {
                std::thread::sleep(RETRY);
                true
            }
        };
        loop {
            let Some(conn) = self.connect() else {
                if !pause() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                continue;
            };
            match conn.recv(buf, left()) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => return Err(e),
                Err(e) => {
                    self.drop_conn(&conn, &e);
                    drop(conn);
                    if !pause() {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                }
            }
        }
    }

    fn send(&self, frame: &[u8]) -> io::Result<()> {
        let Some(conn) = self.current() else {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "no device"));
        };
        match conn.send(frame) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Any failed write ends this connection: the writer thread
                // has stopped. Releasing the interface makes the read that
                // the receiving side is blocked in return, so that it opens
                // the device again.
                conn.invalidate();
                self.drop_conn(&conn, &e);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_is_what_the_plugin_says() {
        for m in ["PSP-1000", "PSP-2000", "PSP-3000", "PSP Go", "PSP Street"] {
            assert_eq!(model(m), Some(m));
        }
        assert_eq!(model("PSP Go\0"), Some("PSP Go"));
        // The plugin does not know, usbhostfs's string, something else.
        assert_eq!(model("PSP"), None);
        assert_eq!(model("<>"), None);
        assert_eq!(model(""), None);
        assert_eq!(model("\"PSP\" Type B"), None);
        assert_eq!(model("PSP with a very long name"), None);
    }

    #[test]
    fn a_psp_is_called_by_its_model() {
        assert_eq!(label(Some("PSP Go"), &[]), "PSP Go");
        assert_eq!(label(Some("PSP Go"), &["PSP-1000".into(), "PSP 1".into()]), "PSP Go");
    }

    #[test]
    fn two_of_a_kind_are_numbered() {
        let mut taken = vec![label(Some("PSP-3000"), &[])];
        taken.push(label(Some("PSP-3000"), &taken));
        taken.push(label(Some("PSP-3000"), &taken));
        assert_eq!(taken, ["PSP-3000", "PSP-3000 (2)", "PSP-3000 (3)"]);
        // The first went away: the next of the kind has its name.
        assert_eq!(label(Some("PSP-3000"), &taken[1..]), "PSP-3000");
    }

    #[test]
    fn without_a_model_they_are_counted() {
        assert_eq!(label(None, &[]), "PSP 1");
        assert_eq!(label(None, &["PSP 1".into()]), "PSP 2");
        assert_eq!(label(None, &["PSP Go".into()]), "PSP 1");
        assert_eq!(label(None, &["PSP 2".into()]), "PSP 1");
    }
}
