//! TCP: smoltcp ends the PSP's connection, a host socket carries it on.

use super::*;

impl Gateway {
    pub(super) fn handle_tcp(&mut self, ip: &packet::Ipv4) -> bool {
        let t = ip.payload;
        if t.len() < 20 {
            return false;
        }
        let client = SocketAddrV4::new(ip.src, u16::from_be_bytes([t[0], t[1]]));
        let dst = SocketAddrV4::new(ip.dst, u16::from_be_bytes([t[2], t[3]]));
        let key = (client, dst);
        let syn_only = t[13] & 0x17 == 0x02; // SYN without ACK, FIN, RST
        if syn_only {
            let mut is_new = true;
            if let Some(conn) = self.conns.get_mut(&key) {
                match conn.handle {
                    None => {
                        // Still connecting on the host side; keep the latest SYN.
                        conn.syn = Some(ip.packet.to_vec());
                        return true;
                    }
                    Some(h) => {
                        let state = self.sockets.get::<tcp::Socket>(h).state();
                        if matches!(state, tcp::State::TimeWait | tcp::State::Closed) {
                            // The same port pair again: the old one is over.
                            self.remove_conn(&key);
                        } else {
                            is_new = false; // a retransmitted SYN, smoltcp's business
                        }
                    }
                }
            }
            if is_new {
                if self.is_broadcast_or_multicast(ip.dst) || dst.port() == 0 || client.port() == 0 {
                    return false;
                }
                if self.conns.len() >= MAX_CONNS {
                    logln!("tcp: {client} -> {dst} refused: {MAX_CONNS} connections are open");
                    self.stats.tcp_failed += 1;
                } else {
                    match self.start_connect(key, ip.packet) {
                        Ok(()) => return true,
                        Err(e) => {
                            logln!("tcp: {client} -> {dst} failed: {e}");
                            self.stats.tcp_failed += 1;
                        }
                    }
                }
                // Falls through: without a listening socket smoltcp answers
                // the SYN with a reset.
            }
        }
        self.phy.rx.push_back(ip.packet.to_vec());
        true
    }

    pub(super) fn start_connect(&mut self, key: ConnKey, syn: &[u8]) -> io::Result<()> {
        let remote = self.host_addr(key.1);
        let mut stream = TcpStream::connect(remote)?;
        let _ = stream.set_nodelay(true);
        let token = self.new_token(Owner::Tcp(key));
        if let Err(e) = self.poll.registry().register(
            &mut stream,
            token,
            Interest::READABLE | Interest::WRITABLE,
        ) {
            self.tokens.remove(&token);
            return Err(e);
        }
        self.conns.insert(
            key,
            Conn {
                stream: Some(stream),
                token,
                handle: None,
                syn: Some(syn.to_vec()),
                started: Instant::now(),
                host_readable: false,
                host_writable: false,
                host_eof: false,
                host_shut: false,
                up: 0,
                down: 0,
                reason: None,
            },
        );
        Ok(())
    }

    pub(super) fn on_tcp_event(&mut self, key: ConnKey, ev: &mio::event::Event) {
        let Some(conn) = self.conns.get_mut(&key) else { return };
        let Some(stream) = conn.stream.as_ref() else { return };
        if conn.handle.is_some() {
            if ev.is_readable() || ev.is_read_closed() || ev.is_error() {
                conn.host_readable = true;
            }
            if ev.is_writable() || ev.is_write_closed() || ev.is_error() {
                conn.host_writable = true;
            }
            return;
        }
        // Connecting: has the connect finished, and how?
        let outcome = match stream.take_error() {
            Ok(Some(e)) | Err(e) => Some(Err(e)),
            Ok(None) => match stream.peer_addr() {
                Ok(_) => Some(Ok(())),
                Err(e)
                    if e.kind() == io::ErrorKind::NotConnected
                        || e.raw_os_error() == Some(libc::EINPROGRESS) =>
                {
                    None
                }
                Err(e) => Some(Err(e)),
            },
        };
        match outcome {
            None => {}
            Some(Ok(())) => self.established(key),
            Some(Err(e)) => self.connect_failed(key, &e.to_string()),
        }
    }

    /// The host side is connected: now let the PSP's handshake proceed.
    pub(super) fn established(&mut self, key: ConnKey) {
        let Some(conn) = self.conns.get_mut(&key) else { return };
        let (client, dst) = key;
        let mut sock = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; TCP_RX_BUF]),
            tcp::SocketBuffer::new(vec![0u8; TCP_TX_BUF]),
        );
        sock.set_nagle_enabled(false);
        sock.set_keep_alive(Some(TCP_KEEP_ALIVE.into()));
        sock.set_timeout(Some(TCP_TIMEOUT.into()));
        let endpoint =
            IpListenEndpoint { addr: Some(IpAddress::Ipv4(*dst.ip())), port: dst.port() };
        if let Err(e) = sock.listen(endpoint) {
            self.connect_failed(key, &format!("{e:?}"));
            return;
        }
        conn.handle = Some(self.sockets.add(sock));
        conn.host_readable = true;
        conn.host_writable = true;
        if let Some(syn) = conn.syn.take() {
            self.phy.rx.push_back(syn);
        }
        self.stats.tcp_opened += 1;
        logln!(
            "tcp: open {client} -> {dst} (connected in {} ms)",
            conn.started.elapsed().as_millis()
        );
        conn.started = Instant::now();
    }

    /// The host side could not connect: have the PSP's SYN answered by a reset.
    pub(super) fn connect_failed(&mut self, key: ConnKey, why: &str) {
        let Some(conn) = self.conns.remove(&key) else { return };
        self.tokens.remove(&conn.token);
        self.stats.tcp_failed += 1;
        logln!("tcp: {} -> {} failed: {why}", key.0, key.1);
        if let Some(syn) = conn.syn {
            self.phy.rx.push_back(syn);
        }
    }

    pub(super) fn abort_all(&mut self, why: &str) {
        let flows: Vec<ConnKey> = self.flows.keys().copied().collect();
        for key in flows {
            self.remove_flow(&key);
        }
        let keys: Vec<ConnKey> = self.conns.keys().copied().collect();
        for key in keys {
            let conn = self.conns.get_mut(&key).unwrap();
            match conn.handle {
                Some(h) => {
                    conn.reason = Some(why.to_string());
                    self.sockets.get_mut::<tcp::Socket>(h).abort();
                }
                None => {
                    let token = conn.token;
                    self.conns.remove(&key);
                    self.tokens.remove(&token);
                }
            }
        }
    }

    pub(super) fn remove_conn(&mut self, key: &ConnKey) {
        let Some(mut conn) = self.conns.remove(key) else { return };
        self.tokens.remove(&conn.token);
        if let Some(h) = conn.handle {
            self.sockets.remove(h);
        }
        if conn.stream.is_some() {
            if conn.reason.is_none() && !(conn.host_eof && conn.host_shut) {
                conn.reason = Some("reset by the PSP or timed out".into());
            }
            finish_host(key, &mut conn, &mut self.stats);
        }
    }
}

