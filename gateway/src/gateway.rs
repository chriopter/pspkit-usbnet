//! The gateway: the whole LAN behind the cable, in one thread.
//!
//! Frames from the PSP arrive on a channel (a second thread blocks in the
//! device's `recv`). ARP, DHCP, ICMP echo and UDP are answered or relayed by
//! hand. TCP is terminated by smoltcp: every new SYN first opens a real
//! `TcpStream` to the destination; once that is connected a smoltcp socket
//! listening on exactly that destination is created and the SYN is fed in.
//! After that bytes are copied between the smoltcp socket and the stream,
//! each direction only as fast as the other side takes them.
//!
//! Everything waits in one `mio::Poll`: host sockets, the frame channel's
//! waker and smoltcp's own timers. Nothing polls on a fixed interval.

use crate::device::{FrameDevice, RECV_BUF};
use crate::dhcp;
use crate::logln;
use crate::packet::{self, BROADCAST_MAC, ETH_HDR, Mac, mac_str};
use mio::net::{TcpStream, UdpSocket};
use mio::{Events, Interest, Poll, Token, Waker};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpListenEndpoint};
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{TryRecvError, sync_channel};
use std::time::{Duration, Instant};

mod tcp_relay;
mod udp_relay;
use tcp_relay::pump;

/// smoltcp buffer for bytes from the PSP that the host has not taken yet.
/// Deliberately just under 64 KiB: a larger buffer makes smoltcp use window
/// scaling, and its scaled window loses up to 7 bytes to rounding, so the
/// right edge it advertises can move back. A sender that has already used
/// the old edge then has a segment cut short, and smoltcp forgets a FIN that
/// arrives behind such a hole, which costs the sender a retransmission
/// timeout. The host takes bytes far faster than the PSP sends them, so the
/// size is no limit in practice.
const TCP_RX_BUF: usize = 65535;
/// smoltcp buffer for bytes from the host that the PSP has not acknowledged:
/// this is the download direction, kept large so the host socket is drained
/// in big reads and there is always a full window ready for the PSP.
const TCP_TX_BUF: usize = 512 * 1024;
const MAX_CONNS: usize = 128;
const MAX_FLOWS: usize = 256;
const MAX_PEERS: usize = 256;
/// In a flow's key, in place of the destination: any.
const ANY: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const TCP_KEEP_ALIVE: Duration = Duration::from_secs(60);
const TCP_TIMEOUT: Duration = Duration::from_secs(150);
const UDP_TIMEOUT: Duration = Duration::from_secs(60);
const DNS_TIMEOUT: Duration = Duration::from_secs(10);
const DNS_RETRY_OTHER: Duration = Duration::from_millis(1500);
const WAKE: Token = Token(0);

#[derive(Clone, Debug)]
pub struct Config {
    pub gateway_ip: Ipv4Addr,
    pub gateway_mac: Mac,
    /// The one address the DHCP server hands out.
    pub client_ip: Ipv4Addr,
    pub prefix_len: u8,
    pub lease_secs: u32,
    /// Where DNS queries go. None: the nameservers of /etc/resolv.conf,
    /// read again for each query, with 1.1.1.1 as the fallback.
    pub resolvers: Option<Vec<SocketAddr>>,
    /// Log a summary of every frame.
    pub verbose: bool,
    /// Print totals this often, when they changed.
    pub stats_interval: Option<Duration>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            gateway_ip: Ipv4Addr::new(10, 77, 0, 1),
            gateway_mac: [0x02, 0x50, 0x43, 0x00, 0x00, 0x01],
            client_ip: Ipv4Addr::new(10, 77, 0, 2),
            prefix_len: 24,
            lease_secs: 86400,
            resolvers: None,
            verbose: false,
            stats_interval: None,
        }
    }
}

impl Config {
    pub fn netmask(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::MAX.checked_shl(32 - u32::from(self.prefix_len)).unwrap_or(0))
    }

    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.gateway_ip) | !u32::from(self.netmask()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub rx_frames: u64,
    pub rx_bytes: u64,
    pub rx_ignored: u64,
    pub tx_frames: u64,
    pub tx_bytes: u64,
    pub tx_lost: u64,
    pub tcp_opened: u64,
    pub tcp_failed: u64,
    pub tcp_closed: u64,
    /// Bytes from the PSP to the host side.
    pub tcp_up: u64,
    /// Bytes from the host side to the PSP.
    pub tcp_down: u64,
    pub udp_out: u64,
    pub udp_in: u64,
    pub dns_queries: u64,
    pub dns_answers: u64,
    pub dhcp_leases: u64,
}

