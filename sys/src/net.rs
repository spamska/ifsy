/*
 * Copyright (c) 2026 spamska and contributors
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
 * FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
 * COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
 * BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 * LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
 * AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
 * USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 * NOTE: This file is licensed under the BSD 2-Clause license. 
 * This does not imply that all components or parts of the Ifsy 
 * operating system are distributed under the same terms. Please consult 
 * the individual components or the root license documentation for details.
 */

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device};
use smoltcp::iface::Context;
use smoltcp::socket::{dhcpv4, dns, icmp, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{DnsQueryType, EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};

use crate::console::color::{failed, pass, warn};
use crate::sync::SpinLock;
use crate::timer::{self, Deadline};
use crate::nic::{self, Nic};

const PING_IDENT: u16 = 0x5a5a;
const TCP_BUF: usize = 16 * 1024;
const UDP_PKTS: usize = 16;
const UDP_BUF: usize = 16 * 1024;
const CLOSE_GRACE_MS: u64 = 15_000;
const PORT_LOW: u16 = 49152;
const PORT_SPAN: u32 = 16384;

pub struct Lease {
    pub address: Ipv4Cidr,
    pub router: Option<Ipv4Address>,
    pub dns: Vec<Ipv4Address>,
}

struct Net {
    dev: Nic,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: SocketHandle,
    icmp: SocketHandle,
    lease: Option<Lease>,
    dying: Vec<(SocketHandle, u64)>,
}

static NET: SpinLock<Option<Net>> = SpinLock::new(None);
static NEXT_PORT: SpinLock<u32> = SpinLock::new(0);

fn now() -> Instant {
    Instant::from_millis(timer::uptime_ms() as i64)
}

pub fn init() -> bool {
    let Some(mut dev) = nic::probe() else {
        kprintln!("ifsy: net: no network device; networking is off {}", warn());
        return false;
    };
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(dev.mac())));
    config.random_seed = timer::counter() ^ 0x9e37_79b9_7f4a_7c15;
    let iface = Interface::new(config, &mut dev, now());
    let mut sockets = SocketSet::new(Vec::new());
    let dhcp = sockets.add(dhcpv4::Socket::new());
    let icmp = sockets.add(icmp::Socket::new(
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 4], vec![0; 512]),
        icmp::PacketBuffer::new(vec![icmp::PacketMetadata::EMPTY; 4], vec![0; 512]),
    ));
    sockets.get_mut::<icmp::Socket>(icmp).bind(icmp::Endpoint::Ident(PING_IDENT)).ok();
    NET.with(|n| *n = Some(Net { dev, iface, sockets, dhcp, icmp, lease: None, dying: Vec::new() }));
    true
}

