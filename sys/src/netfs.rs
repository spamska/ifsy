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

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::{tcp, udp};
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint, Ipv4Address};

use crate::console::color::{failed, pass, warn};
use crate::net;
use crate::p9::{self, Stat};
use crate::sched;
use crate::sync::SpinLock;
use crate::timer::Deadline;

const MAX_CONV: usize = 256;
const MAX_IO: u64 = 1 << 20;
const BACKLOG: usize = 4;
const CONNECT_MS: u64 = 30_000;
const DNS_MS: u64 = 5_000;
const TCP_CHUNK: usize = 16 * 1024;
const UDP_CHUNK: usize = 16 * 1024;
const UDPHDR: usize = 52;
const UNSPEC: Ipv4Address = Ipv4Address::new(0, 0, 0, 0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ctl,
    Data,
    Listen,
    Local,
    Remote,
    Status,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Idle,
    Announced,
    Connecting,
    Connected,
    Closed,
}

struct Conv {
    inuse: u32,
    st: St,
    handle: Option<SocketHandle>,
    backlog: Vec<SocketHandle>,
    laddr: Ipv4Address,
    lport: u16,
    raddr: Ipv4Address,
    rport: u16,
    headers: bool,
}

type Table = SpinLock<Vec<Option<Conv>>>;

static TCP: Table = SpinLock::new(Vec::new());
static UDP: Table = SpinLock::new(Vec::new());

impl Proto {
    fn name(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }

    fn table(self) -> &'static Table {
        match self {
            Proto::Tcp => &TCP,
            Proto::Udp => &UDP,
        }
    }
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Ctl => "ctl",
            Kind::Data => "data",
            Kind::Listen => "listen",
            Kind::Local => "local",
            Kind::Remote => "remote",
            Kind::Status => "status",
        }
    }

    fn perm(self) -> u32 {
        match self {
            Kind::Ctl | Kind::Data | Kind::Listen => 0o660,
            _ => 0o444,
        }
    }

    fn writable(self) -> bool {
        matches!(self, Kind::Ctl | Kind::Data | Kind::Listen)
    }

    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "ctl" => Kind::Ctl,
            "data" => Kind::Data,
            "listen" => Kind::Listen,
            "local" => Kind::Local,
            "remote" => Kind::Remote,
            "status" => Kind::Status,
            _ => return None,
        })
    }
}

const KINDS: [Kind; 6] = [Kind::Ctl, Kind::Data, Kind::Listen, Kind::Local, Kind::Remote, Kind::Status];

pub struct Hold {
    proto: Proto,
    idx: usize,
}

impl Drop for Hold {
    fn drop(&mut self) {
        release(self.proto, self.idx);
    }
}

pub struct NetObj {
    pub kind: Kind,
    pub proto: Proto,
    pub idx: usize,
    hold: Hold,
    pos: SpinLock<u64>,
}

pub struct CsObj {
    answers: SpinLock<(Vec<String>, usize)>,
}

pub enum Opened {
    Dir(String),
    Obj(NetObj),
    Cs(CsObj),
}

enum Node {
    Root,
    Cs,
    ProtoDir(Proto),
    Clone(Proto),
    Conv(Proto, usize),
    File(Proto, usize, Kind),
}

fn parse_proto(s: &str) -> Option<Proto> {
    match s {
        "tcp" => Some(Proto::Tcp),
        "udp" => Some(Proto::Udp),
        _ => None,
    }
}

fn parse(path: &str) -> Option<Node> {
    let rest = path.strip_prefix("/net")?;
    if rest.is_empty() {
        return Some(Node::Root);
    }
    let rest = rest.strip_prefix('/')?;
    let mut it = rest.split('/');
    let first = it.next()?;
    if first == "cs" {
        return if it.next().is_none() { Some(Node::Cs) } else { None };
    }
    let proto = parse_proto(first)?;
    let Some(second) = it.next() else { return Some(Node::ProtoDir(proto)) };
    if second == "clone" {
        return if it.next().is_none() { Some(Node::Clone(proto)) } else { None };
    }
    if second.is_empty() || !second.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let idx: usize = second.parse().ok()?;
    let Some(third) = it.next() else { return Some(Node::Conv(proto, idx)) };
    if it.next().is_some() {
        return None;
    }
    Some(Node::File(proto, idx, Kind::parse(third)?))
}

pub fn is_net(path: &str) -> bool {
    path == "/net" || path.starts_with("/net/")
}

fn live(p: Proto, idx: usize) -> bool {
    p.table().with(|t| matches!(t.get(idx), Some(Some(c)) if c.inuse > 0))
}

fn mk(name: &str, dir: bool, perm: u32, path: &str) -> Stat {
    Stat { name: String::from(name), is_dir: dir, perm, length: 0, mtime: 0, qid_path: p9::hash_path(path) }
}

