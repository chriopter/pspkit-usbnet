//! The "frame device": something that carries whole Ethernet frames to and
//! from the PSP. The gateway only knows this trait.

use std::io;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// Buffer size a caller of [`FrameDevice::recv`] should use.
pub const RECV_BUF: usize = 4096;

pub trait FrameDevice: Send + Sync {
    /// Block until one frame has arrived and return its length. With
    /// `timeout` None this waits forever. `ErrorKind::TimedOut` means nothing
    /// came in time; any other error means the device is gone for good.
    /// The frame may carry one trailing pad byte.
    fn recv(&self, buf: &mut [u8], timeout: Option<Duration>) -> io::Result<usize>;

    /// Send one frame. An error means this frame was lost, nothing more.
    fn send(&self, frame: &[u8]) -> io::Result<()>;
}

/// One end of an in-memory cable, for tests. Like the USB transport it can
/// append a zero byte to frames whose length is a multiple of a packet size.
pub struct MemDevice {
    tx: Sender<Vec<u8>>,
    rx: Mutex<Receiver<Vec<u8>>>,
    pad_multiple: Option<usize>,
}

impl MemDevice {
    /// Two connected ends. `pad_multiple` mimics the USB short-packet rule.
    pub fn pair(pad_multiple: Option<usize>) -> (MemDevice, MemDevice) {
        let (atx, brx) = channel();
        let (btx, arx) = channel();
        (
            MemDevice { tx: atx, rx: Mutex::new(arx), pad_multiple },
            MemDevice { tx: btx, rx: Mutex::new(brx), pad_multiple },
        )
    }
}

impl FrameDevice for MemDevice {
    fn recv(&self, buf: &mut [u8], timeout: Option<Duration>) -> io::Result<usize> {
        let rx = self.rx.lock().unwrap();
        let frame = match timeout {
            None => rx.recv().map_err(|_| io::ErrorKind::BrokenPipe)?,
            Some(t) => rx.recv_timeout(t).map_err(|e| match e {
                RecvTimeoutError::Timeout => io::ErrorKind::TimedOut,
                RecvTimeoutError::Disconnected => io::ErrorKind::BrokenPipe,
            })?,
        };
        let n = frame.len().min(buf.len());
        buf[..n].copy_from_slice(&frame[..n]);
        Ok(n)
    }

    fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut v = Vec::with_capacity(frame.len() + 1);
        v.extend_from_slice(frame);
        if self.pad_multiple.is_some_and(|m| m > 0 && frame.len() % m == 0) {
            v.push(0);
        }
        self.tx.send(v).map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
}
