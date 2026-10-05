//! A DHCP server for exactly one client and one address. Written to be
//! tolerant: any client MAC, answers always broadcast, options parsed
//! leniently, plain BOOTP requests answered too.

use std::net::Ipv4Addr;

pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;

pub const DISCOVER: u8 = 1;
pub const OFFER: u8 = 2;
pub const REQUEST: u8 = 3;
pub const DECLINE: u8 = 4;
pub const ACK: u8 = 5;
pub const NAK: u8 = 6;
pub const RELEASE: u8 = 7;
pub const INFORM: u8 = 8;

const MAGIC: [u8; 4] = [99, 130, 83, 99];
const BOOTP_LEN: usize = 236;

pub fn type_name(t: u8) -> &'static str {
    match t {
        DISCOVER => "DISCOVER",
        OFFER => "OFFER",
        REQUEST => "REQUEST",
        DECLINE => "DECLINE",
        ACK => "ACK",
        NAK => "NAK",
        RELEASE => "RELEASE",
        INFORM => "INFORM",
        _ => "?",
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub xid: [u8; 4],
    pub flags: [u8; 2],
    pub ciaddr: Ipv4Addr,
    pub giaddr: Ipv4Addr,
    pub htype: u8,
    pub hlen: u8,
    pub chaddr: [u8; 16],
    /// None for a plain BOOTP request (no option 53).
    pub msg_type: Option<u8>,
    pub requested_ip: Option<Ipv4Addr>,
    pub server_id: Option<Ipv4Addr>,
    pub hostname: Option<String>,
}

impl Request {
    pub fn mac(&self) -> &[u8] {
        &self.chaddr[..6]
    }
}

fn ip4(b: &[u8]) -> Option<Ipv4Addr> {
    (b.len() >= 4).then(|| Ipv4Addr::new(b[0], b[1], b[2], b[3]))
}

/// Parse the UDP payload of a client message. None if it is not a BOOTREQUEST.
pub fn parse(p: &[u8]) -> Option<Request> {
    if p.len() < BOOTP_LEN || p[0] != 1 {
        return None;
    }
    let mut req = Request {
        xid: p[4..8].try_into().unwrap(),
        flags: p[10..12].try_into().unwrap(),
        ciaddr: ip4(&p[12..16]).unwrap(),
        giaddr: ip4(&p[24..28]).unwrap(),
        htype: p[1],
        hlen: p[2],
        chaddr: p[28..44].try_into().unwrap(),
        msg_type: None,
        requested_ip: None,
        server_id: None,
        hostname: None,
    };
    if p.len() >= BOOTP_LEN + 4 && p[BOOTP_LEN..BOOTP_LEN + 4] == MAGIC {
        let mut o = &p[BOOTP_LEN + 4..];
        // Stops quietly at the end marker or at a truncated option.
        while let [code, rest @ ..] = o {
            match *code {
                0 => {
                    o = rest;
                    continue;
                }
                255 => break,
                _ => {}
            }
            let Some((&len, rest)) = rest.split_first() else { break };
            let Some(val) = rest.get(..usize::from(len)) else { break };
            match *code {
                53 => req.msg_type = val.first().copied(),
                50 => req.requested_ip = ip4(val),
                54 => req.server_id = ip4(val),
                12 => {
                    let name: String = String::from_utf8_lossy(val)
                        .chars()
                        .filter(|c| c.is_ascii_graphic())
                        .take(64)
                        .collect();
                    if !name.is_empty() {
                        req.hostname = Some(name);
                    }
                }
                _ => {}
            }
            o = &rest[usize::from(len)..];
        }
    }
    Some(req)
}

/// What the server hands out.
#[derive(Debug, Clone, Copy)]
pub struct Lease {
    pub server: Ipv4Addr,
    pub client: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub broadcast: Ipv4Addr,
    pub lease_secs: u32,
}

/// What to do with a client message.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Send this message type back (OFFER, ACK or NAK).
    Reply(u8),
    /// Plain BOOTP: reply without DHCP options.
    BootpReply,
    Ignore,
}

pub fn decide(req: &Request, lease: &Lease) -> Action {
    match req.msg_type {
        None => Action::BootpReply,
        Some(DISCOVER) => Action::Reply(OFFER),
        Some(REQUEST) => {
            if req.server_id.is_some_and(|s| s != lease.server) {
                // The client chose another server. There is none, but stay out.
                return Action::Ignore;
            }
            let wants = req
                .requested_ip
                .or((!req.ciaddr.is_unspecified()).then_some(req.ciaddr));
            match wants {
                Some(ip) if ip != lease.client => Action::Reply(NAK),
                _ => Action::Reply(ACK),
            }
        }
        Some(INFORM) => Action::Reply(ACK),
        _ => Action::Ignore,
    }
}

