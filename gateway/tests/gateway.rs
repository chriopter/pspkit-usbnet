//! The gateway, driven by a second smoltcp instance that plays the PSP over
//! the in-memory frame device. Needs nothing but localhost.

use pspkit_usbnetd::device::{FrameDevice, MemDevice, RECV_BUF};
use pspkit_usbnetd::gateway::{Config, Gateway};
use pspkit_usbnetd::packet;
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{dhcpv4, tcp, udp};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const PSP_MAC: [u8; 6] = [0x02, 0x50, 0x53, 0x50, 0x00, 0x01];
const GW_MAC: [u8; 6] = [0x02, 0x50, 0x43, 0x00, 0x00, 0x01];
const GW_IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 1);
const PSP_IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 2);
/// The PSP's receive window: like the real one, without window scaling.
const TCP_RX_BUF: usize = 65535;
const TCP_TX_BUF: usize = 256 * 1024;

struct Phy {
    rx: VecDeque<Vec<u8>>,
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
        let mut frame = vec![0u8; len];
        let r = f(&mut frame);
        self.0.push(frame);
        r
    }
}
impl Device for Phy {
    type RxToken<'a> = PhyRx;
    type TxToken<'a> = PhyTx<'a>;
    fn receive(&mut self, _: SmolInstant) -> Option<(PhyRx, PhyTx<'_>)> {
        let f = self.rx.pop_front()?;
        Some((PhyRx(f), PhyTx(&mut self.tx)))
    }
    fn transmit(&mut self, _: SmolInstant) -> Option<PhyTx<'_>> {
        Some(PhyTx(&mut self.tx))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

/// The PSP's end: an Ethernet interface without an address, until DHCP.
struct Psp {
    dev: MemDevice,
    gateway: JoinHandle<io::Result<()>>,
    iface: Interface,
    sockets: SocketSet<'static>,
    phy: Phy,
    start: Instant,
    next_port: u16,
}

impl Psp {
    fn new(cfg: Config) -> Psp {
        // Frames ending on a 512 byte boundary get a pad byte, as on USB.
        let (psp_end, gw_end) = MemDevice::pair(Some(512));
        let gateway = thread::spawn(move || Gateway::new(cfg, Arc::new(gw_end))?.run());
        let mut phy = Phy { rx: VecDeque::new(), tx: Vec::new() };
        let mut ifcfg =
            smoltcp::iface::Config::new(HardwareAddress::Ethernet(EthernetAddress(PSP_MAC)));
        ifcfg.random_seed = 0x5053_5031;
        let iface = Interface::new(ifcfg, &mut phy, SmolInstant::from_micros(0));
        Psp {
            dev: psp_end,
            gateway,
            iface,
            sockets: SocketSet::new(Vec::new()),
            phy,
            start: Instant::now(),
            next_port: 49152,
        }
    }

    /// A PSP that has its lease.
    fn booted(cfg: Config) -> Psp {
        let mut psp = Psp::new(cfg);
        psp.dhcp();
        psp
    }

    fn now(&self) -> SmolInstant {
        SmolInstant::from_micros(self.start.elapsed().as_micros() as i64)
    }

    fn poll(&mut self) {
        let now = self.now();
        self.iface.poll(now, &mut self.phy, &mut self.sockets);
        for f in self.phy.tx.drain(..) {
            self.dev.send(&f).expect("gateway is gone");
        }
    }

    /// One turn of the PSP's network stack: send what the application
    /// caused, wait at most `max_wait` for frames, take them in. The caller
    /// looks at its sockets after every turn, so nothing that is ready ever
    /// sits behind the wait.
    fn step(&mut self, max_wait: Duration) {
        self.poll();
        let wait = self
            .iface
            .poll_delay(self.now(), &self.sockets)
            .map_or(max_wait, |d| Duration::from_micros(d.total_micros()).min(max_wait));
        let mut buf = [0u8; RECV_BUF];
        let mut wait = Some(wait);
        // The first frame may be waited for; whatever else is there is taken along.
        while let Ok(n) = self.dev.recv(&mut buf, wait) {
            self.phy.rx.push_back(buf[..n].to_vec());
            wait = Some(Duration::ZERO);
        }
        self.poll();
    }