impl Net {
    fn poll(&mut self) {
        let t = now();
        let _: PollResult = self.iface.poll(t, &mut self.dev, &mut self.sockets);
        let event = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll();
        match event {
            Some(dhcpv4::Event::Configured(cfg)) => {
                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    addrs.push(IpCidr::Ipv4(cfg.address)).ok();
                });
                match cfg.router {
                    Some(r) => {
                        self.iface.routes_mut().add_default_ipv4_route(r).ok();
                    }
                    None => {
                        self.iface.routes_mut().remove_default_ipv4_route();
                    }
                }
                let dns: Vec<Ipv4Address> = cfg.dns_servers.iter().copied().collect();
                kprintln!(
                    "ifsy: net: dhcp lease {} router {} dns {} {}",
                    cfg.address,
                    cfg.router.map(|r| alloc::format!("{}", r)).unwrap_or_else(|| alloc::string::String::from("none")),
                    dns.first().map(|d| alloc::format!("{}", d)).unwrap_or_else(|| alloc::string::String::from("none")),
                    pass()
                );
                self.lease = Some(Lease { address: cfg.address, router: cfg.router, dns });
            }
            Some(dhcpv4::Event::Deconfigured) => {
                self.iface.update_ip_addrs(|addrs| addrs.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                self.lease = None;
                kprintln!("ifsy: net: dhcp lease lost {}", warn());
            }
            None => {}
        }
        self.reap();
    }

    fn reap(&mut self) {
        let t = timer::uptime_ms();
        let mut i = 0;
        while i < self.dying.len() {
            let (h, deadline) = self.dying[i];
            let sock = self.sockets.get_mut::<tcp::Socket>(h);
            if matches!(sock.state(), tcp::State::Closed | tcp::State::TimeWait) || t >= deadline {
                sock.abort();
                let _ = self.sockets.remove(h);
                self.dying.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

pub fn poll() {
    NET.with(|n| {
        if let Some(n) = n.as_mut() {
            n.poll();
        }
    });
}

pub fn active() -> bool {
    NET.with(|n| n.is_some())
}

pub fn lease() -> Option<(Ipv4Cidr, Option<Ipv4Address>)> {
    NET.with(|n| n.as_ref().and_then(|n| n.lease.as_ref().map(|l| (l.address, l.router))))
}

fn wait(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let limit = Deadline::after_ms(ms);
    loop {
        poll();
        if done() {
            return true;
        }
        if limit.expired() {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn ping(target: Ipv4Address) -> bool {
    let sent = NET.with(|n| {
        let n = n.as_mut()?;
        let sock = n.sockets.get_mut::<icmp::Socket>(n.icmp);
        while sock.can_recv() {
            let _ = sock.recv();
        }
        let repr = Icmpv4Repr::EchoRequest { ident: PING_IDENT, seq_no: 1, data: &[0x55; 32] };
        let buf = sock.send(repr.buffer_len(), IpAddress::Ipv4(target)).ok()?;
        repr.emit(&mut Icmpv4Packet::new_unchecked(buf), &ChecksumCapabilities::default());
        Some(())
    });
    if sent.is_none() {
        return false;
    }
    wait(3000, || {
        NET.with(|n| {
            let Some(n) = n.as_mut() else { return false };
            let sock = n.sockets.get_mut::<icmp::Socket>(n.icmp);
            while let Ok((data, src)) = sock.recv() {
                if src != IpAddress::Ipv4(target) {
                    continue;
                }
                let Ok(pkt) = Icmpv4Packet::new_checked(data) else { continue };
                if let Ok(Icmpv4Repr::EchoReply { ident: PING_IDENT, .. }) = Icmpv4Repr::parse(&pkt, &ChecksumCapabilities::default()) {
                    return true;
                }
            }
            false
        })
    })
}

pub fn selftest() {
    if !active() {
        return;
    }
    if !wait(8000, || lease().is_some()) {
        kprintln!("ifsy: net: no DHCP answer within 8 s {}", warn());
        return;
    }
    let Some((_, Some(router))) = lease() else {
        kprintln!("ifsy: net: lease has no router, skipping the ping test {}", warn());
        return;
    };
    if ping(router) {
        kprintln!("ifsy: net: ping {} {}", router, pass());
    } else {
        kprintln!("ifsy: net: ping {} got no reply {}", router, failed());
    }
}

pub fn add_tcp() -> Option<SocketHandle> {
    NET.with(|n| {
        let n = n.as_mut()?;
        let sock = tcp::Socket::new(tcp::SocketBuffer::new(vec![0; TCP_BUF]), tcp::SocketBuffer::new(vec![0; TCP_BUF]));
        Some(n.sockets.add(sock))
    })
}

pub fn add_udp() -> Option<SocketHandle> {
    NET.with(|n| {
        let n = n.as_mut()?;
        let sock = udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; UDP_PKTS], vec![0; UDP_BUF]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; UDP_PKTS], vec![0; UDP_BUF]),
        );
        Some(n.sockets.add(sock))
    })
}

pub fn remove_tcp(h: SocketHandle) {
    NET.with(|n| {
        if let Some(n) = n.as_mut() {
            n.sockets.get_mut::<tcp::Socket>(h).abort();
            let _ = n.sockets.remove(h);
        }
    });
}

pub fn retire_tcp(h: SocketHandle) {
    NET.with(|n| {
        if let Some(n) = n.as_mut() {
            n.sockets.get_mut::<tcp::Socket>(h).close();
            n.dying.push((h, timer::uptime_ms() + CLOSE_GRACE_MS));
        }
    });
}

pub fn remove_udp(h: SocketHandle) {
    NET.with(|n| {
        if let Some(n) = n.as_mut() {
            n.sockets.get_mut::<udp::Socket>(h).close();
            let _ = n.sockets.remove(h);
        }
    });
}

pub fn with_tcp<R>(h: SocketHandle, f: impl FnOnce(&mut tcp::Socket<'static>, &mut Context) -> R) -> Option<R> {
    NET.with(|n| {
        let n = n.as_mut()?;
        let sock = n.sockets.get_mut::<tcp::Socket>(h);
        Some(f(sock, n.iface.context()))
    })
}

pub fn with_udp<R>(h: SocketHandle, f: impl FnOnce(&mut udp::Socket<'static>) -> R) -> Option<R> {
    NET.with(|n| {
        let n = n.as_mut()?;
        Some(f(n.sockets.get_mut::<udp::Socket>(h)))
    })
}

pub fn local_ip() -> Option<Ipv4Address> {
    NET.with(|n| n.as_ref().and_then(|n| n.iface.ipv4_addr()))
}

pub fn alloc_port(taken: impl Fn(u16) -> bool) -> Option<u16> {
    for _ in 0..PORT_SPAN {
        let port = NEXT_PORT.with(|p| {
            if *p == 0 {
                *p = (timer::counter() % PORT_SPAN as u64) as u32 + 1;
            }
            *p = *p % PORT_SPAN + 1;
            PORT_LOW + (*p - 1) as u16
        });
        if !taken(port) {
            return Some(port);
        }
    }
    None
}

pub fn dns_start(name: &str) -> Result<(SocketHandle, dns::QueryHandle), &'static str> {
    NET.with(|n| {
        let n = n.as_mut().ok_or("no network device")?;
        let servers: Vec<IpAddress> = n
            .lease
            .as_ref()
            .map(|l| l.dns.iter().map(|d| IpAddress::Ipv4(*d)).collect())
            .unwrap_or_default();
        if servers.is_empty() {
            return Err("no dns server");
        }
        let h = n.sockets.add(dns::Socket::new(&servers, vec![None]));
        let started = {
            let sock = n.sockets.get_mut::<dns::Socket>(h);
            sock.start_query(n.iface.context(), name, DnsQueryType::A)
        };
        match started {
            Ok(q) => Ok((h, q)),
            Err(_) => {
                let _ = n.sockets.remove(h);
                Err("bad host name")
            }
        }
    })
}

pub fn dns_result(h: SocketHandle, q: dns::QueryHandle) -> Option<Result<Ipv4Address, &'static str>> {
    NET.with(|n| {
        let n = n.as_mut()?;
        let r = n.sockets.get_mut::<dns::Socket>(h).get_query_result(q);
        match r {
            Err(dns::GetQueryResultError::Pending) => None,
            Err(_) => Some(Err("dns lookup failed")),
            Ok(addrs) => Some(
                addrs
                    .iter()
                    .map(|a| {
                        let IpAddress::Ipv4(v) = *a;
                        v
                    })
                    .next()
                    .ok_or("no such host"),
            ),
        }
    })
}

pub fn dns_done(h: SocketHandle) {
    NET.with(|n| {
        if let Some(n) = n.as_mut() {
            let _ = n.sockets.remove(h);
        }
    });
}
