//! Ethernet, ARP, IPv4, UDP and ICMP by hand: just what the gateway needs
//! outside of TCP (which smoltcp handles).

use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicU16, Ordering};

pub type Mac = [u8; 6];
pub const BROADCAST_MAC: Mac = [0xff; 6];
pub const ETH_HDR: usize = 14;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;
/// Largest UDP payload that fits one unfragmented frame.
pub const MAX_UDP_PAYLOAD: usize = 1500 - 20 - 8;

static IP_ID: AtomicU16 = AtomicU16::new(1);

pub fn mac_str(m: &[u8]) -> String {
    format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn ip_at(b: &[u8], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(b[at], b[at + 1], b[at + 2], b[at + 3])
}

/// Internet checksum over the concatenation of `parts` (every part but the
/// last must have an even length).
pub fn checksum(parts: &[&[u8]]) -> u16 {
    let mut sum: u32 = 0;
    for p in parts {
        let mut chunks = p.chunks_exact(2);
        for c in &mut chunks {
            sum += u32::from(u16::from_be_bytes([c[0], c[1]]));
        }
        if let [last] = chunks.remainder() {
            sum += u32::from(*last) << 8;
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn set_eth_header(frame: &mut [u8], dst: &Mac, src: &Mac, ethertype: u16) {
    frame[0..6].copy_from_slice(dst);
    frame[6..12].copy_from_slice(src);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
}

pub const ARP_REQUEST: u16 = 1;
pub const ARP_REPLY: u16 = 2;

pub struct Arp {
    pub op: u16,
    pub sender_mac: Mac,
    pub sender_ip: Ipv4Addr,
    pub target_mac: Mac,
    pub target_ip: Ipv4Addr,
}

/// Parse an Ethernet/IPv4 ARP packet from a whole frame.
pub fn parse_arp(frame: &[u8]) -> Option<Arp> {
    let a = frame.get(ETH_HDR..ETH_HDR + 28)?;
    if be16(a, 0) != 1 || be16(a, 2) != ETHERTYPE_IPV4 || a[4] != 6 || a[5] != 4 {
        return None;
    }
    Some(Arp {
        op: be16(a, 6),
        sender_mac: a[8..14].try_into().unwrap(),
        sender_ip: ip_at(a, 14),
        target_mac: a[18..24].try_into().unwrap(),
        target_ip: ip_at(a, 24),
    })
}

pub fn build_arp(eth_dst: &Mac, arp: &Arp) -> Vec<u8> {
    let mut f = vec![0u8; ETH_HDR + 28];
    set_eth_header(&mut f, eth_dst, &arp.sender_mac, ETHERTYPE_ARP);
    let a = &mut f[ETH_HDR..];
    a[0..6].copy_from_slice(&[0, 1, 8, 0, 6, 4]);
    a[6..8].copy_from_slice(&arp.op.to_be_bytes());
    a[8..14].copy_from_slice(&arp.sender_mac);
    a[14..18].copy_from_slice(&arp.sender_ip.octets());
    a[18..24].copy_from_slice(&arp.target_mac);
    a[24..28].copy_from_slice(&arp.target_ip.octets());
    f
}

/// A validated IPv4 header; `payload` is cut to the length the header states,
/// which removes Ethernet or USB padding.
pub struct Ipv4<'a> {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub proto: u8,
    pub fragment: bool,
    /// Header and payload, without padding.
    pub packet: &'a [u8],
    pub payload: &'a [u8],
}

pub fn parse_ipv4(pkt: &[u8]) -> Option<Ipv4<'_>> {
    if pkt.len() < 20 || pkt[0] >> 4 != 4 {
        return None;
    }
    let ihl = usize::from(pkt[0] & 0x0f) * 4;
    let total = usize::from(be16(pkt, 2));
    if ihl < 20 || total < ihl || total > pkt.len() {
        return None;
    }
    if checksum(&[&pkt[..ihl]]) != 0 {
        return None;
    }
    Some(Ipv4 {
        src: ip_at(pkt, 12),
        dst: ip_at(pkt, 16),
        proto: pkt[9],
        fragment: be16(pkt, 6) & 0x3fff != 0,
        packet: &pkt[..total],
        payload: &pkt[ihl..total],
    })
}

/// An Ethernet frame with an IPv4 header and room for `payload_len` bytes.
fn ipv4_frame(
    dst_mac: &Mac,
    src_mac: &Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    proto: u8,
    payload_len: usize,
) -> Vec<u8> {
    let mut f = vec![0u8; ETH_HDR + 20 + payload_len];
    set_eth_header(&mut f, dst_mac, src_mac, ETHERTYPE_IPV4);
    let ip = &mut f[ETH_HDR..ETH_HDR + 20];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&((20 + payload_len) as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&IP_ID.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    ip[8] = 64;
    ip[9] = proto;
    ip[12..16].copy_from_slice(&src.octets());
    ip[16..20].copy_from_slice(&dst.octets());
    let c = checksum(&[ip]);
    ip[10..12].copy_from_slice(&c.to_be_bytes());
    f
}

pub fn build_udp(
    dst_mac: &Mac,
    src_mac: &Mac,
    src: SocketAddrV4,
    dst: SocketAddrV4,
    payload: &[u8],
) -> Vec<u8> {
    let ulen = 8 + payload.len();
    let mut f = ipv4_frame(dst_mac, src_mac, *src.ip(), *dst.ip(), PROTO_UDP, ulen);
    let u = &mut f[ETH_HDR + 20..];
    u[0..2].copy_from_slice(&src.port().to_be_bytes());
    u[2..4].copy_from_slice(&dst.port().to_be_bytes());
    u[4..6].copy_from_slice(&(ulen as u16).to_be_bytes());
    u[8..].copy_from_slice(payload);
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&src.ip().octets());
    pseudo[4..8].copy_from_slice(&dst.ip().octets());
    pseudo[9] = PROTO_UDP;
    pseudo[10..12].copy_from_slice(&(ulen as u16).to_be_bytes());
    let c = match checksum(&[&pseudo, u]) {
        0 => 0xffff,
        c => c,
    };
    u[6..8].copy_from_slice(&c.to_be_bytes());
    f
}

pub struct Udp<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: &'a [u8],
}

pub fn parse_udp(seg: &[u8]) -> Option<Udp<'_>> {
    if seg.len() < 8 {
        return None;
    }
    let len = usize::from(be16(seg, 4));
    if len < 8 || len > seg.len() {
        return None;
    }
    Some(Udp { src_port: be16(seg, 0), dst_port: be16(seg, 2), payload: &seg[8..len] })
}