/// The nameservers of a resolv.conf.
pub fn parse_resolv_conf(text: &str) -> Vec<SocketAddr> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "nameserver" {
                return None;
            }
            let addr = words.next()?;
            let addr = addr.split('%').next()?;
            Some(SocketAddr::new(addr.parse::<IpAddr>().ok()?, 53))
        })
        .collect()
}

/// IP packets between the gateway and smoltcp; Ethernet is added and removed
/// by the gateway.
struct Phy {
    rx: VecDeque<Vec<u8>>,
    /// Frames with room for the Ethernet header in front.
    tx: Vec<Vec<u8>>,
}

struct PhyRx(Vec<u8>);
struct PhyTx<'a>(&'a mut Vec<Vec<u8>>);

impl RxToken for PhyRx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for PhyTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut frame = vec![0u8; ETH_HDR + len];
        let r = f(&mut frame[ETH_HDR..]);
        self.0.push(frame);
        r
    }
}

impl Device for Phy {
    type RxToken<'a> = PhyRx;
    type TxToken<'a> = PhyTx<'a>;

    fn receive(&mut self, _now: SmolInstant) -> Option<(PhyRx, PhyTx<'_>)> {
        let pkt = self.rx.pop_front()?;
        Some((PhyRx(pkt), PhyTx(&mut self.tx)))
    }

    fn transmit(&mut self, _now: SmolInstant) -> Option<PhyTx<'_>> {
        Some(PhyTx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = 1500;
        caps
    }
}

/// (the PSP's address and port, the destination it asked for)
type ConnKey = (SocketAddrV4, SocketAddrV4);

struct Conn {
    /// The host side. None once it is finished with.
    stream: Option<TcpStream>,
    token: Token,
    /// The smoltcp side. None while the host side is still connecting.
    handle: Option<SocketHandle>,
    /// The PSP's SYN (an IP packet), kept until the host side is connected.
    syn: Option<Vec<u8>>,
    started: Instant,
    host_readable: bool,
    host_writable: bool,
    /// The host sent EOF; our FIN to the PSP is queued.
    host_eof: bool,
    /// The PSP sent FIN and everything before it went to the host.
    host_shut: bool,
    up: u64,
    down: u64,
    reason: Option<String>,
}

/// A UDP socket on the host. For DNS: one per (PSP port, query), connected
/// to a resolver. For everything else: one per PSP port, whatever the
/// destination, and what anyone sends to it goes to the PSP, the way a home
/// router does it. Games that connect players directly count on that.
struct Flow {
    sock: UdpSocket,
    token: Token,
    /// The resolvers, for DNS.
    targets: Vec<SocketAddr>,
    target: usize,
    dns: bool,
    /// Host addresses the PSP has sent to, under the name it used for them
    /// (10.77.0.1 is this computer), so their answers come from that name.
    peers: HashMap<SocketAddr, SocketAddrV4>,
    last: Instant,
    /// Since when a DNS query has been waiting for its answer.
    waiting_since: Option<Instant>,
}

impl Flow {
    fn expires(&self) -> Instant {
        self.last + if self.dns { DNS_TIMEOUT } else { UDP_TIMEOUT }
    }
}

#[derive(Clone, Copy)]
enum Owner {
    Tcp(ConnKey),
    Udp(ConnKey),
}

pub struct Gateway {
    cfg: Config,
    dev: Arc<dyn FrameDevice>,
    poll: Poll,
    waker: Arc<Waker>,
    start: Instant,
    iface: Interface,
    sockets: SocketSet<'static>,
    phy: Phy,
    conns: HashMap<ConnKey, Conn>,
    flows: HashMap<ConnKey, Flow>,
    tokens: HashMap<Token, Owner>,
    next_token: usize,
    client_mac: Option<Mac>,
    stats: Stats,
    printed: Stats,
    last_print: Option<Instant>,
    tx_error: Option<io::ErrorKind>,
    scratch: Vec<u8>,
}

fn is_would_block(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::WouldBlock
}