pub fn stat(path: &str) -> Option<Stat> {
    match parse(path)? {
        Node::Root => Some(mk("net", true, 0o555, path)),
        Node::Cs => Some(mk("cs", false, 0o666, path)),
        Node::ProtoDir(p) => Some(mk(p.name(), true, 0o555, path)),
        Node::Clone(_) => Some(mk("clone", false, 0o666, path)),
        Node::Conv(p, i) => live(p, i).then(|| mk(&format!("{}", i), true, 0o555, path)),
        Node::File(p, i, k) => live(p, i).then(|| mk(k.name(), false, k.perm(), path)),
    }
}

pub fn entries(path: &str) -> Option<Vec<Stat>> {
    match parse(path)? {
        Node::Root => {
            let mut out: Vec<Stat> =
                [Proto::Tcp, Proto::Udp].iter().map(|p| mk(p.name(), true, 0o555, &format!("/net/{}", p.name()))).collect();
            out.push(mk("cs", false, 0o666, "/net/cs"));
            Some(out)
        }
        Node::ProtoDir(p) => {
            let mut out = Vec::new();
            out.push(mk("clone", false, 0o666, &format!("/net/{}/clone", p.name())));
            let ids: Vec<usize> = p
                .table()
                .with(|t| t.iter().enumerate().filter(|(_, c)| matches!(c, Some(c) if c.inuse > 0)).map(|(i, _)| i).collect());
            for i in ids {
                out.push(mk(&format!("{}", i), true, 0o555, &format!("/net/{}/{}", p.name(), i)));
            }
            Some(out)
        }
        Node::Conv(p, i) => {
            if !live(p, i) {
                return None;
            }
            Some(KINDS.iter().map(|k| mk(k.name(), false, k.perm(), &format!("/net/{}/{}/{}", p.name(), i, k.name()))).collect())
        }
        _ => None,
    }
}

fn new_conv() -> Conv {
    Conv {
        inuse: 1,
        st: St::Idle,
        handle: None,
        backlog: Vec::new(),
        laddr: UNSPEC,
        lport: 0,
        raddr: UNSPEC,
        rport: 0,
        headers: false,
    }
}

fn alloc(p: Proto) -> Result<Hold, &'static str> {
    p.table().with(|t| {
        let idx = match t.iter().position(|c| c.is_none()) {
            Some(i) => i,
            None => {
                if t.len() >= MAX_CONV {
                    return Err("no free conversations");
                }
                t.push(None);
                t.len() - 1
            }
        };
        t[idx] = Some(new_conv());
        Ok(Hold { proto: p, idx })
    })
}

fn acquire(p: Proto, idx: usize) -> Result<Hold, &'static str> {
    p.table().with(|t| match t.get_mut(idx) {
        Some(Some(c)) if c.inuse > 0 => {
            c.inuse += 1;
            Ok(Hold { proto: p, idx })
        }
        _ => Err("file does not exist"),
    })
}

fn release(p: Proto, idx: usize) {
    let dead = p.table().with(|t| {
        let slot = t.get_mut(idx)?;
        let c = slot.as_mut()?;
        c.inuse -= 1;
        if c.inuse > 0 { None } else { slot.take() }
    });
    if let Some(c) = dead {
        destroy(p, c);
    }
}

fn destroy(p: Proto, c: Conv) {
    for h in c.backlog {
        net::remove_tcp(h);
    }
    if let Some(h) = c.handle {
        match p {
            Proto::Tcp if matches!(c.st, St::Connected | St::Closed) => net::retire_tcp(h),
            Proto::Tcp => net::remove_tcp(h),
            Proto::Udp => net::remove_udp(h),
        }
    }
}

fn obj(h: Hold, kind: Kind) -> NetObj {
    NetObj { kind, proto: h.proto, idx: h.idx, hold: h, pos: SpinLock::new(0) }
}

impl NetObj {
    pub fn path(&self) -> String {
        format!("/net/{}/{}/{}", self.proto.name(), self.idx, self.kind.name())
    }

    pub fn stat(&self) -> Stat {
        mk(self.kind.name(), false, self.kind.perm(), &self.path())
    }
}

pub fn open(path: &str, mode: u64) -> Result<Opened, &'static str> {
    let acc = mode & 3;
    match parse(path).ok_or("file does not exist")? {
        Node::Root | Node::ProtoDir(_) | Node::Conv(..) => {
            if stat(path).is_none() {
                return Err("file does not exist");
            }
            if acc != 0 && acc != 3 {
                return Err("is a directory");
            }
            Ok(Opened::Dir(String::from(path)))
        }
        Node::Cs => Ok(Opened::Cs(CsObj { answers: SpinLock::new((Vec::new(), 0)) })),
        Node::Clone(p) => {
            let h = alloc(p)?;
            Ok(Opened::Obj(obj(h, Kind::Ctl)))
        }
        Node::File(p, i, k) => {
            if !k.writable() && acc != 0 {
                return Err("permission denied");
            }
            let h = acquire(p, i)?;
            if k == Kind::Listen {
                if p != Proto::Tcp {
                    return Err("not a stream protocol");
                }
                let nh = accept_call(p, i)?;
                return Ok(Opened::Obj(obj(nh, Kind::Ctl)));
            }
            Ok(Opened::Obj(obj(h, k)))
        }
    }
}