pub const ICMP_ECHO_REQUEST: u8 = 8;
pub const ICMP_ECHO_REPLY: u8 = 0;

/// An ICMP message: `kind` and code 0, then `rest` (for echo: id, sequence
/// number and data).
pub fn build_icmp(
    dst_mac: &Mac,
    src_mac: &Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    kind: u8,
    rest: &[u8],
) -> Vec<u8> {
    let mut f = ipv4_frame(dst_mac, src_mac, src, dst, PROTO_ICMP, 4 + rest.len());
    let m = &mut f[ETH_HDR + 20..];
    m[0] = kind;
    m[4..].copy_from_slice(rest);
    let c = checksum(&[m]);
    m[2..4].copy_from_slice(&c.to_be_bytes());
    f
}

pub fn build_echo(
    dst_mac: &Mac,
    src_mac: &Mac,
    src: Ipv4Addr,
    dst: Ipv4Addr,
    kind: u8,
    id: u16,
    seq: u16,
    data: &[u8],
) -> Vec<u8> {
    let mut rest = Vec::with_capacity(4 + data.len());
    rest.extend_from_slice(&id.to_be_bytes());
    rest.extend_from_slice(&seq.to_be_bytes());
    rest.extend_from_slice(data);
    build_icmp(dst_mac, src_mac, src, dst, kind, &rest)
}