/// The host side of a connection is finished with: close it and say so.
pub(super) fn finish_host(key: &ConnKey, conn: &mut Conn, stats: &mut Stats) {
    if conn.stream.take().is_none() {
        return;
    }
    stats.tcp_closed += 1;
    logln!(
        "tcp: close {} -> {}: {} B up, {} B down, {:.1} s{}",
        key.0,
        key.1,
        conn.up,
        conn.down,
        conn.started.elapsed().as_secs_f64(),
        conn.reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
    );
}

pub(super) enum Step {
    Moved(usize),
    Eof,
    Blocked,
    Retry,
    NoRoom,
    Failed(io::Error),
}

pub(super) fn step(r: io::Result<usize>, room: usize) -> Step {
    match r {
        Ok(0) if room == 0 => Step::NoRoom,
        Ok(0) => Step::Eof,
        Ok(n) => Step::Moved(n),
        Err(e) if is_would_block(&e) => Step::Blocked,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Step::Retry,
        Err(e) => Step::Failed(e),
    }
}

/// Copy what can be copied in both directions without blocking. Backpressure
/// is the buffers themselves: nothing is read from the host while smoltcp's
/// send buffer is full (the PSP is slow), nothing is taken from smoltcp's
/// receive buffer while the host does not accept it (so the PSP's window
/// closes). Returns whether anything changed that smoltcp should know about.
pub(super) fn pump(key: &ConnKey, conn: &mut Conn, sockets: &mut SocketSet<'static>, stats: &mut Stats) -> bool {
    let (Some(handle), Some(stream)) = (conn.handle, conn.stream.as_mut()) else { return false };
    let sock = sockets.get_mut::<tcp::Socket>(handle);
    let mut moved = false;
    let mut failure: Option<io::Error> = None;

    // PSP -> host
    while conn.host_writable && sock.can_recv() {
        let r = sock.recv(|data| match step(stream.write(data), data.len()) {
            Step::Moved(n) => (n, Step::Moved(n)),
            other => (0, other),
        });
        match r {
            Ok(Step::Moved(n)) => {
                conn.up += n as u64;
                stats.tcp_up += n as u64;
                moved = true;
            }
            Ok(Step::Blocked) => conn.host_writable = false,
            Ok(Step::Retry) => {}
            Ok(Step::NoRoom) => break,
            Ok(Step::Eof) => {
                failure = Some(io::ErrorKind::WriteZero.into());
                break;
            }
            Ok(Step::Failed(e)) => {
                failure = Some(e);
                break;
            }
            Err(_) => break,
        }
    }

    // The PSP's FIN, once everything before it has gone out.
    if failure.is_none()
        && !conn.host_shut
        && sock.recv_queue() == 0
        && matches!(
            sock.state(),
            tcp::State::CloseWait
                | tcp::State::LastAck
                | tcp::State::Closing
                | tcp::State::TimeWait
                | tcp::State::Closed
        )
    {
        let _ = stream.shutdown(Shutdown::Write);
        conn.host_shut = true;
    }

    // host -> PSP
    while failure.is_none() && conn.host_readable && !conn.host_eof && sock.can_send() {
        let r = sock.send(|room| match step(stream.read(room), room.len()) {
            Step::Moved(n) => (n, Step::Moved(n)),
            other => (0, other),
        });
        match r {
            Ok(Step::Moved(n)) => {
                conn.down += n as u64;
                stats.tcp_down += n as u64;
                moved = true;
            }
            Ok(Step::Eof) => {
                conn.host_eof = true;
                sock.close();
                moved = true;
            }
            Ok(Step::Blocked) => conn.host_readable = false,
            Ok(Step::Retry) => {}
            Ok(Step::NoRoom) => break,
            Ok(Step::Failed(e)) => failure = Some(e),
            Err(_) => break,
        }
    }

    if let Some(e) = failure {
        // The host side broke: reset the PSP's side. smoltcp sends the RST
        // on its next poll; the connection is removed after that.
        conn.reason = Some(format!("host side: {e}"));
        sock.abort();
        finish_host(key, conn, stats);
        return true;
    }

    // Both directions closed and delivered: the host side is done. The
    // smoltcp socket lives on through TIME-WAIT.
    if conn.host_eof && conn.host_shut && sock.state() == tcp::State::TimeWait {
        finish_host(key, conn, stats);
    }
    moved
}