fn wait_on<R>(mut f: impl FnMut() -> Option<Result<R, &'static str>>) -> Result<R, &'static str> {
    let Some(me) = sched::current() else { return Err("no current thread") };
    loop {
        net::poll();
        if let Some(r) = f() {
            return r;
        }
        if !sched::switching_active() {
            return Err("operation would block");
        }
        sched::sys_netwait();
        if matches!(sched::finish_blocked_syscall(me), Some(r) if r != 0) {
            return Err("interrupted");
        }
    }
}

fn parse_port(s: &str) -> Result<u16, &'static str> {
    s.parse::<u16>().map_err(|_| "bad port")
}

fn parse_ip(s: &str) -> Result<Ipv4Address, &'static str> {
    if s.is_empty() || s == "*" {
        return Ok(UNSPEC);
    }
    s.parse::<Ipv4Address>().map_err(|_| "bad address")
}

fn parse_addr(s: &str) -> Result<(Ipv4Address, u16), &'static str> {
    let mut parts = s.split('!');
    let first = parts.next().ok_or("bad address")?;
    match parts.next() {
        None => Ok((UNSPEC, parse_port(first)?)),
        Some(port) => Ok((parse_ip(first)?, parse_port(port)?)),
    }
}

fn endpoint(addr: Ipv4Address, port: u16) -> IpEndpoint {
    IpEndpoint::new(IpAddress::Ipv4(addr), port)
}

fn split_ep(ep: IpEndpoint) -> (Ipv4Address, u16) {
    let IpAddress::Ipv4(a) = ep.addr;
    (a, ep.port)
}

fn taken(t: &[Option<Conv>], port: u16) -> bool {
    t.iter().flatten().any(|c| c.lport == port && c.st != St::Idle && c.st != St::Closed)
}

fn with_conv<R>(p: Proto, idx: usize, f: impl FnOnce(&mut Conv) -> R) -> Result<R, &'static str> {
    p.table().with(|t| match t.get_mut(idx) {
        Some(Some(c)) => Ok(f(c)),
        _ => Err("conversation closed"),
    })
}

fn handle_of(p: Proto, idx: usize) -> Result<SocketHandle, &'static str> {
    with_conv(p, idx, |c| match c.st {
        St::Connected | St::Closed => c.handle,
        _ => None,
    })?
    .ok_or("not connected")
}

fn listener_endpoint(addr: Ipv4Address, port: u16) -> IpListenEndpoint {
    IpListenEndpoint { addr: if addr == UNSPEC { None } else { Some(IpAddress::Ipv4(addr)) }, port }
}

fn new_listener(addr: Ipv4Address, port: u16) -> Result<SocketHandle, &'static str> {
    let h = net::add_tcp().ok_or("no network device")?;
    let ok = net::with_tcp(h, |s, _| s.listen(listener_endpoint(addr, port)).is_ok()).unwrap_or(false);
    if !ok {
        net::remove_tcp(h);
        return Err("address already in use");
    }
    Ok(h)
}

fn ready(s: &tcp::Socket) -> bool {
    matches!(s.state(), tcp::State::Established | tcp::State::CloseWait)
}

