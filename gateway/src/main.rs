//! pspkit-usbnetd: see `--help`.

use pspkit_usbnetd::device::{FrameDevice, RECV_BUF};
use pspkit_usbnetd::gateway::{Config, Gateway};
use pspkit_usbnetd::log;
use pspkit_usbnetd::logln;
use pspkit_usbnetd::ui;
use pspkit_usbnetd::packet::{self, BROADCAST_MAC, ETH_HDR, Mac, mac_str};
use pspkit_usbnetd::usb::{Bus, LINKS, OpenError, UsbConn, UsbLink};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// Bound on 127.0.0.1 by the one gateway that runs.
const ONLY_ONE_PORT: u16 = 10770;

const HELP: &str = "\
pspkit-usbnetd - the PC side of Ethernet over the PSP's USB cable

A user-mode gateway for the PSP's usbnet driver (USB 054c:01c9, the interface
of class 0xfd). It is the whole LAN behind the cable: it answers ARP, is the
DHCP server (the PSP gets 10.77.0.2/24, router and DNS 10.77.0.1), forwards
DNS to this computer's resolver and relays the PSP's TCP and UDP through
ordinary sockets. No TAP/TUN device, no root. 10.77.0.1 itself stands for
this computer: the PSP reaches local services there (as 127.0.0.1).

Up to 4 PSPs are served at once, each on its own cable. Each cable is a LAN
of its own with these same addresses; the PSPs do not see each other. The
log names them by model (\"PSP Go\"; the second of a kind \"PSP Go (2)\"), or
\"PSP 1\", \"PSP 2\" where the plugin is too old to tell.

Usage: pspkit-usbnetd [OPTIONS]

Runs in the foreground until killed and shows how far the connection is.
It waits for the PSP and finds it again after every power cycle.

Options:
  -v, --verbose   the event log instead of the steps; twice: every frame too
      --dns IP    the nameserver the PSP's DNS queries go to, instead of
                  this computer's (/etc/resolv.conf, else 1.1.1.1); may be
                  given more than once
      --stats     print totals every 10 s, when they changed
      --ping      test mode: ARP for 10.77.0.2, then 5 ICMP echo requests
                  from 10.77.0.1, print the round-trip times and exit (the
                  first PSP, if there are several)
  -h, --help      this text
  -V, --version   the version

Exit status:
  0   --ping: every echo was answered (also --help, --version)
  1   another pspkit-usbnetd is already running; the gateway stopped on an
      internal error; --ping: no ARP answer, or an echo went unanswered
  2   wrong command line
  3   --ping: no PSP on USB, no usbnet interface, or it cannot be claimed
  The gateway itself never exits on its own; it ends by signal (SIGINT,
  SIGTERM), with the shell's usual 130 or 143.
";

fn main() -> ExitCode {
    let mut cfg = Config::default();
    let mut ping = false;
    let mut verbose = 0;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dns" => match args.next().and_then(|a| a.parse::<IpAddr>().ok()) {
                Some(ip) => cfg.resolvers.get_or_insert_with(Vec::new).push(SocketAddr::new(ip, 53)),
                None => {
                    eprintln!("pspkit-usbnetd: --dns needs an IP address");
                    return ExitCode::from(2);
                }
            },
            "-v" | "--verbose" => verbose += 1,
            "--stats" => cfg.stats_interval = Some(Duration::from_secs(10)),
            "--ping" => ping = true,
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("pspkit-usbnetd {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("pspkit-usbnetd: unknown argument '{other}' (see --help)");
                return ExitCode::from(2);
            }
        }
    }

    cfg.verbose = verbose > 1;
    // One gateway per computer: two would take the PSP from each other. The
    // mark is a UDP port on this computer, held as long as this one runs.
    let _only_one = match UdpSocket::bind((Ipv4Addr::LOCALHOST, ONLY_ONE_PORT)) {
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            eprintln!("pspkit-usbnetd: another pspkit-usbnetd is already running on this computer; end that one first");
            return ExitCode::from(1);
        }
        other => other.ok(),
    };
    if ping {
        return ping_test(&cfg);
    }

    // The screen of steps, or with -v the event log alone.
    let mut bus = None;
    if verbose == 0 {
        cfg.status = ui::start(env!("CARGO_PKG_VERSION"), || bus.insert(Bus::new()).usable());
    }
    let bus = Arc::new(bus.unwrap_or_default());
    logln!(
        "pspkit-usbnetd {}: gateway {}/{} ({}), client {}",
        env!("CARGO_PKG_VERSION"),
        cfg.gateway_ip,
        cfg.prefix_len,
        mac_str(&cfg.gateway_mac),
        cfg.client_ip
    );
    // A gateway of its own for every PSP there may be: each cable is its
    // own LAN, so they share nothing but the bus.
    let (ended, first_end) = channel();
    for link in 0..LINKS {
        let mut cfg = cfg.clone();
        cfg.status = cfg.status.of(link);
        let link = UsbLink::new(bus.clone(), cfg.status.clone());
        let ended = ended.clone();
        std::thread::spawn(move || {
            log::set_label(Some(link.label()));
            let result = Gateway::new(cfg, Arc::new(link)).and_then(|mut gw| gw.run());
            let _ = ended.send(result);
        });
    }
    // None of them ends unless something is wrong; then all do.
    match first_end.recv() {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            eprintln!("pspkit-usbnetd: stopped: {e}");
            ExitCode::from(1)
        }
        Err(_) => ExitCode::from(1),
    }
}

