//! UDP: DHCP, DNS to the PC's resolvers, and every other datagram through a host socket.

use super::*;

impl Gateway {
    pub(super) fn handle_udp(&mut self, ip: &packet::Ipv4) -> bool {
        let Some(udp) = packet::parse_udp(ip.payload) else { return false };
        if udp.dst_port == dhcp::SERVER_PORT
            && (ip.dst.is_broadcast() || ip.dst == self.cfg.gateway_ip || ip.dst == self.cfg.broadcast())
        {
            return self.handle_dhcp(udp.payload);
        }
        if self.is_broadcast_or_multicast(ip.dst) || udp.dst_port == 0 || udp.src_port == 0 {
            return false;
        }
        let client = SocketAddrV4::new(ip.src, udp.src_port);
        let dst = SocketAddrV4::new(ip.dst, udp.dst_port);
        self.udp_out(client, dst, udp.payload)
    }

    pub(super) fn handle_dhcp(&mut self, payload: &[u8]) -> bool {
        let Some(req) = dhcp::parse(payload) else { return false };
        let lease = dhcp::Lease {
            server: self.cfg.gateway_ip,
            client: self.cfg.client_ip,
            netmask: self.cfg.netmask(),
            broadcast: self.cfg.broadcast(),
            lease_secs: self.cfg.lease_secs,
        };
        let mac = mac_str(req.mac());
        let name = req.hostname.as_ref().map(|h| format!(" \"{h}\"")).unwrap_or_default();
        let action = dhcp::decide(&req, &lease);
        if req.msg_type == Some(dhcp::DISCOVER) {
            // The client's network stack starts over: what it had open is gone.
            self.abort_all("the PSP restarted its network (DHCP DISCOVER)");
        }
        match action {
            dhcp::Action::Ignore => {
                let t = req.msg_type.unwrap_or(0);
                logln!("dhcp: {} from {mac}{name}, not answered", dhcp::type_name(t));
                return true;
            }
            dhcp::Action::Reply(dhcp::OFFER) => {
                logln!("dhcp: DISCOVER from {mac}{name}, offering {}", lease.client);
            }
            dhcp::Action::Reply(dhcp::NAK) => {
                let asked = req.requested_ip.unwrap_or(req.ciaddr);
                logln!("dhcp: REQUEST for {asked} from {mac}{name} refused (NAK)");
            }
            dhcp::Action::Reply(_) if req.msg_type == Some(dhcp::INFORM) => {
                logln!("dhcp: INFORM from {mac}{name} answered");
            }
            dhcp::Action::Reply(_) | dhcp::Action::BootpReply => {
                self.stats.dhcp_leases += 1;
                crate::ui::connected(&lease.client.to_string());
                logln!(
                    "dhcp: lease {}/{} to {mac}{name} for {} s, router and DNS {}",
                    lease.client,
                    self.cfg.prefix_len,
                    lease.lease_secs,
                    lease.server
                );
            }
        }
        let reply = dhcp::build_reply(&req, &action, &lease);
        // Always broadcast: the client may not have its address yet, and an
        // old BSD client takes a broadcast in every state.
        let frame = packet::build_udp(
            &BROADCAST_MAC,
            &self.cfg.gateway_mac,
            SocketAddrV4::new(self.cfg.gateway_ip, dhcp::SERVER_PORT),
            SocketAddrV4::new(Ipv4Addr::BROADCAST, dhcp::CLIENT_PORT),
            &reply,
        );
        self.emit(&frame);
        true
    }

    pub(super) fn resolvers(&self) -> Vec<SocketAddr> {
        let mut list = match &self.cfg.resolvers {
            Some(r) => r.clone(),
            None => std::fs::read_to_string("/etc/resolv.conf")
                .map(|t| parse_resolv_conf(&t))
                .unwrap_or_default(),
        };
        if list.is_empty() {
            list.push(SocketAddr::new(Ipv4Addr::new(1, 1, 1, 1).into(), 53));
        }
        // One socket per flow, so one address family: that of the first.
        let v4 = list[0].is_ipv4();
        list.retain(|a| a.is_ipv4() == v4);
        list
    }