fn accept_call(p: Proto, idx: usize) -> Result<Hold, &'static str> {
    let claimed = wait_on(|| {
        let r = p.table().with(|t| {
            let c = match t.get_mut(idx) {
                Some(Some(c)) => c,
                _ => return Err("conversation closed"),
            };
            if c.st != St::Announced {
                return Err("not announced");
            }
            let pos = c.backlog.iter().position(|&h| net::with_tcp(h, |s, _| ready(s)).unwrap_or(false));
            Ok(pos.map(|i| (c.backlog.swap_remove(i), c.laddr, c.lport)))
        });
        match r {
            Err(e) => Some(Err(e)),
            Ok(None) => None,
            Ok(Some(v)) => Some(Ok(v)),
        }
    })?;
    let (h, laddr, lport) = claimed;
    match new_listener(laddr, lport) {
        Ok(nl) => {
            let kept = with_conv(p, idx, |c| {
                if c.st == St::Announced {
                    c.backlog.push(nl);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            if !kept {
                net::remove_tcp(nl);
            }
        }
        Err(_) => {}
    }
    let hold = match alloc(p) {
        Ok(h2) => h2,
        Err(e) => {
            net::remove_tcp(h);
            return Err(e);
        }
    };
    let eps = net::with_tcp(h, |s, _| (s.local_endpoint(), s.remote_endpoint())).unwrap_or((None, None));
    let _ = with_conv(p, hold.idx, |c| {
        c.st = St::Connected;
        c.handle = Some(h);
        if let Some(l) = eps.0 {
            (c.laddr, c.lport) = split_ep(l);
        }
        if let Some(r) = eps.1 {
            (c.raddr, c.rport) = split_ep(r);
        }
    });
    Ok(hold)
}

fn tcp_connect(p: Proto, idx: usize, args: &[&str]) -> Result<(), &'static str> {
    let (raddr, rport) = parse_addr(args.first().ok_or("bad address")?)?;
    if rport == 0 || raddr == UNSPEC {
        return Err("bad address");
    }
    let want = match args.get(1) {
        Some(s) => parse_port(s)?,
        None => 0,
    };
    let lport = p.table().with(|t| {
        let bound = match t.get(idx) {
            Some(Some(c)) if c.st == St::Idle => c.lport,
            Some(Some(_)) => return Err("connection in use"),
            _ => return Err("conversation closed"),
        };
        let port = if want != 0 {
            want
        } else if bound != 0 {
            bound
        } else {
            net::alloc_port(|q| taken(t, q)).ok_or("no free ports")?
        };
        if let Some(Some(c)) = t.get_mut(idx) {
            c.st = St::Connecting;
            c.lport = port;
            c.raddr = raddr;
            c.rport = rport;
        }
        Ok(port)
    })?;
    let revert = |h: Option<SocketHandle>| {
        if let Some(h) = h {
            net::remove_tcp(h);
        }
        let _ = with_conv(p, idx, |c| {
            c.st = St::Idle;
            c.handle = None;
        });
    };
    let Some(h) = net::add_tcp() else {
        revert(None);
        return Err("no network device");
    };
    let started = net::with_tcp(h, |s, cx| {
        s.set_nagle_enabled(false);
        s.connect(cx, endpoint(raddr, rport), IpListenEndpoint { addr: None, port: lport })
    });
    match started {
        Some(Ok(())) => {}
        Some(Err(_)) | None => {
            revert(Some(h));
            return Err("network unreachable");
        }
    }
    let _ = with_conv(p, idx, |c| c.handle = Some(h));
    let limit = Deadline::after_ms(CONNECT_MS);
    let outcome = wait_on(|| {
        let st = net::with_tcp(h, |s, _| s.state());
        match st {
            Some(tcp::State::Established) | Some(tcp::State::CloseWait) => Some(Ok(())),
            Some(tcp::State::SynSent) | Some(tcp::State::SynReceived) => {
                if limit.expired() {
                    Some(Err("connection timed out"))
                } else {
                    None
                }
            }
            Some(tcp::State::Closed) => Some(Err("connection refused")),
            Some(_) => Some(Err("connection reset by peer")),
            None => Some(Err("no network device")),
        }
    });
    match outcome {
        Ok(()) => {
            let local = net::with_tcp(h, |s, _| s.local_endpoint()).flatten();
            let _ = with_conv(p, idx, |c| {
                c.st = St::Connected;
                if let Some(l) = local {
                    (c.laddr, c.lport) = split_ep(l);
                }
            });
            Ok(())
        }
        Err(e) => {
            revert(Some(h));
            Err(e)
        }
    }
}

fn tcp_announce(p: Proto, idx: usize, args: &[&str]) -> Result<(), &'static str> {
    let (laddr, mut lport) = parse_addr(args.first().ok_or("bad address")?)?;
    let port = p.table().with(|t| {
        match t.get(idx) {
            Some(Some(c)) if c.st == St::Idle => {
                if lport == 0 {
                    lport = c.lport;
                }
            }
            Some(Some(_)) => return Err("connection in use"),
            _ => return Err("conversation closed"),
        }
        if lport == 0 {
            lport = net::alloc_port(|q| taken(t, q)).ok_or("no free ports")?;
        } else if taken(t, lport) {
            return Err("address already in use");
        }
        if let Some(Some(c)) = t.get_mut(idx) {
            c.st = St::Announced;
            c.laddr = laddr;
            c.lport = lport;
        }
        Ok(lport)
    })?;
    let mut made = Vec::new();
    for _ in 0..BACKLOG {
        match new_listener(laddr, port) {
            Ok(h) => made.push(h),
            Err(e) => {
                for h in made {
                    net::remove_tcp(h);
                }
                let _ = with_conv(p, idx, |c| {
                    c.st = St::Idle;
                    c.lport = 0;
                });
                return Err(e);
            }
        }
    }
    let stored = with_conv(p, idx, |c| {
        c.backlog = core::mem::take(&mut made);
    });
    for h in made {
        net::remove_tcp(h);
    }
    stored
}

