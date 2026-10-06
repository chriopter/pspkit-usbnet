//! How far the connection is, told by the USB link and the gateway to
//! whoever shows it (`ui`). Nobody listening is the default. Each PSP has
//! its own link and gateway; what they tell goes out with their number.

use std::fmt;
use std::net::Ipv4Addr;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// No PSP on USB yet, or its interface is not there.
    Waiting,
    /// The PSP is there, but another program has its interface.
    Busy,
    /// The PSP is there, but this user may not open it.
    NoAccess,
    /// A link has a PSP; what it is called.
    Found(String),
    /// The connection ended or the cable was pulled.
    Lost,
    /// The PSP has its address.
    Connected(Ipv4Addr),
    /// Bytes relayed over TCP so far, and the connections open now.
    Traffic { down: u64, up: u64, connections: usize },
}

#[derive(Clone, Default)]
pub struct Status {
    listener: Option<Arc<dyn Fn(usize, Event) + Send + Sync>>,
    /// Which of the links this one speaks for, from 0.
    link: usize,
}

impl Status {
    pub fn to(listener: impl Fn(usize, Event) + Send + Sync + 'static) -> Status {
        Status { listener: Some(Arc::new(listener)), link: 0 }
    }

    /// The same listener, told about another link.
    pub fn of(&self, link: usize) -> Status {
        Status { listener: self.listener.clone(), link }
    }

    pub fn tell(&self, event: Event) {
        if let Some(listener) = &self.listener {
            listener(self.link, event);
        }
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.listener.is_some() { "Status(listener)" } else { "Status(none)" })
    }
}