    fn until<T>(&mut self, what: &str, secs: u64, mut f: impl FnMut(&mut Psp) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(v) = f(self) {
                return v;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.step(Duration::from_millis(20));
        }
    }

    /// Send a frame by hand and return the next frame that comes back.
    fn raw(&mut self, frame: &[u8], wait: Duration) -> Option<Vec<u8>> {
        self.dev.send(frame).unwrap();
        let mut buf = [0u8; RECV_BUF];
        self.dev.recv(&mut buf, Some(wait)).ok().map(|n| buf[..n].to_vec())
    }

    fn dhcp(&mut self) -> (IpCidr, Option<Ipv4Addr>, Vec<Ipv4Addr>) {
        let h = self.sockets.add(dhcpv4::Socket::new());
        let (cidr, router, dns) = self.until("a DHCP lease", 10, |psp| {
            match psp.sockets.get_mut::<dhcpv4::Socket>(h).poll() {
                Some(dhcpv4::Event::Configured(c)) => {
                    Some((IpCidr::Ipv4(c.address), c.router, c.dns_servers.to_vec()))
                }
                _ => None,
            }
        });
        self.sockets.remove(h);
        self.iface.update_ip_addrs(|a| {
            a.clear();
            a.push(cidr).unwrap();
        });
        if let Some(r) = router {
            self.iface.routes_mut().add_default_ipv4_route(r).unwrap();
        }
        (cidr, router, dns)
    }

    fn tcp_connect(&mut self, dst: Ipv4Addr, port: u16) -> SocketHandle {
        let mut s = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; TCP_RX_BUF]),
            tcp::SocketBuffer::new(vec![0u8; TCP_TX_BUF]),
        );
        let local = self.next_port;
        self.next_port += 1;
        s.connect(self.iface.context(), (IpAddress::Ipv4(dst), port), local).unwrap();
        self.sockets.add(s)
    }

    fn tcp(&mut self, h: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut::<tcp::Socket>(h)
    }

    fn tcp_established(&mut self, h: SocketHandle) {
        self.until("the TCP handshake", 10, |psp| {
            let s = psp.tcp(h);
            assert!(s.is_open(), "connection refused");
            s.may_send().then_some(())
        });
    }

    fn tcp_write_all(&mut self, h: SocketHandle, data: &[u8]) {
        let mut off = 0;
        self.until("room to send", 30, |psp| {
            let s = psp.tcp(h);
            while off < data.len() && s.can_send() {
                off += s.send_slice(&data[off..]).unwrap();
            }
            (off == data.len()).then_some(())
        });
    }

    fn tcp_read_exact(&mut self, h: SocketHandle, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        self.until("data", 30, |psp| {
            let s = psp.tcp(h);
            while out.len() < len && s.can_recv() {
                let want = len - out.len();
                s.recv(|d| {
                    let n = d.len().min(want);
                    out.extend_from_slice(&d[..n]);
                    (n, ())
                })
                .unwrap();
            }
            (out.len() == len).then_some(())
        });
        out
    }

    /// Unplug: the gateway must notice and stop cleanly.
    fn unplug(self) {
        let Psp { dev, gateway, .. } = self;
        drop(dev);
        gateway.join().expect("gateway panicked").expect("gateway failed");
    }
}

fn fnv1a(hash: &mut u64, data: &[u8]) {
    for b in data {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}
const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// Deterministic noise.
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(len + 8);
    while v.len() < len {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.extend_from_slice(&seed.to_le_bytes());
    }
    v.truncate(len);
    v
}

fn arp_request(sender_ip: Ipv4Addr, target_ip: Ipv4Addr) -> Vec<u8> {
    packet::build_arp(
        &packet::BROADCAST_MAC,
        &packet::Arp {
            op: packet::ARP_REQUEST,
            sender_mac: PSP_MAC,
            sender_ip,
            target_mac: [0; 6],
            target_ip,
        },
    )
}