fn udp_bind(p: Proto, idx: usize, laddr: Ipv4Address, want: u16) -> Result<(SocketHandle, u16), &'static str> {
    let port = p.table().with(|t| {
        match t.get(idx) {
            Some(Some(c)) if c.st == St::Idle => {}
            Some(Some(_)) => return Err("connection in use"),
            _ => return Err("conversation closed"),
        }
        let port = if want != 0 {
            if taken(t, want) {
                return Err("address already in use");
            }
            want
        } else {
            net::alloc_port(|q| taken(t, q)).ok_or("no free ports")?
        };
        if let Some(Some(c)) = t.get_mut(idx) {
            c.lport = port;
            c.laddr = laddr;
            c.st = St::Connecting;
        }
        Ok(port)
    })?;
    let revert = || {
        let _ = with_conv(p, idx, |c| {
            c.st = St::Idle;
            c.lport = 0;
        });
    };
    let Some(h) = net::add_udp() else {
        revert();
        return Err("no network device");
    };
    let bound = net::with_udp(h, |s| s.bind(listener_endpoint(laddr, port)).is_ok()).unwrap_or(false);
    if !bound {
        net::remove_udp(h);
        revert();
        return Err("address already in use");
    }
    Ok((h, port))
}

fn udp_connect(p: Proto, idx: usize, args: &[&str]) -> Result<(), &'static str> {
    let (raddr, rport) = parse_addr(args.first().ok_or("bad address")?)?;
    if rport == 0 || raddr == UNSPEC {
        return Err("bad address");
    }
    let want = match args.get(1) {
        Some(s) => parse_port(s)?,
        None => 0,
    };
    let (h, _) = udp_bind(p, idx, UNSPEC, want)?;
    with_conv(p, idx, |c| {
        c.handle = Some(h);
        c.raddr = raddr;
        c.rport = rport;
        c.st = St::Connected;
    })
}

fn udp_announce(p: Proto, idx: usize, args: &[&str]) -> Result<(), &'static str> {
    let (laddr, lport) = parse_addr(args.first().ok_or("bad address")?)?;
    let (h, _) = udp_bind(p, idx, laddr, lport)?;
    with_conv(p, idx, |c| {
        c.handle = Some(h);
        c.st = St::Announced;
    })
}

fn hangup(p: Proto, idx: usize) -> Result<(), &'static str> {
    let (h, st) = with_conv(p, idx, |c| {
        let r = (c.handle, c.st);
        if c.st == St::Connected || c.st == St::Announced {
            c.st = St::Closed;
        }
        r
    })?;
    match (p, h) {
        (Proto::Tcp, Some(h)) if st == St::Connected => {
            net::with_tcp(h, |s, _| s.close());
            net::poll();
        }
        (Proto::Udp, Some(h)) => {
            net::with_udp(h, |s| s.close());
        }
        _ => {}
    }
    if p == Proto::Tcp && st == St::Announced {
        let bl = with_conv(p, idx, |c| core::mem::take(&mut c.backlog))?;
        for h in bl {
            net::remove_tcp(h);
        }
    }
    Ok(())
}

fn write_ctl(o: &NetObj, text: &str) -> Result<(), &'static str> {
    let line = text.trim_matches(|c: char| c == '\n' || c == '\0' || c == '\r' || c == ' ');
    let mut words = line.split_whitespace();
    let Some(cmd) = words.next() else { return Ok(()) };
    let args: Vec<&str> = words.collect();
    let (p, i) = (o.proto, o.idx);
    match (cmd, p) {
        ("connect", Proto::Tcp) => tcp_connect(p, i, &args),
        ("connect", Proto::Udp) => udp_connect(p, i, &args),
        ("announce", Proto::Tcp) => tcp_announce(p, i, &args),
        ("announce", Proto::Udp) => udp_announce(p, i, &args),
        ("bind", _) => {
            let port = parse_port(args.first().ok_or("bad port")?)?;
            with_conv(p, i, |c| {
                if c.st != St::Idle {
                    return Err("connection in use");
                }
                c.lport = port;
                Ok(())
            })?
        }
        ("hangup", _) | ("close", _) => hangup(p, i),
        ("headers", Proto::Udp) => with_conv(p, i, |c| c.headers = true),
        ("accept", _) => Ok(()),
        ("reject", _) => hangup(p, i),
        ("keepalive", Proto::Tcp) => {
            let ms = args.first().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            let h = with_conv(p, i, |c| c.handle)?.ok_or("not connected")?;
            net::with_tcp(h, |s, _| {
                s.set_keep_alive(if ms == 0 { None } else { Some(smoltcp::time::Duration::from_millis(ms)) })
            });
            Ok(())
        }
        ("ttl", _) | ("tos", _) | ("ignoreadv", _) | ("addmulti", _) | ("remmulti", _) | ("checksum", _) => Ok(()),
        _ => Err("bad control message"),
    }
}

fn state_name(s: tcp::State) -> &'static str {
    match s {
        tcp::State::Closed => "Closed",
        tcp::State::Listen => "Listen",
        tcp::State::SynSent => "Syn_sent",
        tcp::State::SynReceived => "Syn_received",
        tcp::State::Established => "Established",
        tcp::State::FinWait1 => "Finwait1",
        tcp::State::FinWait2 => "Finwait2",
        tcp::State::CloseWait => "Close_wait",
        tcp::State::Closing => "Closing",
        tcp::State::LastAck => "Last_ack",
        tcp::State::TimeWait => "Time_wait",
    }
}

