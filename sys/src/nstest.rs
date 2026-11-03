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

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_char;

use crate::console::color::{failed, pass};
use crate::io;
use crate::mm::aspace::AddressSpace;
use crate::sched;
use crate::syscall::{self, dispatch_args};
use crate::{kprintln, mm};

const ME: sched::Tid = sched::BOOT_TID;
const DMDIR: u64 = 0x8000_0000;
const MREPL: u64 = 0;
const MAFTER: u64 = 2;
const MCREATE: u64 = 4;
const RFNAMEG: u64 = 1;
const RFCNAMEG: u64 = 1 << 10;

struct Report {
    pass: u32,
    fail: u32,
}

impl Report {
    fn check(&mut self, name: &str, cond: bool) {
        if cond {
            self.pass += 1;
            kprintln!("ifsy: ns: {} {}", name, pass());
        } else {
            self.fail += 1;
            kprintln!("ifsy: ns: {} {}", name, failed());
        }
    }
}

fn sys(n: u32, a: &[u64]) -> i64 {
    let mut args = [0u64; 6];
    args[..a.len()].copy_from_slice(a);
    dispatch_args(n, &args)
}

fn put(va: u64, bytes: &[u8]) -> bool {
    sched::with_aspace(ME, |a| a.write_bytes(va, bytes).is_ok()).unwrap_or(false)
}

fn get(va: u64, n: usize) -> Vec<u8> {
    sched::with_aspace(ME, |a| {
        let mut b = vec![0u8; n];
        a.read_bytes(va, &mut b).ok().map(|_| b)
    })
    .flatten()
    .unwrap_or_default()
}

fn text(va: u64, n: usize) -> String {
    String::from_utf8_lossy(&get(va, n)).into_owned()
}