/// One line describing a frame, for `--verbose`.
pub fn summarize(frame: &[u8]) -> String {
    let mut s = String::new();
    if frame.len() < ETH_HDR {
        let _ = write!(s, "runt frame");
        return s;
    }
    match be16(frame, 12) {
        ETHERTYPE_ARP => match parse_arp(frame) {
            Some(a) if a.op == ARP_REQUEST => {
                let _ = write!(s, "ARP who-has {} tell {}", a.target_ip, a.sender_ip);
            }
            Some(a) if a.op == ARP_REPLY => {
                let _ = write!(s, "ARP {} is-at {}", a.sender_ip, mac_str(&a.sender_mac));
            }
            _ => {
                let _ = write!(s, "ARP (unparsed)");
            }
        },
        ETHERTYPE_IPV4 => match parse_ipv4(&frame[ETH_HDR..]) {
            None => {
                let _ = write!(s, "IPv4 (bad header)");
            }
            Some(ip) if ip.fragment => {
                let _ = write!(s, "IPv4 fragment {} > {} proto {}", ip.src, ip.dst, ip.proto);
            }
            Some(ip) => match ip.proto {
                PROTO_TCP if ip.payload.len() >= 20 => {
                    let t = ip.payload;
                    let off = usize::from(t[12] >> 4) * 4;
                    let mut flags = String::new();
                    for (bit, ch) in [(0x02, 'S'), (0x01, 'F'), (0x04, 'R'), (0x08, 'P'), (0x10, '.')] {
                        if t[13] & bit != 0 {
                            flags.push(ch);
                        }
                    }
                    let _ = write!(
                        s,
                        "TCP {}:{} > {}:{} [{}] seq {} ack {} win {} len {}",
                        ip.src,
                        be16(t, 0),
                        ip.dst,
                        be16(t, 2),
                        flags,
                        u32::from_be_bytes([t[4], t[5], t[6], t[7]]),
                        u32::from_be_bytes([t[8], t[9], t[10], t[11]]),
                        be16(t, 14),
                        t.len().saturating_sub(off)
                    );
                }
                PROTO_UDP if ip.payload.len() >= 8 => {
                    let u = ip.payload;
                    let (sp, dp) = (be16(u, 0), be16(u, 2));
                    let tag = match (sp, dp) {
                        (67 | 68, _) | (_, 67 | 68) => " DHCP",
                        (53, _) | (_, 53) => " DNS",
                        _ => "",
                    };
                    let _ = write!(
                        s,
                        "UDP{} {}:{} > {}:{} len {}",
                        tag,
                        ip.src,
                        sp,
                        ip.dst,
                        dp,
                        u.len() - 8
                    );
                }
                PROTO_ICMP if ip.payload.len() >= 8 => {
                    let m = ip.payload;
                    let what = match m[0] {
                        ICMP_ECHO_REQUEST => "echo request".to_string(),
                        ICMP_ECHO_REPLY => "echo reply".to_string(),
                        t => format!("type {} code {}", t, m[1]),
                    };
                    let _ = write!(s, "ICMP {} > {} {}", ip.src, ip.dst, what);
                    if m[0] == ICMP_ECHO_REQUEST || m[0] == ICMP_ECHO_REPLY {
                        let _ = write!(s, " id {} seq {}", be16(m, 4), be16(m, 6));
                    }
                }
                p => {
                    let _ = write!(s, "IPv4 {} > {} proto {}", ip.src, ip.dst, p);
                }
            },
        },
        t => {
            let _ = write!(s, "ethertype 0x{:04x}", t);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_round_trip_and_checksums() {
        let src = SocketAddrV4::new(Ipv4Addr::new(10, 77, 0, 1), 53);
        let dst = SocketAddrV4::new(Ipv4Addr::new(10, 77, 0, 2), 4000);
        let f = build_udp(&[2; 6], &[4; 6], src, dst, b"hello");
        let ip = parse_ipv4(&f[ETH_HDR..]).expect("valid header checksum");
        assert_eq!((ip.src, ip.dst, ip.proto), (*src.ip(), *dst.ip(), PROTO_UDP));
        let u = parse_udp(ip.payload).unwrap();
        assert_eq!((u.src_port, u.dst_port, u.payload), (53, 4000, &b"hello"[..]));
        let mut pseudo = [0u8; 12];
        pseudo[0..4].copy_from_slice(&src.ip().octets());
        pseudo[4..8].copy_from_slice(&dst.ip().octets());
        pseudo[9] = PROTO_UDP;
        pseudo[11] = 13;
        assert_eq!(checksum(&[&pseudo, ip.payload]), 0);
    }

    #[test]
    fn padding_is_cut_off() {
        let mut f = build_echo(
            &[2; 6],
            &[4; 6],
            Ipv4Addr::new(10, 77, 0, 1),
            Ipv4Addr::new(10, 77, 0, 2),
            ICMP_ECHO_REQUEST,
            1,
            2,
            &[9; 8],
        );
        f.push(0);
        let ip = parse_ipv4(&f[ETH_HDR..]).unwrap();
        assert_eq!(ip.payload.len(), 16);
        assert_eq!(checksum(&[ip.payload]), 0);
    }
}