fn text_of(o: &NetObj) -> Result<String, &'static str> {
    let (p, i) = (o.proto, o.idx);
    let snap = with_conv(p, i, |c| (c.st, c.handle, c.laddr, c.lport, c.raddr, c.rport, c.backlog.len()))?;
    let (st, h, mut laddr, mut lport, mut raddr, mut rport, nback) = snap;
    match o.kind {
        Kind::Ctl => Ok(format!("{:11} ", i)),
        Kind::Local | Kind::Remote => {
            if let (Proto::Tcp, Some(h), St::Connected) = (p, h, st) {
                if let Some((l, r)) = net::with_tcp(h, |s, _| (s.local_endpoint(), s.remote_endpoint())) {
                    if let Some(l) = l {
                        (laddr, lport) = split_ep(l);
                    }
                    if let Some(r) = r {
                        (raddr, rport) = split_ep(r);
                    }
                }
            }
            if o.kind == Kind::Local {
                if laddr == UNSPEC && st == St::Connected {
                    if let Some(a) = net::local_ip() {
                        laddr = a;
                    }
                }
                Ok(format!("{}!{}\n", laddr, lport))
            } else {
                Ok(format!("{}!{}\n", raddr, rport))
            }
        }
        Kind::Status => match p {
            Proto::Tcp => match (h, st) {
                (Some(h), St::Connected | St::Closed | St::Connecting) => {
                    let r = net::with_tcp(h, |s, _| (s.state(), s.recv_queue(), s.send_queue()));
                    match r {
                        Some((s, rq, sq)) => Ok(format!("{} qin {} qout {}\n", state_name(s), rq, sq)),
                        None => Ok(String::from("Closed\n")),
                    }
                }
                _ => match st {
                    St::Announced => Ok(format!("Listen backlog {}\n", nback)),
                    _ => Ok(String::from("Closed\n")),
                },
            },
            Proto::Udp => match st {
                St::Idle | St::Closed => Ok(String::from("Closed\n")),
                _ => Ok(String::from("Open\n")),
            },
        },
        _ => Err("permission denied"),
    }
}

fn tcp_read(o: &NetObj, n: usize) -> Result<Vec<u8>, &'static str> {
    let h = handle_of(o.proto, o.idx)?;
    let mut tmp = vec![0u8; n.min(TCP_CHUNK)];
    if tmp.is_empty() {
        return Ok(tmp);
    }
    let k = wait_on(|| {
        net::with_tcp(h, |s, _| {
            if s.can_recv() {
                Some(Ok(s.recv_slice(&mut tmp).unwrap_or(0)))
            } else if !s.may_recv() {
                Some(Ok(0))
            } else {
                None
            }
        })
        .unwrap_or(Some(Err("no network device")))
    })?;
    tmp.truncate(k);
    Ok(tmp)
}

fn tcp_write(o: &NetObj, data: &[u8]) -> Result<usize, &'static str> {
    let h = handle_of(o.proto, o.idx)?;
    let mut done = 0usize;
    while done < data.len() {
        let k = wait_on(|| {
            net::with_tcp(h, |s, _| {
                if !s.may_send() {
                    Some(Err("connection closed"))
                } else if s.can_send() {
                    match s.send_slice(&data[done..]) {
                        Ok(k) if k > 0 => Some(Ok(k)),
                        Ok(_) => None,
                        Err(_) => Some(Err("connection closed")),
                    }
                } else {
                    None
                }
            })
            .unwrap_or(Some(Err("no network device")))
        });
        match k {
            Ok(k) => done += k,
            Err(e) => return if done > 0 { Ok(done) } else { Err(e) },
        }
        net::poll();
    }
    Ok(done)
}

fn mapped(a: Ipv4Address) -> [u8; 16] {
    let mut v = [0u8; 16];
    v[10] = 0xff;
    v[11] = 0xff;
    v[12..16].copy_from_slice(&a.octets());
    v
}

fn udp_read(o: &NetObj, n: usize) -> Result<Vec<u8>, &'static str> {
    let (p, i) = (o.proto, o.idx);
    let (h, st, headers, raddr, rport) = with_conv(p, i, |c| (c.handle, c.st, c.headers, c.raddr, c.rport))?;
    let h = h.ok_or("not connected")?;
    let connected = st == St::Connected;
    let ifc = net::local_ip().unwrap_or(UNSPEC);
    let lport = with_conv(p, i, |c| c.lport)?;
    let mut tmp = vec![0u8; UDP_CHUNK];
    let got = wait_on(|| {
        let r = net::with_udp(h, |s| loop {
            match s.recv_slice(&mut tmp) {
                Ok((k, meta)) => {
                    if connected && meta.endpoint != endpoint(raddr, rport) {
                        continue;
                    }
                    return Some(Ok((k, meta.endpoint, meta.local_address)));
                }
                Err(_) => return None,
            }
        });
        match r {
            Some(Some(v)) => Some(v),
            Some(None) => None,
            None => Some(Err("no network device")),
        }
    })?;
    let (k, from, to) = got;
    let mut out = Vec::new();
    if headers {
        let (fa, fp) = split_ep(from);
        let la = match to {
            Some(IpAddress::Ipv4(a)) => a,
            None => ifc,
        };
        out.extend_from_slice(&mapped(fa));
        out.extend_from_slice(&mapped(la));
        out.extend_from_slice(&mapped(ifc));
        out.extend_from_slice(&fp.to_be_bytes());
        out.extend_from_slice(&lport.to_be_bytes());
    }
    out.extend_from_slice(&tmp[..k]);
    out.truncate(n);
    Ok(out)
}