impl Gateway {
    pub fn new(cfg: Config, dev: Arc<dyn FrameDevice>) -> io::Result<Gateway> {
        let poll = Poll::new()?;
        let waker = Arc::new(Waker::new(poll.registry(), WAKE)?);
        let start = Instant::now();
        let mut phy = Phy { rx: VecDeque::new(), tx: Vec::new() };
        let mut ifcfg = smoltcp::iface::Config::new(HardwareAddress::Ip);
        ifcfg.random_seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
            ^ (u64::from(std::process::id()) << 32);
        let mut iface = Interface::new(ifcfg, &mut phy, SmolInstant::from_micros(0));
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(cfg.gateway_ip), cfg.prefix_len));
        });
        // Accept packets for every destination, as a router does.
        iface.set_any_ip(true);
        let _ = iface.routes_mut().add_default_ipv4_route(cfg.gateway_ip);
        Ok(Gateway {
            cfg,
            dev,
            poll,
            waker,
            start,
            iface,
            sockets: SocketSet::new(Vec::new()),
            phy,
            conns: HashMap::new(),
            flows: HashMap::new(),
            tokens: HashMap::new(),
            next_token: 1,
            client_mac: None,
            stats: Stats::default(),
            printed: Stats::default(),
            last_print: None,
            tx_error: None,
            scratch: vec![0u8; 65536],
        })
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Serve until the device reports that it is gone for good (the USB link
    /// never does; the in-memory device does when its other end is dropped).
    pub fn run(&mut self) -> io::Result<()> {
        let (frame_tx, frame_rx) = sync_channel::<Vec<u8>>(1024);
        let pending = Arc::new(AtomicBool::new(false));
        {
            let dev = self.dev.clone();
            let waker = self.waker.clone();
            let pending = pending.clone();
            std::thread::Builder::new().name("frame-rx".into()).spawn(move || {
                let mut buf = vec![0u8; RECV_BUF];
                loop {
                    match dev.recv(&mut buf, None) {
                        Ok(n) => {
                            if frame_tx.send(buf[..n].to_vec()).is_err() {
                                return;
                            }
                            if !pending.swap(true, Ordering::SeqCst) {
                                let _ = waker.wake();
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
                        Err(_) => break,
                    }
                }
                drop(frame_tx);
                let _ = waker.wake();
            })?;
        }

        let mut events = Events::with_capacity(256);
        loop {
            pending.store(false, Ordering::SeqCst);
            let mut gone = false;
            loop {
                match frame_rx.try_recv() {
                    Ok(frame) => self.handle_frame(&frame),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        gone = true;
                        break;
                    }
                }
            }
            self.service();
            if gone {
                return Ok(());
            }
            let timeout = self.housekeeping();
            match self.poll.poll(&mut events, timeout) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            for ev in events.iter() {
                if ev.token() == WAKE {
                    continue;
                }
                match self.tokens.get(&ev.token()).copied() {
                    Some(Owner::Tcp(key)) => self.on_tcp_event(key, ev),
                    Some(Owner::Udp(key)) => self.on_udp_readable(key),
                    None => {}
                }
            }
        }
    }

    fn now(&self) -> SmolInstant {
        SmolInstant::from_micros(self.start.elapsed().as_micros() as i64)
    }

    fn new_token(&mut self, owner: Owner) -> Token {
        let t = Token(self.next_token);
        self.next_token += 1;
        self.tokens.insert(t, owner);
        t
    }

    // ---- frames out ----

    fn emit(&mut self, frame: &[u8]) {
        if self.cfg.verbose {
            logln!("> {:4} {}", frame.len(), packet::summarize(frame));
        }
        match self.dev.send(frame) {
            Ok(()) => {
                self.stats.tx_frames += 1;
                self.stats.tx_bytes += frame.len() as u64;
                if self.tx_error.take().is_some() {
                    logln!("link: sending to the PSP works again");
                }
            }
            Err(e) => {
                self.stats.tx_lost += 1;
                if self.tx_error != Some(e.kind()) {
                    logln!("link: cannot send to the PSP, frames are lost: {e}");
                    self.tx_error = Some(e.kind());
                }
            }
        }
    }

    fn flush_smoltcp_tx(&mut self) {
        if self.phy.tx.is_empty() {
            return;
        }
        let dst = self.client_mac.unwrap_or(BROADCAST_MAC);
        let src = self.cfg.gateway_mac;
        for mut frame in std::mem::take(&mut self.phy.tx) {
            packet::set_eth_header(&mut frame, &dst, &src, packet::ETHERTYPE_IPV4);
            self.emit(&frame);
        }
    }

    // ---- frames in ----

    fn handle_frame(&mut self, frame: &[u8]) {
        self.stats.rx_frames += 1;
        self.stats.rx_bytes += frame.len() as u64;
        if self.cfg.verbose {
            logln!("< {:4} {}", frame.len(), packet::summarize(frame));
        }
        if frame.len() < ETH_HDR {
            self.stats.rx_ignored += 1;
            return;
        }
        let src_mac: Mac = frame[6..12].try_into().unwrap();
        let handled = match u16::from_be_bytes([frame[12], frame[13]]) {
            packet::ETHERTYPE_ARP => self.handle_arp(frame),
            packet::ETHERTYPE_IPV4 => self.handle_ipv4(&frame[ETH_HDR..], src_mac),
            _ => false,
        };
        if !handled {
            self.stats.rx_ignored += 1;
        }
    }

    fn learn_mac(&mut self, mac: Mac) {
        // Unicast source addresses only.
        if mac[0] & 1 == 0 && self.client_mac != Some(mac) {
            self.client_mac = Some(mac);
        }
    }

    fn handle_arp(&mut self, frame: &[u8]) -> bool {
        let Some(arp) = packet::parse_arp(frame) else { return false };
        if arp.op != packet::ARP_REQUEST {
            return arp.op == packet::ARP_REPLY;
        }
        self.learn_mac(arp.sender_mac);
        // We are the only other node, so every address but the asker's own
        // is "ours". An address probe (sender 0.0.0.0) is only answered for
        // the gateway, or the client would see its own address as taken.
        let answer = if arp.sender_ip.is_unspecified() {
            arp.target_ip == self.cfg.gateway_ip
        } else {
            arp.target_ip != arp.sender_ip
        };
        if answer {
            let reply = packet::build_arp(
                &arp.sender_mac,
                &packet::Arp {
                    op: packet::ARP_REPLY,
                    sender_mac: self.cfg.gateway_mac,
                    sender_ip: arp.target_ip,
                    target_mac: arp.sender_mac,
                    target_ip: arp.sender_ip,
                },
            );
            self.emit(&reply);
        }
        true
    }

    fn handle_ipv4(&mut self, pkt: &[u8], src_mac: Mac) -> bool {
        let Some(ip) = packet::parse_ipv4(pkt) else { return false };
        if ip.fragment {
            return false;
        }
        self.learn_mac(src_mac);
        match ip.proto {
            packet::PROTO_TCP => self.handle_tcp(&ip),
            packet::PROTO_UDP => self.handle_udp(&ip),
            packet::PROTO_ICMP => self.handle_icmp(&ip, src_mac),
            _ => false,
        }
    }

    fn handle_icmp(&mut self, ip: &packet::Ipv4, src_mac: Mac) -> bool {
        let m = ip.payload;
        if m.len() < 8
            || m[0] != packet::ICMP_ECHO_REQUEST
            || ip.dst != self.cfg.gateway_ip
            || packet::checksum(&[m]) != 0
        {
            return false;
        }
        let reply = packet::build_icmp(
            &src_mac,
            &self.cfg.gateway_mac,
            self.cfg.gateway_ip,
            ip.src,
            packet::ICMP_ECHO_REPLY,
            &m[4..],
        );
        self.emit(&reply);
        true
    }

    fn is_broadcast_or_multicast(&self, ip: Ipv4Addr) -> bool {
        ip.is_broadcast() || ip.is_multicast() || ip.is_unspecified() || ip == self.cfg.broadcast()
    }

    /// Where a destination the PSP names really is: the gateway's own
    /// address stands for this computer.
    fn host_addr(&self, dst: SocketAddrV4) -> SocketAddr {
        if *dst.ip() == self.cfg.gateway_ip {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), dst.port())
        } else {
            dst.into()
        }
    }

    /// Let smoltcp work, send what it produced, move bytes between the two
    /// sides of every connection; repeat until nothing moves any more.
    fn service(&mut self) {
        loop {
            let now = self.now();
            self.iface.poll(now, &mut self.phy, &mut self.sockets);
            self.flush_smoltcp_tx();

            // Connections that are over, or whose SYN smoltcp did not take.
            // One that closed with bytes from the PSP still waiting for a
            // slow host stays until pump() has delivered them.
            let over: Vec<ConnKey> = self
                .conns
                .iter()
                .filter(|(_, c)| {
                    c.handle.is_some_and(|h| {
                        let s = self.sockets.get::<tcp::Socket>(h);
                        match s.state() {
                            tcp::State::Listen => true,
                            tcp::State::Closed => c.stream.is_none() || s.recv_queue() == 0,
                            _ => false,
                        }
                    })
                })
                .map(|(k, _)| *k)
                .collect();
            for key in over {
                self.remove_conn(&key);
            }

            let mut moved = false;
            for (key, conn) in self.conns.iter_mut() {
                moved |= pump(key, conn, &mut self.sockets, &mut self.stats);
            }
            if !moved {
                break;
            }
        }
    }

    /// Timeouts and statistics. Returns how long the main loop may sleep.
    fn housekeeping(&mut self) -> Option<Duration> {
        let now = Instant::now();

        let late: Vec<ConnKey> = self
            .conns
            .iter()
            .filter(|(_, c)| c.handle.is_none() && now >= c.started + CONNECT_TIMEOUT)
            .map(|(k, _)| *k)
            .collect();
        if !late.is_empty() {
            for key in late {
                self.connect_failed(key, "connect timed out");
            }
            self.service();
        }

        let idle: Vec<ConnKey> =
            self.flows.iter().filter(|(_, f)| now >= f.expires()).map(|(k, _)| *k).collect();
        for key in idle {
            self.remove_flow(&key);
        }

        if self.stats.dhcp_leases > 0 {
            let open = self.conns.values().filter(|c| c.stream.is_some()).count();
            crate::ui::traffic(self.stats.tcp_down, self.stats.tcp_up, open);
        }

        let mut next: Option<Instant> = None;
        let mut sooner = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));

        if let Some(interval) = self.cfg.stats_interval
            && self.stats != self.printed
        {
            match self.last_print {
                Some(last) if now < last + interval => sooner(last + interval),
                _ => {
                    self.print_stats();
                    self.printed = self.stats.clone();
                    self.last_print = Some(now);
                }
            }
        }
        for c in self.conns.values().filter(|c| c.handle.is_none()) {
            sooner(c.started + CONNECT_TIMEOUT);
        }
        for f in self.flows.values() {
            sooner(f.expires());
        }

        let mut timeout = next.map(|t| t.saturating_duration_since(Instant::now()));
        if let Some(d) = self.iface.poll_delay(self.now(), &self.sockets) {
            let d = Duration::from_micros(d.total_micros());
            timeout = Some(timeout.map_or(d, |t| t.min(d)));
        }
        timeout
    }

    fn print_stats(&self) {
        let s = &self.stats;
        let connecting = self.conns.values().filter(|c| c.handle.is_none()).count();
        logln!(
            "stats: from PSP {} frames {} B ({} ignored), to PSP {} frames {} B ({} lost); \
             tcp {} open, {} connecting, {} opened, {} failed, {} B up, {} B down; \
             udp {} out {} in, {} flows; dns {} queries {} answers; dhcp {} leases",
            s.rx_frames,
            s.rx_bytes,
            s.rx_ignored,
            s.tx_frames,
            s.tx_bytes,
            s.tx_lost,
            self.conns.values().filter(|c| c.handle.is_some() && c.stream.is_some()).count(),
            connecting,
            s.tcp_opened,
            s.tcp_failed,
            s.tcp_up,
            s.tcp_down,
            s.udp_out,
            s.udp_in,
            self.flows.len(),
            s.dns_queries,
            s.dns_answers,
            s.dhcp_leases
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolv_conf() {
        let text = "# comment\nsearch lan\nnameserver 127.0.0.53\nnameserver fe80::1%eth0\n\
                    nameserver bogus\noptions edns0\n";
        let r = parse_resolv_conf(text);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], "127.0.0.53:53".parse().unwrap());
        assert!(r[1].is_ipv6());
    }

    #[test]
    fn netmask_and_broadcast() {
        let c = Config::default();
        assert_eq!(c.netmask(), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(c.broadcast(), Ipv4Addr::new(10, 77, 0, 255));
    }
}