/// Build the UDP payload of the answer to `req`.
pub fn build_reply(req: &Request, action: &Action, lease: &Lease) -> Vec<u8> {
    let msg_type = match action {
        Action::Reply(t) => Some(*t),
        _ => None,
    };
    let nak = msg_type == Some(NAK);
    let inform = req.msg_type == Some(INFORM);
    let mut p = vec![0u8; BOOTP_LEN];
    p[0] = 2; // BOOTREPLY
    p[1] = req.htype;
    p[2] = req.hlen;
    p[4..8].copy_from_slice(&req.xid);
    p[10..12].copy_from_slice(&req.flags);
    if !nak {
        p[12..16].copy_from_slice(&req.ciaddr.octets());
        if !inform {
            p[16..20].copy_from_slice(&lease.client.octets());
        }
        p[20..24].copy_from_slice(&lease.server.octets()); // siaddr
    }
    p[24..28].copy_from_slice(&req.giaddr.octets());
    p[28..44].copy_from_slice(&req.chaddr);
    p.extend_from_slice(&MAGIC);
    let mut opt = |code: u8, val: &[u8]| {
        p.push(code);
        p.push(val.len() as u8);
        p.extend_from_slice(val);
    };
    if let Some(t) = msg_type {
        opt(53, &[t]);
    }
    opt(54, &lease.server.octets());
    if !nak {
        if msg_type.is_some() && !inform {
            opt(51, &lease.lease_secs.to_be_bytes());
            opt(58, &(lease.lease_secs / 2).to_be_bytes());
            opt(59, &(lease.lease_secs / 8 * 7).to_be_bytes());
        }
        opt(1, &lease.netmask.octets());
        opt(28, &lease.broadcast.octets());
        opt(3, &lease.server.octets());
        opt(6, &lease.server.octets());
    }
    p.push(255);
    // BOOTP's minimum packet size, which old clients insist on.
    if p.len() < 300 {
        p.resize(300, 0);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEASE: Lease = Lease {
        server: Ipv4Addr::new(10, 77, 0, 1),
        client: Ipv4Addr::new(10, 77, 0, 2),
        netmask: Ipv4Addr::new(255, 255, 255, 0),
        broadcast: Ipv4Addr::new(10, 77, 0, 255),
        lease_secs: 86400,
    };

    fn client_msg(options: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; BOOTP_LEN];
        p[0] = 1;
        p[1] = 1;
        p[2] = 6;
        p[4..8].copy_from_slice(&[1, 2, 3, 4]);
        p[28..34].copy_from_slice(&[2, 0x50, 0x53, 0x50, 0, 1]);
        p.extend_from_slice(&MAGIC);
        p.extend_from_slice(options);
        p
    }

    #[test]
    fn discover_gets_offer() {
        let req = parse(&client_msg(&[53, 1, DISCOVER, 12, 3, b'p', b's', b'p', 255])).unwrap();
        assert_eq!(req.hostname.as_deref(), Some("psp"));
        let action = decide(&req, &LEASE);
        assert_eq!(action, Action::Reply(OFFER));
        let r = build_reply(&req, &action, &LEASE);
        assert!(r.len() >= 300);
        assert_eq!(r[0], 2);
        assert_eq!(&r[4..8], &[1, 2, 3, 4]);
        assert_eq!(&r[16..20], &[10, 77, 0, 2]);
        assert_eq!(&r[240..243], &[53, 1, OFFER]);
    }

    #[test]
    fn request_variants() {
        let ok = parse(&client_msg(&[53, 1, REQUEST, 50, 4, 10, 77, 0, 2, 54, 4, 10, 77, 0, 1, 255]));
        assert_eq!(decide(&ok.unwrap(), &LEASE), Action::Reply(ACK));
        let stale = parse(&client_msg(&[53, 1, REQUEST, 50, 4, 192, 168, 1, 7, 255]));
        assert_eq!(decide(&stale.unwrap(), &LEASE), Action::Reply(NAK));
        let other = parse(&client_msg(&[53, 1, REQUEST, 54, 4, 192, 168, 1, 1, 255]));
        assert_eq!(decide(&other.unwrap(), &LEASE), Action::Ignore);
        // No end marker, truncated last option: still understood.
        let sloppy = parse(&client_msg(&[53, 1, REQUEST, 0, 0, 50, 4, 10, 77]));
        assert_eq!(decide(&sloppy.unwrap(), &LEASE), Action::Reply(ACK));
    }
}