fn udp_write(o: &NetObj, data: &[u8]) -> Result<usize, &'static str> {
    let (p, i) = (o.proto, o.idx);
    let (h, st, headers, raddr, rport) = with_conv(p, i, |c| (c.handle, c.st, c.headers, c.raddr, c.rport))?;
    let h = h.ok_or("not connected")?;
    let (dst, payload) = if headers {
        if data.len() < UDPHDR {
            return Err("short write");
        }
        let a = Ipv4Address::new(data[12], data[13], data[14], data[15]);
        let port = u16::from_be_bytes([data[48], data[49]]);
        (endpoint(a, port), &data[UDPHDR..])
    } else {
        if st != St::Connected {
            return Err("not connected");
        }
        (endpoint(raddr, rport), data)
    };
    wait_on(|| {
        let r = net::with_udp(h, |s| match s.send_slice(payload, dst) {
            Ok(()) => Some(Ok(())),
            Err(udp::SendError::BufferFull) => None,
            Err(udp::SendError::Unaddressable) => Some(Err("no route to host")),
        });
        match r {
            Some(v) => v,
            None => Some(Err("no network device")),
        }
    })?;
    net::poll();
    Ok(data.len())
}

fn put_text(o: &NetObj, text: &str, buf: u64, n: u64, off: u64) -> i64 {
    let explicit = off != crate::io::CURRENT_OFFSET;
    let start = if explicit { off } else { o.pos.with(|p| *p) } as usize;
    let t = text.as_bytes();
    if start >= t.len() {
        return 0;
    }
    let k = (n.min(MAX_IO) as usize).min(t.len() - start);
    if !sched::copy_to_user(buf, &t[start..start + k]) {
        return crate::io::fail("bad address in system call");
    }
    if !explicit {
        o.pos.with(|p| *p += k as u64);
    }
    k as i64
}

pub fn read(o: &NetObj, buf: u64, n: u64, off: u64) -> i64 {
    match o.kind {
        Kind::Data => {
            let n = n.min(MAX_IO) as usize;
            if n == 0 {
                return 0;
            }
            let r = match o.proto {
                Proto::Tcp => tcp_read(o, n),
                Proto::Udp => udp_read(o, n),
            };
            match r {
                Ok(d) => {
                    if !d.is_empty() && !sched::copy_to_user(buf, &d) {
                        return crate::io::fail("bad address in system call");
                    }
                    d.len() as i64
                }
                Err(m) => crate::io::fail(m),
            }
        }
        Kind::Listen => crate::io::fail("permission denied"),
        _ => match text_of(o) {
            Ok(t) => put_text(o, &t, buf, n, off),
            Err(m) => crate::io::fail(m),
        },
    }
}

pub fn write(o: &NetObj, buf: u64, n: u64) -> i64 {
    if !o.kind.writable() || o.kind == Kind::Listen {
        return crate::io::fail("permission denied");
    }
    let n = n.min(MAX_IO) as usize;
    let mut data = vec![0u8; n];
    if !sched::copy_from_user(buf, &mut data) {
        return crate::io::fail("bad address in system call");
    }
    let r = match o.kind {
        Kind::Ctl => match core::str::from_utf8(&data) {
            Ok(t) => write_ctl(o, t).map(|_| n),
            Err(_) => Err("bad control message"),
        },
        _ => match o.proto {
            Proto::Tcp => tcp_write(o, &data),
            Proto::Udp => udp_write(o, &data),
        },
    };
    match r {
        Ok(k) => k as i64,
        Err(m) => crate::io::fail(m),
    }
}

const SERVICES: [(&str, u16); 24] = [
    ("echo", 7),
    ("discard", 9),
    ("daytime", 13),
    ("ftp", 21),
    ("ssh", 22),
    ("telnet", 23),
    ("smtp", 25),
    ("time", 37),
    ("domain", 53),
    ("dns", 53),
    ("tftp", 69),
    ("gopher", 70),
    ("http", 80),
    ("www", 80),
    ("pop3", 110),
    ("ntp", 123),
    ("imap", 143),
    ("imap4", 143),
    ("https", 443),
    ("rexexec", 512),
    ("9fs", 564),
    ("9pfs", 564),
    ("irc", 6667),
    ("ircd", 6667),
];