fn errstr(buf: u64) -> String {
    let _ = sys(syscall::ERRSTR, &[buf, 128]);
    let b = get(buf, 128);
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

struct Env {
    h: u64,
}

impl Env {
    fn path(&self, slot: u64, s: &str) -> u64 {
        let va = self.h + slot * 0x40;
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        put(va, &v);
        va
    }

    fn buf(&self) -> u64 {
        self.h + 0x1000
    }

    fn scratch(&self) -> u64 {
        self.h + 0x800
    }

    fn open(&self, p: &str, mode: u64) -> i64 {
        let va = self.path(1, p);
        sys(syscall::OPEN, &[va, mode])
    }

    fn read_all(&self, p: &str) -> Option<Vec<u8>> {
        let fd = self.open(p, 0);
        if fd < 0 {
            return None;
        }
        let n = sys(syscall::_READ, &[fd as u64, self.buf(), 1024]);
        sys(syscall::CLOSE, &[fd as u64]);
        if n < 0 { None } else { Some(get(self.buf(), n as usize)) }
    }

    fn read_text(&self, p: &str) -> Option<String> {
        self.read_all(p).map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    fn entries(&self, p: &str) -> Vec<String> {
        let Some(b) = self.read_all(p) else { return Vec::new() };
        let mut out = Vec::new();
        let mut i = 0;
        while i + 2 <= b.len() {
            let l = 2 + u16::from_le_bytes([b[i], b[i + 1]]) as usize;
            if i + l > b.len() || l < 45 {
                break;
            }
            let nl = u16::from_le_bytes([b[i + 41], b[i + 42]]) as usize;
            out.push(String::from_utf8_lossy(&b[i + 43..i + 43 + nl]).into_owned());
            i += l;
        }
        out
    }

    fn mkdir(&self, p: &str) {
        let va = self.path(1, p);
        let fd = sys(syscall::CREATE, &[va, 0, DMDIR | 0o755]);
        if fd >= 0 {
            sys(syscall::CLOSE, &[fd as u64]);
        }
    }

    fn rm(&self, p: &str) -> i64 {
        let va = self.path(1, p);
        sys(syscall::REMOVE, &[va])
    }

    fn bind(&self, name: &str, old: &str, flag: u64) -> i64 {
        let a = self.path(2, name);
        let b = self.path(3, old);
        sys(syscall::BIND, &[a, b, flag])
    }

    fn unmount(&self, name: Option<&str>, old: &str) -> i64 {
        let a = name.map_or(0, |n| self.path(2, n));
        let b = self.path(3, old);
        sys(syscall::UNMOUNT, &[a, b])
    }

    fn mount(&self, fd: i64, old: &str, flag: u64) -> i64 {
        let b = self.path(3, old);
        let spec = self.path(4, "");
        sys(syscall::MOUNT, &[fd as u64, u64::MAX, b, flag, spec])
    }
}

fn wstat_name(name: &str) -> Vec<u8> {
    let mut d = p9fcall::Dir::new();
    d.typ = u16::MAX;
    d.dev = u32::MAX;
    d.qid = p9fcall::Qid { path: u64::MAX, vers: u32::MAX, typ: u8::MAX };
    d.mode = u32::MAX;
    d.atime = u32::MAX;
    d.mtime = u32::MAX;
    d.length = -1;
    let mut nm = String::from(name).into_bytes();
    nm.push(0);
    let empty = [0u8; 1];
    d.name = nm.as_ptr() as *const c_char;
    d.uid = empty.as_ptr() as *const c_char;
    d.gid = empty.as_ptr() as *const c_char;
    d.muid = empty.as_ptr() as *const c_char;
    let mut b = vec![0u8; p9fcall::size_d2m(&d)];
    let k = p9fcall::conv_d2m(&d, &mut b);
    b.truncate(k);
    b
}

fn segments(r: &mut Report, e: &Env) {
    let va = sys(syscall::SEGATTACH, &[0, e.path(5, "memory"), 0, 8192]);
    let mem_ok = va > 0 && put(va as u64, b"private") && text(va as u64, 7) == "private";
    r.check("segattach memory maps zeroed private pages", mem_ok && get(va as u64 + 4096, 4) == vec![0u8; 4]);
    r.check("segdetach unmaps a memory segment", sys(syscall::SEGDETACH, &[va as u64]) == 0 && get(va as u64, 1).is_empty());

    let m3 = sys(syscall::SEGATTACH, &[0, e.path(5, "memory"), 0, 3 * 4096]);
    let freed = sys(syscall::SEGFREE, &[m3 as u64 + 4096, 4096]);
    r.check(
        "segfree releases pages in the middle of a segment",
        m3 > 0 && freed == 0 && get(m3 as u64 + 4096, 1).is_empty() && !get(m3 as u64, 1).is_empty() && !get(m3 as u64 + 8192, 1).is_empty(),
    );
    sys(syscall::SEGDETACH, &[m3 as u64]);
    sys(syscall::SEGDETACH, &[m3 as u64 + 8192]);

    let s1 = sys(syscall::SEGATTACH, &[0, e.path(5, "nstest-seg"), 0, 4096]);
    let s2 = sys(syscall::SEGATTACH, &[0, e.path(5, "nstest-seg"), 0, 4096]);
    let both = s1 > 0 && s2 > 0 && s1 != s2;
    put(s1 as u64, b"shared!");
    r.check("a named segment attached twice shares its pages", both && text(s2 as u64, 7) == "shared!");

    let child: Option<AddressSpace> = sched::with_aspace(ME, |a| a.duplicate().ok()).flatten();
    let mut shared_fork = false;
    if let Some(mut c) = child {
        let mut b = [0u8; 7];
        let saw = c.read_bytes(s1 as u64, &mut b).is_ok() && &b == b"shared!";
        let wrote = c.write_bytes(s1 as u64, b"forked!").is_ok();
        shared_fork = saw && wrote && text(s2 as u64, 7) == "forked!";
    }
    r.check("shared segments stay shared across a copying rfork", shared_fork);
    r.check(
        "segdetach releases shared attachments",
        sys(syscall::SEGDETACH, &[s1 as u64]) == 0 && sys(syscall::SEGDETACH, &[s2 as u64]) == 0,
    );

    let anon = sys(syscall::SEGATTACH, &[0, e.path(5, "shared"), 0, 4096]);
    r.check("segattach shared makes an anonymous shared segment", anon > 0 && put(anon as u64, b"x") && sys(syscall::SEGDETACH, &[anon as u64]) == 0);

    let f1 = sys(syscall::SEGFLUSH, &[s1 as u64, 16]);
    let m4 = sys(syscall::SEGATTACH, &[0, e.path(5, "memory"), 0, 4096]);
    let f2 = sys(syscall::SEGFLUSH, &[m4 as u64, 16]);
    let f3 = sys(syscall::SEGFLUSH, &[0x10, 4]);
    sys(syscall::SEGDETACH, &[m4 as u64]);
    r.check("segflush accepts mapped ranges and rejects unmapped ones", f1 == -1 && f2 == 0 && f3 == -1);

    let bad = sys(syscall::SEGATTACH, &[0, e.path(5, "physical"), 0, 4096]);
    r.check("segattach refuses physical segments", bad == -1 && errstr(e.scratch()) == "permission denied");

    r.check("sysr1 returns 0", sys(syscall::SYSR1, &[]) == 0);
    let fs = sys(syscall::_FSESSION, &[0, e.scratch()]);
    r.check("_fsession fails as obsolete", fs == -1 && errstr(e.scratch()).contains("fversion"));
}

fn mount_and_bind(r: &mut Report, e: &Env) {
    e.mkdir("/nsa");
    e.mkdir("/nsb");

    let fd_auth = io::install(crate::p9srv::client());
    let fa = sys(syscall::FAUTH, &[fd_auth as u64, e.path(4, "")]);
    r.check("fauth reports a server that needs no authentication", fa == -1 && errstr(e.scratch()) == "no authentication required");
    sys(syscall::CLOSE, &[fd_auth as u64]);

    let fd = io::install(crate::p9srv::client());
    let vbuf = e.h + 0x1800;
    put(vbuf, b"9P2000\0");
    let k = sys(syscall::FVERSION, &[fd as u64, 8192, vbuf, 64]);
    r.check("fversion negotiates 9P2000 with the server", k == 6 && text(vbuf, 6) == "9P2000");

    let m = e.mount(fd, "/nsa", MREPL | MCREATE);
    sys(syscall::CLOSE, &[fd as u64]);
    r.check("mount attaches the server at /nsa and survives closing the fd", m == 0);

    r.check("read of a mounted file goes over 9P", e.read_text("/nsa/hello").as_deref() == Some("hello from 9p\n"));
    r.check("a file two directories down is reachable", e.read_text("/nsa/sub/deep").as_deref() == Some("deep file"));

    let mut names = e.entries("/nsa");
    names.sort();
    r.check("reading a mounted directory returns the server's stat entries", names == ["hello", "sub"]);

    let st = e.buf() + 0x400;
    let sr = sys(syscall::_STAT, &[e.path(1, "/nsa/hello"), st]);
    let sb = get(st, 116);
    let nm_ok = sb.len() == 116 && &sb[..6] == b"hello\0";
    let len = if sb.len() == 116 { u32::from_le_bytes([sb[104], sb[105], sb[106], sb[107]]) } else { 0 };
    r.check("_stat of a remote file returns the 116-byte record", sr == 0 && nm_ok && len == 14);

    let fd2 = e.open("/nsa/hello", 0);
    let o = sys(syscall::OSEEK, &[fd2 as u64, 6, 0]);
    let n = sys(syscall::_READ, &[fd2 as u64, e.buf(), 4]);
    sys(syscall::CLOSE, &[fd2 as u64]);
    r.check("oseek repositions a remote file", o == 6 && n == 4 && text(e.buf(), 4) == "from");

    let cp = e.path(1, "/nsa/new.txt");
    let cfd = sys(syscall::CREATE, &[cp, 2, 0o644]);
    put(e.buf(), b"abc");
    let w = sys(syscall::_WRITE, &[cfd as u64, e.buf(), 3]);
    sys(syscall::CLOSE, &[cfd as u64]);
    r.check("create and write on a mounted directory", cfd >= 0 && w == 3 && e.read_text("/nsa/new.txt").as_deref() == Some("abc"));

    let wb = wstat_name("renamed.txt");
    put(e.buf() + 0x200, &wb);
    let ws = sys(syscall::WSTAT, &[e.path(1, "/nsa/new.txt"), e.buf() + 0x200, wb.len() as u64]);
    r.check(
        "wstat renames a remote file",
        ws == 0 && e.read_text("/nsa/renamed.txt").as_deref() == Some("abc") && e.read_text("/nsa/new.txt").is_none(),
    );
    r.check("remove deletes a remote file", e.rm("/nsa/renamed.txt") == 0 && e.read_text("/nsa/renamed.txt").is_none());

    r.check("bind replaces a directory with another subtree", e.bind("/nsa/sub", "/nsb", MREPL) == 0 && e.read_text("/nsb/deep").as_deref() == Some("deep file"));
    r.check("bind after builds a union directory", e.bind("/nsa", "/nsb", MAFTER) == 0 && {
        let mut u = e.entries("/nsb");
        u.sort();
        u == ["deep", "hello", "sub"]
    });
    r.check("lookup in a union finds the first member that has the name", e.read_text("/nsb/hello").as_deref() == Some("hello from 9p\n"));

    let cr = sys(syscall::CREATE, &[e.path(1, "/nsb/x"), 2, 0o644]);
    r.check("create in a union without MCREATE is refused", cr == -1 && errstr(e.scratch()) == "mounted directory forbids creation");

    r.check("unmount removes one member of a union", e.unmount(Some("/nsa"), "/nsb") == 0 && e.entries("/nsb") == ["deep"]);
    r.check("unmount of a name that is not in the union fails", e.unmount(Some("/nsa"), "/nsb") == -1 && errstr(e.scratch()) == "not in union");
    r.check("unmount with no name drops the whole union", e.unmount(None, "/nsb") == 0 && e.read_text("/nsb/deep").is_none());

    let before = crate::p9srv::node_count();
    r.check("the server saw the changes", before == 4);

    let rf = sys(syscall::RFORK, &[RFNAMEG]);
    r.check("rfork RFNAMEG gives a private copy that still sees the mount", rf == 0 && e.read_text("/nsa/hello").is_some());
    let rc = sys(syscall::RFORK, &[RFCNAMEG]);
    r.check("rfork RFCNAMEG starts from an empty namespace", rc == 0 && e.read_text("/nsa/hello").is_none());

    r.check("unmounting something not mounted fails", e.unmount(None, "/nsa") == -1 && errstr(e.scratch()) == "not mounted");
    e.rm("/nsa");
    e.rm("/nsb");
}

fn symlinks_through_ns(r: &mut Report, e: &Env) {
    e.mkdir("/lnx");
    e.mkdir("/lny");
    let cfd = sys(syscall::CREATE, &[e.path(1, "/lnx/real"), 2, 0o644]);
    put(e.buf(), b"data");
    sys(syscall::_WRITE, &[cfd as u64, e.buf(), 4]);
    sys(syscall::CLOSE, &[cfd as u64]);

    let made = crate::fs::make_symlink("/lny/real", "/lnfile").is_ok() && crate::fs::make_symlink("/lny", "/lndir").is_ok();
    r.check("absolute symbolic links can be created", made);
    r.check("an absolute link to a name only a bind provides fails before the bind", e.read_text("/lnfile").is_none());

    r.check("bind /lnx over /lny", e.bind("/lnx", "/lny", MREPL) == 0);
    r.check("an absolute link is resolved through the namespace", e.read_text("/lnfile").as_deref() == Some("data"));
    r.check("a directory link in the middle of a path is resolved through the namespace", e.read_text("/lndir/real").as_deref() == Some("data"));

    r.check("unmounting the bind makes the link dangle again", e.unmount(None, "/lny") == 0 && e.read_text("/lnfile").is_none());

    let loopy = crate::fs::make_symlink("/lnloop", "/lnloop").is_ok();
    let _ = e.read_text("/lnloop");
    r.check("a link to itself ends after the hop limit instead of looping", loopy);

    for p in ["/lnfile", "/lndir", "/lnloop", "/lnx/real", "/lnx", "/lny"] {
        e.rm(p);
    }
}

pub fn selftest() {
    let mut r = Report { pass: 0, fail: 0 };
    let free0 = mm::frame_stats().1;
    let Ok(a) = mm::new_address_space() else {
        kprintln!("ifsy: ns: skipped: no address space");
        return;
    };
    if !sched::attach_aspace(ME, a) {
        return;
    }
    let h = sched::with_aspace(ME, |a| {
        let h = a.brk(0).ok()?;
        a.brk(h + 4 * 4096).ok()?;
        Some(h)
    })
    .flatten();
    let Some(h) = h else {
        kprintln!("ifsy: ns: skipped: could not build a user heap");
        return;
    };
    let e = Env { h };
    segments(&mut r, &e);
    mount_and_bind(&mut r, &e);
    symlinks_through_ns(&mut r, &e);
    sched::detach_aspace(ME);
    r.check("the namespace tests leak no frames", mm::frame_stats().1 == free0 || mm::frame_stats().1 + 8 >= free0);
    let tag = if r.fail > 0 { failed() } else { pass() };
    kprintln!("ifsy: ns: {} pass, {} fail {}", r.pass, r.fail, tag);
}
