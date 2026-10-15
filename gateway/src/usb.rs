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
    state: Mutex<TxState>,
    wake: Condvar,
    room: Condvar,
}
#[derive(Default)]
struct TxState {
    queue: VecDeque<Vec<u8>>,
    queued: usize,
    stopped: bool,
}

impl Tx {
    fn new() -> Self {
        Self { state: Mutex::new(TxState::default()), wake: Condvar::new(), room: Condvar::new() }
    }

    fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.stopped = true;
        state.queue.clear();
        state.queued = 0;
        self.wake.notify_all();
        self.room.notify_all();
    }

    fn enqueue(&self, frame: &[u8]) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        while !state.stopped && state.queued + frame.len() > QUEUE_LIMIT {
            state = self.room.wait(state).unwrap();
        }
        if state.stopped {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "USB connection closed"));
        }
        state.queued += frame.len();
        state.queue.push_back(frame.to_vec());
        self.wake.notify_one();
        Ok(())
    }

    fn next_batch(&self, batch: &mut Vec<u8>) -> bool {
        let mut state = self.state.lock().unwrap();
        while !state.stopped && state.queue.is_empty() {
            state = self.wake.wait(state).unwrap();
        }
        if state.stopped { return false; }
        batch.clear();
        while let Some(frame) = state.queue.front() {
            if batch.len() + 2 + frame.len() > BATCH - 4 { break; }
            batch.extend_from_slice(&(frame.len() as u16).to_le_bytes());
            batch.extend_from_slice(frame);
            let len = frame.len();
            state.queued -= len;
            state.queue.pop_front();
        }
        self.room.notify_all();
        true
    }
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
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
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
            let tx = Arc::new(Tx::new());
            let writer = {
                let (handle, tx) = (handle.clone(), tx.clone());
                let (ep_out, max_packet) = (found.ep_out, found.max_packet);
                std::thread::spawn(move || writer(&handle, &tx, ep_out, max_packet))
            };
            return Ok(UsbConn {
                handle,
                tx,
                writer: Mutex::new(Some(writer)),
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
        self.tx.stop();
        // No writer may use the interface after it is released or reclaimed.
        // An in-flight bulk write has the bounded WRITE_TIMEOUT above.
        if let Some(writer) = self.writer.lock().unwrap().take() {
            let _ = writer.join();
        }
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
        self.invalidate();
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
        if !tx.next_batch(&mut batch) { return; }
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
                tx.stop();
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
        if frame.is_empty() || frame.len() > 1600 {
            return Ok(());
        }
        self.tx.enqueue(frame)
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
                        OpenError::NoInterface => Event::NoInterface,
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
    fn disconnect_wakes_a_sender_waiting_for_queue_space() {
        let tx = Arc::new(Tx::new());
        let frame = vec![0x42; 1600];
        for _ in 0..QUEUE_LIMIT / frame.len() { tx.enqueue(&frame).unwrap(); }
        let (done, result) = std::sync::mpsc::channel();
        let sender = tx.clone();
        let thread = std::thread::spawn(move || { done.send(sender.enqueue(&frame)).unwrap(); });
        assert!(result.recv_timeout(Duration::from_millis(30)).is_err());
        tx.stop();
        assert_eq!(result.recv_timeout(Duration::from_secs(1)).unwrap().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        thread.join().unwrap();
        assert_eq!(tx.state.lock().unwrap().queued, 0);
    }

    #[test]
    fn disconnect_wakes_an_idle_writer_and_discards_pending_frames() {
        let tx = Arc::new(Tx::new());
        let (done, result) = std::sync::mpsc::channel();
        let reader = tx.clone();
        let thread = std::thread::spawn(move || {
            done.send(reader.next_batch(&mut Vec::new())).unwrap();
        });
        tx.stop();
        assert!(!result.recv_timeout(Duration::from_secs(1)).unwrap());
        thread.join().unwrap();
        let tx = Tx::new();
        tx.enqueue(&[1, 2, 3]).unwrap();
        tx.stop();
        assert!(!tx.next_batch(&mut Vec::new()));
        assert_eq!(tx.enqueue(&[4]).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn batches_keep_frame_boundaries_and_queue_accounting() {
        let tx = Tx::new();
        let frames: Vec<_> = (0..20).map(|n| vec![n; 1500]).collect();
        for frame in &frames { tx.enqueue(frame).unwrap(); }
        let mut received = Vec::new();
        let mut batch = Vec::new();
        for _ in 0..2 {
            assert!(tx.next_batch(&mut batch));
            assert!(batch.len() <= BATCH - 4);
            let mut bytes = &batch[..];
            while !bytes.is_empty() {
                let len = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
                received.push(bytes[2..2 + len].to_vec());
                bytes = &bytes[2 + len..];
            }
        }
        assert_eq!(received, frames);
        assert_eq!(tx.state.lock().unwrap().queued, 0);
    }

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
