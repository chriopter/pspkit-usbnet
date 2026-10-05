//! How far the connection is, told by the USB link and the gateway to
//! whoever shows it (`ui`). Nobody listening is the default.

use std::fmt;
use std::net::Ipv4Addr;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// No PSP on USB yet, or its interface is not there.
    Waiting,
    /// The PSP is there, but another program has its interface.
    Busy,
    /// The PSP is there, but this user may not open it.
    NoAccess,
    Found,
    /// The connection ended or the cable was pulled.
    Lost,
    /// The PSP has its address.
    Connected(Ipv4Addr),
    /// Bytes relayed over TCP so far, and the connections open now.
    Traffic { down: u64, up: u64, connections: usize },
}

#[derive(Clone, Default)]
pub struct Status(Option<Arc<dyn Fn(Event) + Send + Sync>>);

impl Status {
    pub fn to(listener: impl Fn(Event) + Send + Sync + 'static) -> Status {
        Status(Some(Arc::new(listener)))
    }

    pub fn tell(&self, event: Event) {
        if let Some(listener) = &self.0 {
            listener(event);
        }
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "Status(listener)" } else { "Status(none)" })
    }
}