    pub(super) fn udp_out(&mut self, client: SocketAddrV4, dst: SocketAddrV4, payload: &[u8]) -> bool {
        let dns = *dst.ip() == self.cfg.gateway_ip && dst.port() == 53;
        let key = (client, if dns { dst } else { ANY });
        let now = Instant::now();
        if !self.flows.contains_key(&key) {
            if self.flows.len() >= MAX_FLOWS {
                let oldest = self.flows.iter().min_by_key(|(_, f)| f.last).map(|(k, _)| *k);
                if let Some(k) = oldest {
                    self.remove_flow(&k);
                }
            }
            let targets = if dns { self.resolvers() } else { Vec::new() };
            let bind: SocketAddr = match targets.first() {
                Some(t) if t.is_ipv6() => (Ipv6Addr::UNSPECIFIED, 0).into(),
                _ => (Ipv4Addr::UNSPECIFIED, 0).into(),
            };
            let token = self.new_token(Owner::Udp(key));
            let made = UdpSocket::bind(bind).and_then(|mut sock| {
                if dns {
                    sock.connect(targets[0])?;
                }
                self.poll.registry().register(&mut sock, token, Interest::READABLE)?;
                Ok(sock)
            });
            match made {
                Ok(sock) => {
                    let flow = Flow {
                        sock,
                        token,
                        targets,
                        target: 0,
                        dns,
                        peers: HashMap::new(),
                        last: now,
                        waiting_since: None,
                    };
                    self.flows.insert(key, flow);
                }
                Err(e) => {
                    self.tokens.remove(&token);
                    logln!("udp: {client} -> {dst}: no socket: {e}");
                    return true;
                }
            }
        }
        let to = self.host_addr(dst);
        let flow = self.flows.get_mut(&key).unwrap();
        flow.last = now;
        let sent = if dns {
            self.stats.dns_queries += 1;
            match flow.waiting_since {
                // The same query again, still unanswered: try the next resolver.
                Some(since) if now - since >= DNS_RETRY_OTHER && flow.targets.len() > 1 => {
                    flow.target = (flow.target + 1) % flow.targets.len();
                    let _ = flow.sock.connect(flow.targets[flow.target]);
                    flow.waiting_since = Some(now);
                }
                Some(_) => {}
                None => flow.waiting_since = Some(now),
            }
            flow.sock.send(payload)
        } else {
            if flow.peers.len() >= MAX_PEERS && !flow.peers.contains_key(&to) {
                flow.peers.clear();
            }
            flow.peers.insert(to, dst);
            flow.sock.send_to(payload, to)
        };
        match sent {
            Ok(_) => self.stats.udp_out += 1,
            Err(e) => {
                if self.cfg.verbose {
                    logln!("udp: {client} -> {dst}: {e}");
                }
            }
        }
        true
    }

    pub(super) fn remove_flow(&mut self, key: &ConnKey) {
        if let Some(flow) = self.flows.remove(key) {
            self.tokens.remove(&flow.token);
        }
    }

    pub(super) fn on_udp_readable(&mut self, key: ConnKey) {
        let (client, dst) = key;
        let mac = self.client_mac.unwrap_or(BROADCAST_MAC);
        let mut buf = std::mem::take(&mut self.scratch);
        let mut frames = Vec::new();
        if let Some(flow) = self.flows.get_mut(&key) {
            let mut errors = 0;
            loop {
                let got = if flow.dns {
                    flow.sock.recv(&mut buf).map(|n| (n, Some(dst)))
                } else {
                    // From anyone; under the name the PSP knows the sender by.
                    flow.sock.recv_from(&mut buf).map(|(n, from)| {
                        let name = flow.peers.get(&from).copied().or(match from {
                            SocketAddr::V4(a) => Some(a),
                            SocketAddr::V6(_) => None,
                        });
                        (n, name)
                    })
                };
                match got {
                    Ok((n, _)) if n > packet::MAX_UDP_PAYLOAD => {
                        logln!("udp: {n} byte datagram for {client} is too big, dropped");
                    }
                    Ok((_, None)) => {}
                    Ok((n, Some(from))) => {
                        flow.last = Instant::now();
                        flow.waiting_since = None;
                        if flow.dns {
                            self.stats.dns_answers += 1;
                        }
                        self.stats.udp_in += 1;
                        frames.push(packet::build_udp(
                            &mac,
                            &self.cfg.gateway_mac,
                            from,
                            client,
                            &buf[..n],
                        ));
                    }
                    Err(e) if is_would_block(&e) => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        // For instance "connection refused" from an ICMP error.
                        errors += 1;
                        if errors > 8 {
                            break;
                        }
                    }
                }
            }
        }
        self.scratch = buf;
        for f in frames {
            self.emit(&f);
        }
    }
}