#[test]
fn dhcp_gives_the_lease() {
    let mut psp = Psp::new(Config::default());
    let (cidr, router, dns) = psp.dhcp();
    assert_eq!(cidr, IpCidr::new(IpAddress::Ipv4(PSP_IP), 24));
    assert_eq!(router, Some(GW_IP));
    assert_eq!(dns, vec![GW_IP]);
    psp.unplug();
}

#[test]
fn arp_and_ping_by_hand() {
    let mut psp = Psp::new(Config::default());
    let wait = Duration::from_secs(2);

    // The gateway's address resolves to the gateway's MAC.
    let reply = psp.raw(&arp_request(PSP_IP, GW_IP), wait).expect("ARP reply");
    let arp = packet::parse_arp(&reply).unwrap();
    assert_eq!(arp.op, packet::ARP_REPLY);
    assert_eq!((arp.sender_ip, arp.sender_mac), (GW_IP, GW_MAC));
    assert_eq!((arp.target_ip, arp.target_mac), (PSP_IP, PSP_MAC));
    assert_eq!(&reply[0..6], &PSP_MAC);

    // So does any other address: the gateway is the only other node.
    let other = Ipv4Addr::new(10, 77, 0, 99);
    let reply = psp.raw(&arp_request(PSP_IP, other), wait).expect("ARP reply");
    let arp = packet::parse_arp(&reply).unwrap();
    assert_eq!((arp.sender_ip, arp.sender_mac), (other, GW_MAC));

    // But not the client's own: neither a probe nor an announcement.
    let short = Duration::from_millis(300);
    assert!(psp.raw(&arp_request(Ipv4Addr::UNSPECIFIED, PSP_IP), short).is_none());
    assert!(psp.raw(&arp_request(PSP_IP, PSP_IP), short).is_none());

    // ICMP echo to the gateway, here with a frame of exactly 512 bytes, which
    // travels with a pad byte, as on USB.
    let data = noise(512 - 14 - 20 - 8, 7);
    let echo = packet::build_echo(
        &GW_MAC,
        &PSP_MAC,
        PSP_IP,
        GW_IP,
        packet::ICMP_ECHO_REQUEST,
        0x1234,
        9,
        &data,
    );
    assert_eq!(echo.len(), 512);
    let reply = psp.raw(&echo, wait).expect("echo reply");
    assert_eq!(reply.len(), 513, "a 512 byte frame is padded");
    let ip = packet::parse_ipv4(&reply[14..]).unwrap();
    assert_eq!((ip.src, ip.dst, ip.proto), (GW_IP, PSP_IP, packet::PROTO_ICMP));
    assert_eq!(ip.payload[0], packet::ICMP_ECHO_REPLY);
    assert_eq!(packet::checksum(&[ip.payload]), 0);
    assert_eq!(&ip.payload[4..8], &[0x12, 0x34, 0, 9]);
    assert_eq!(&ip.payload[8..], &data[..]);
    psp.unplug();
}

/// A server that echoes one connection and reports how many bytes it saw.
fn echo_server() -> (u16, JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let t = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0u8; 16384];
        let mut total = 0;
        loop {
            match s.read(&mut buf).unwrap() {
                0 => return total,
                n => {
                    s.write_all(&buf[..n]).unwrap();
                    total += n;
                }
            }
        }
    });
    (port, t)
}

fn echo_through(psp: &mut Psp, dst: Ipv4Addr) {
    let (port, server) = echo_server();
    let h = psp.tcp_connect(dst, port);
    psp.tcp_established(h);
    psp.tcp_write_all(h, b"hello from the PSP");
    assert_eq!(psp.tcp_read_exact(h, 18), b"hello from the PSP");
    // More than any buffer on the way holds at once.
    let big = noise(700_000, 42);
    let mut back = Vec::new();
    for chunk in big.chunks(100_000) {
        psp.tcp_write_all(h, chunk);
        back.extend(psp.tcp_read_exact(h, chunk.len()));
    }
    assert!(back == big, "echoed data differs");
    // Our FIN reaches the server, its FIN comes back, the socket closes.
    psp.tcp(h).close();
    psp.until("the close", 10, |psp| (!psp.tcp(h).is_active()).then_some(()));
    assert_eq!(server.join().unwrap(), 18 + big.len());
    psp.sockets.remove(h);
}

