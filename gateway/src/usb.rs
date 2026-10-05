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

use crate::device::FrameDevice;
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
    /// Opening or claiming failed (permissions, or another program has it).
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
            OpenError::Failed(e) => write!(f, "cannot use the usbnet interface: {e}"),
        }
    }
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
}

impl UsbConn {
    pub fn open() -> Result<UsbConn, OpenError> {
        let ctx = rusb::Context::new().map_err(|e| OpenError::Failed(format!("libusb: {e}")))?;
        Self::open_in(&ctx)
    }

    pub fn open_in(ctx: &rusb::Context) -> Result<UsbConn, OpenError> {
        let fail = |what: &str, e: rusb::Error| OpenError::Failed(format!("{what}: {e}"));
        let devices = ctx.devices().map_err(|e| fail("listing devices", e))?;
        let mut result = OpenError::Absent;
        for dev in devices.iter() {
            let Ok(dd) = dev.device_descriptor() else { continue };
            if dd.vendor_id() != VENDOR_ID || dd.product_id() != PRODUCT_ID {
                continue;
            }
            result = OpenError::NoInterface;
            let Ok(cfg) = dev.active_config_descriptor() else { continue };
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
                        Direction::Out => {
                            ep_out = ep_out.or(Some((e.address(), e.max_packet_size())))
                        }
                    }
                }
                let (Some(ep_in), Some((ep_out, mps))) = (ep_in, ep_out) else { continue };
                let handle = dev.open().map_err(|e| fail("open", e))?;
                handle
                    .claim_interface(d.interface_number())
                    .map_err(|e| fail("claim interface", e))?;
                let handle = Arc::new(handle);
                let max_packet = usize::from(mps).max(1);
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
                    std::thread::spawn(move || writer(&handle, &tx, ep_out, max_packet));
                }
                return Ok(UsbConn {
                    handle,
                    tx,
                    rx: Mutex::new(VecDeque::new()),
                    interface: d.interface_number(),
                    ep_in,
                    ep_out,
                    max_packet: usize::from(mps).max(1),
                    place: format!("bus {} device {}", dev.bus_number(), dev.address()),
                });
            }
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
        format!(
            "{}, interface {}, in 0x{:02x} out 0x{:02x}, max packet {}",
            self.place, self.interface, self.ep_in, self.ep_out, self.max_packet
        )
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
pub struct UsbLink {
    ctx: Option<rusb::Context>,
    conn: Mutex<Option<Arc<UsbConn>>>,
    /// The last reason for not having a device, so each is logged once.
    last_problem: Mutex<Option<OpenError>>,
    status: Status,
}

impl UsbLink {
    /// Whether libusb started.
    pub fn usable(&self) -> bool {
        self.ctx.is_some()
    }

    pub fn new(status: Status) -> UsbLink {
        let ctx = match rusb::Context::new() {
            Ok(c) => Some(c),
            Err(e) => {
                logln!("usb: libusb does not start: {e}");
                None
            }
        };
        UsbLink { ctx, conn: Mutex::new(None), last_problem: Mutex::new(None), status }
    }

    fn current(&self) -> Option<Arc<UsbConn>> {
        self.conn.lock().unwrap().clone()
    }

    fn drop_conn(&self, conn: &Arc<UsbConn>, why: &io::Error) {
        let mut cur = self.conn.lock().unwrap();
        if cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, conn)) {
            *cur = None;
            logln!("usb: device lost ({why})");
            self.status.tell(Event::Lost);
        }
    }

    /// The connection, opening it if need be. None after one failed attempt.
    fn connect(&self) -> Option<Arc<UsbConn>> {
        if let Some(c) = self.current() {
            return Some(c);
        }
        let opened = match &self.ctx {
            Some(ctx) => UsbConn::open_in(ctx),
            None => Err(OpenError::Failed("libusb did not start".into())),
        };
        match opened {
            Ok(c) => {
                logln!("usb: device found ({})", c.describe());
                self.status.tell(Event::Found);
                *self.last_problem.lock().unwrap() = None;
                let c = Arc::new(c);
                *self.conn.lock().unwrap() = Some(c.clone());
                Some(c)
            }
            Err(e) => {
                let mut last = self.last_problem.lock().unwrap();
                if last.as_ref() != Some(&e) {
                    logln!("usb: waiting for the device: {e}");
                    self.status.tell(match &e {
                        OpenError::Failed(_) => Event::NoAccess,
                        _ => Event::Waiting,
                    });
                    *last = Some(e);
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