fn service(s: &str) -> Result<u16, &'static str> {
    if let Ok(p) = s.parse::<u16>() {
        return Ok(p);
    }
    SERVICES.iter().find(|(n, _)| *n == s).map(|(_, p)| *p).ok_or("unknown service")
}

fn resolve_host(host: &str) -> Result<Ipv4Address, &'static str> {
    if host.is_empty() || host == "*" {
        return Ok(UNSPEC);
    }
    if let Ok(a) = host.parse::<Ipv4Address>() {
        return Ok(a);
    }
    let (h, q) = net::dns_start(host)?;
    let limit = Deadline::after_ms(DNS_MS);
    let r = wait_on(|| match net::dns_result(h, q) {
        Some(r) => Some(r),
        None if limit.expired() => Some(Err("dns lookup timed out")),
        None => None,
    });
    net::dns_done(h);
    r
}

fn translate(query: &str) -> Result<String, &'static str> {
    let line = query.trim_matches(|c: char| c == '\n' || c == '\0' || c == '\r' || c == ' ');
    let parts: Vec<&str> = line.split('!').collect();
    let (proto, host, svc) = match parts.len() {
        2 => (parts[0], "*", parts[1]),
        3 => (parts[0], parts[1], parts[2]),
        _ => return Err("bad dial string"),
    };
    let proto = match proto {
        "net" | "tcp" => "tcp",
        "udp" => "udp",
        _ => return Err("unknown network"),
    };
    let port = service(svc)?;
    let ip = resolve_host(host)?;
    Ok(format!("/net/{}/clone {}!{}\n", proto, ip, port))
}

pub fn cs_read(o: &CsObj, buf: u64, n: u64) -> i64 {
    let line = o.answers.with(|a| {
        let next = a.0.get(a.1).cloned();
        if next.is_some() {
            a.1 += 1;
        }
        next
    });
    let Some(line) = line else { return 0 };
    let k = (n.min(MAX_IO) as usize).min(line.len());
    if !sched::copy_to_user(buf, &line.as_bytes()[..k]) {
        return crate::io::fail("bad address in system call");
    }
    k as i64
}

pub fn cs_write(o: &CsObj, buf: u64, n: u64) -> i64 {
    let n = n.min(MAX_IO) as usize;
    let mut data = vec![0u8; n];
    if !sched::copy_from_user(buf, &mut data) {
        return crate::io::fail("bad address in system call");
    }
    let Ok(text) = core::str::from_utf8(&data) else { return crate::io::fail("bad dial string") };
    match translate(text) {
        Ok(ans) => {
            o.answers.with(|a| *a = (vec![ans], 0));
            n as i64
        }
        Err(m) => crate::io::fail(m),
    }
}

pub fn selftest() {
    if !net::active() {
        return;
    }
    let mut ok = true;
    let mut check = |name: &str, good: bool| {
        if !good {
            kprintln!("ifsy: netfs: {} {}", name, failed());
        }
        ok &= good;
    };
    let c = alloc(Proto::Tcp);
    check("tcp clone", c.is_ok());
    if let Ok(h) = c {
        let o = obj(h, Kind::Ctl);
        check("tcp announce", write_ctl(&o, "announce 7001").is_ok());
        let st = obj(acquire(Proto::Tcp, o.idx).unwrap_or(Hold { proto: Proto::Tcp, idx: usize::MAX }), Kind::Status);
        check("tcp status Listen", text_of(&st).map(|t| t.starts_with("Listen")).unwrap_or(false));
        let l = obj(acquire(Proto::Tcp, o.idx).unwrap_or(Hold { proto: Proto::Tcp, idx: usize::MAX }), Kind::Local);
        check("tcp local", text_of(&l).map(|t| t.ends_with("!7001\n")).unwrap_or(false));
        let c2 = alloc(Proto::Tcp);
        if let Ok(h2) = c2 {
            let o2 = obj(h2, Kind::Ctl);
            check("tcp port in use", write_ctl(&o2, "announce 7001") == Err("address already in use"));
        }
        check("tcp hangup", write_ctl(&o, "hangup").is_ok());
    }
    let u = alloc(Proto::Udp);
    check("udp clone", u.is_ok());
    if let Ok(h) = u {
        let o = obj(h, Kind::Ctl);
        check("udp announce", write_ctl(&o, "announce 7002").is_ok());
        check("udp headers", write_ctl(&o, "headers").is_ok());
        check("bad ctl", write_ctl(&o, "frobnicate") == Err("bad control message"));
    }
    let leftover = TCP.with(|t| t.iter().flatten().count()) + UDP.with(|t| t.iter().flatten().count());
    check("conversations released", leftover == 0);
    if ok {
        kprintln!("ifsy: netfs: /net tree smoke test {}", pass());
    } else {
        kprintln!("ifsy: netfs: /net tree smoke test {}", warn());
    }
}