#[test]
fn tcp_echo() {
    let mut psp = Psp::booted(Config::default());
    // 127.0.0.1 named directly: any destination is accepted and relayed.
    echo_through(&mut psp, Ipv4Addr::LOCALHOST);
    // The gateway's own address stands for the host.
    echo_through(&mut psp, GW_IP);
    psp.unplug();
}

#[test]
fn tcp_refused_is_a_reset() {
    let mut psp = Psp::booted(Config::default());
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let t0 = Instant::now();
    let h = psp.tcp_connect(Ipv4Addr::LOCALHOST, port);
    psp.until("the reset", 5, |psp| (psp.tcp(h).state() == tcp::State::Closed).then_some(()));
    assert!(t0.elapsed() < Duration::from_secs(2), "refusal took {:?}", t0.elapsed());
    psp.unplug();
}

#[test]
fn tcp_download_20mb() {
    const LEN: usize = 20 * 1024 * 1024;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let data = noise(LEN, 0x1234_5678_9abc_def1);
        let mut hash = FNV_START;
        fnv1a(&mut hash, &data);
        s.write_all(&data).unwrap();
        hash
    });

    let mut psp = Psp::booted(Config::default());
    let h = psp.tcp_connect(Ipv4Addr::LOCALHOST, port);
    psp.tcp_established(h);
    let t0 = Instant::now();
    let mut hash = FNV_START;
    let mut got = 0usize;
    let deadline = t0 + Duration::from_secs(120);
    loop {
        let s = psp.tcp(h);
        while s.can_recv() {
            got += s
                .recv(|d| {
                    fnv1a(&mut hash, d);
                    (d.len(), d.len())
                })
                .unwrap();
        }
        if !s.may_recv() {
            break; // the server's FIN, after all its data
        }
        assert!(Instant::now() < deadline, "download stalled at {got} bytes");
        psp.step(Duration::from_millis(50));
    }
    let secs = t0.elapsed().as_secs_f64();
    assert_eq!(got, LEN, "wrong length");
    assert_eq!(hash, server.join().unwrap(), "data differs");
    println!(
        "download: {} MB in {:.2} s = {:.0} MB/s",
        LEN / (1024 * 1024),
        secs,
        LEN as f64 / (1024.0 * 1024.0) / secs
    );
    psp.unplug();
}

#[test]
fn tcp_upload_8mb() {
    const LEN: usize = 8 * 1024 * 1024;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut hash = FNV_START;
        let mut total = 0;
        let mut buf = vec![0u8; 65536];
        loop {
            match s.read(&mut buf).unwrap() {
                0 => return (total, hash),
                n => {
                    fnv1a(&mut hash, &buf[..n]);
                    total += n;
                    if total < 1024 * 1024 {
                        // A slow reader at first: the PSP must be held back.
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            }
        }
    });

    let mut psp = Psp::booted(Config::default());
    let h = psp.tcp_connect(GW_IP, port);
    psp.tcp_established(h);
    let data = noise(LEN, 99);
    let mut hash = FNV_START;
    fnv1a(&mut hash, &data);
    let t0 = Instant::now();
    psp.tcp_write_all(h, &data);
    psp.tcp(h).close();
    psp.until("the close", 30, |psp| (!psp.tcp(h).is_active()).then_some(()));
    let secs = t0.elapsed().as_secs_f64();
    assert_eq!(server.join().unwrap(), (LEN, hash));
    println!(
        "upload: {} MB in {:.2} s = {:.0} MB/s",
        LEN / (1024 * 1024),
        secs,
        LEN as f64 / (1024.0 * 1024.0) / secs
    );
    psp.unplug();
}

fn udp_socket(psp: &mut Psp, port: u16) -> SocketHandle {
    let buffer = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 8], vec![0u8; 8192]);
    let mut s = udp::Socket::new(buffer(), buffer());
    s.bind(port).unwrap();
    psp.sockets.add(s)
}