/// Read frames until `want` accepts one or the time is up.
fn wait_for<T>(
    dev: &UsbConn,
    verbose: bool,
    timeout: Duration,
    mut want: impl FnMut(&[u8]) -> Option<T>,
) -> io::Result<Option<T>> {
    let deadline = Instant::now() + timeout;
    let mut buf = vec![0u8; RECV_BUF];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        match dev.recv(&mut buf, Some(left)) {
            Ok(n) => {
                if verbose {
                    logln!("< {:4} {}", n, packet::summarize(&buf[..n]));
                }
                if let Some(v) = want(&buf[..n]) {
                    return Ok(Some(v));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::TimedOut => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

fn ping_test(cfg: &Config) -> ExitCode {
    const COUNT: u16 = 5;
    const TIMEOUT: Duration = Duration::from_secs(1);
    let target: Ipv4Addr = cfg.client_ip;
    let dev = match UsbConn::open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("pspkit-usbnetd: {e}");
            if let OpenError::Failed(_) = e {
                eprintln!("(is another program using the interface, such as a running pspkit-usbnetd?)");
            }
            return ExitCode::from(3);
        }
    };
    println!("usbnet: {}", dev.describe());
    let send = |frame: &[u8]| {
        if cfg.verbose {
            logln!("> {:4} {}", frame.len(), packet::summarize(frame));
        }
        dev.send(frame)
    };
    let run = || -> io::Result<bool> {
        let request = packet::build_arp(
            &BROADCAST_MAC,
            &packet::Arp {
                op: packet::ARP_REQUEST,
                sender_mac: cfg.gateway_mac,
                sender_ip: cfg.gateway_ip,
                target_mac: [0; 6],
                target_ip: target,
            },
        );
        let t0 = Instant::now();
        send(&request)?;
        let mac: Option<Mac> = wait_for(&dev, cfg.verbose, TIMEOUT, |f| {
            packet::parse_arp(f)
                .filter(|a| a.op == packet::ARP_REPLY && a.sender_ip == target)
                .map(|a| a.sender_mac)
        })?;
        let Some(mac) = mac else {
            println!("ARP: no answer for {target}");
            return Ok(false);
        };
        println!(
            "ARP: {target} is at {} ({:.2} ms)",
            mac_str(&mac),
            t0.elapsed().as_secs_f64() * 1000.0
        );

        let id = std::process::id() as u16;
        let data: Vec<u8> = (0..56u8).map(|i| i.wrapping_mul(7)).collect();
        let mut answered = 0;
        let mut total = 0.0;
        for seq in 1..=COUNT {
            let echo = packet::build_echo(
                &mac,
                &cfg.gateway_mac,
                cfg.gateway_ip,
                target,
                packet::ICMP_ECHO_REQUEST,
                id,
                seq,
                &data,
            );
            let t0 = Instant::now();
            send(&echo)?;
            let ok = wait_for(&dev, cfg.verbose, TIMEOUT, |f| {
                if f.len() < ETH_HDR || f[12..14] != [0x08, 0x00] {
                    return None;
                }
                let ip = packet::parse_ipv4(&f[ETH_HDR..])?;
                let m = ip.payload;
                let ours = ip.proto == packet::PROTO_ICMP
                    && ip.src == target
                    && m.len() >= 8
                    && m[0] == packet::ICMP_ECHO_REPLY
                    && m[4..6] == id.to_be_bytes()
                    && m[6..8] == seq.to_be_bytes();
                // The reply must be intact: checksum and payload.
                ours.then(|| packet::checksum(&[m]) == 0 && m[8..] == data[..])
            })?;
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            match ok {
                Some(true) => {
                    answered += 1;
                    total += ms;
                    println!("{} bytes from {target}: seq={seq} time={ms:.2} ms", data.len() + 8);
                }
                Some(false) => println!("from {target}: seq={seq} answer is damaged"),
                None => println!("from {target}: seq={seq} no answer within {} s", TIMEOUT.as_secs()),
            }
        }
        println!(
            "{answered} of {COUNT} echo requests answered{}",
            if answered > 0 { format!(", mean {:.2} ms", total / f64::from(answered)) } else { String::new() }
        );
        Ok(answered == COUNT)
    };
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("pspkit-usbnetd: USB error: {e}");
            ExitCode::from(1)
        }
    }
}