fn udp_exchange(psp: &mut Psp, h: SocketHandle, dst: Ipv4Addr, port: u16, data: &[u8]) -> Vec<u8> {
    let to = IpEndpoint::new(IpAddress::Ipv4(dst), port);
    psp.sockets.get_mut::<udp::Socket>(h).send_slice(data, to).unwrap();
    psp.until("a UDP answer", 5, |psp| {
        let s = psp.sockets.get_mut::<udp::Socket>(h);
        let (answer, meta) = s.recv().ok()?;
        // The answer comes from the address the PSP sent to.
        assert_eq!(meta.endpoint, to);
        Some(answer.to_vec())
    })
}

#[test]
fn dns_is_forwarded() {
    // A fake resolver: answers with the query, the response bit set and four
    // bytes of "address" appended.
    let resolver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = resolver.local_addr().unwrap();
    thread::spawn(move || {
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = resolver.recv_from(&mut buf) {
            let mut answer = buf[..n].to_vec();
            answer[2] |= 0x80;
            answer.extend_from_slice(&[93, 184, 216, 34]);
            resolver.send_to(&answer, from).unwrap();
        }
    });
    let cfg = Config { resolvers: Some(vec![addr]), ..Config::default() };
    let mut psp = Psp::booted(cfg);
    let h = udp_socket(&mut psp, 4000);
    for id in [0x11u8, 0x22, 0x33] {
        let mut query = vec![id, id, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        query.extend_from_slice(b"\x07example\x03com\x00\x00\x01\x00\x01");
        let answer = udp_exchange(&mut psp, h, GW_IP, 53, &query);
        let mut expect = query.clone();
        expect[2] |= 0x80;
        expect.extend_from_slice(&[93, 184, 216, 34]);
        assert_eq!(answer, expect);
    }
    psp.unplug();
}

#[test]
fn udp_is_relayed() {
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = server.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = server.recv_from(&mut buf) {
            buf[..n].reverse();
            server.send_to(&buf[..n], from).unwrap();
        }
    });
    let mut psp = Psp::booted(Config::default());
    let h = udp_socket(&mut psp, 4001);
    assert_eq!(udp_exchange(&mut psp, h, GW_IP, port, b"abc"), b"cba");
    assert_eq!(udp_exchange(&mut psp, h, GW_IP, port, b"12345"), b"54321");
    // The largest datagram that fits a frame.
    let big = noise(packet::MAX_UDP_PAYLOAD, 5);
    let mut rev = big.clone();
    rev.reverse();
    assert_eq!(udp_exchange(&mut psp, h, GW_IP, port, &big), rev);
    psp.unplug();
}

/// One port on the host per port of the PSP, whatever the destination, and
/// anyone may send to it: what games need whose players connect directly.
#[test]
fn udp_port_is_the_same_for_all_and_open_to_all() {
    let a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let b = UdpSocket::bind("127.0.0.1:0").unwrap();
    let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
    for s in [&a, &b] {
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    }
    let mut psp = Psp::booted(Config::default());
    let h = udp_socket(&mut psp, 4002);
    let mut seen = Vec::new();
    for server in [&a, &b] {
        let to = IpEndpoint::new(IpAddress::Ipv4(GW_IP), server.local_addr().unwrap().port());
        psp.sockets.get_mut::<udp::Socket>(h).send_slice(b"hello", to).unwrap();
        psp.until("the datagram to be sent", 5, |psp| {
            psp.sockets.get_mut::<udp::Socket>(h).can_send().then_some(())
        });
        let mut buf = [0u8; 16];
        let mut got = None;
        psp.until("the datagram at the server", 5, |_| {
            server.set_nonblocking(true).unwrap();
            got = server.recv_from(&mut buf).ok();
            got.map(|_| ())
        });
        seen.push(got.unwrap().1);
    }
    assert_eq!(seen[0], seen[1], "both servers see the same port of the gateway");

    // Someone the PSP never sent to reaches it there, under its own address.
    stranger.send_to(b"knock", seen[0]).unwrap();
    let from = psp.until("the stranger's datagram", 5, |psp| {
        let (data, meta) = psp.sockets.get_mut::<udp::Socket>(h).recv().ok()?;
        assert_eq!(data, b"knock");
        Some(meta.endpoint)
    });
    assert_eq!(from, IpEndpoint::new(IpAddress::Ipv4(GW_IP), stranger.local_addr().unwrap().port()));
    psp.unplug();
}
