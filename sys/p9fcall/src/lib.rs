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

#![cfg_attr(not(test), no_std)]

use core::ffi::c_char;
use core::mem::ManuallyDrop;

pub const VERSION9P: &str = "9P2000";
pub const MAXWELEM: usize = 16;
pub const BIT8SZ: usize = 1;
pub const BIT16SZ: usize = 2;
pub const BIT32SZ: usize = 4;
pub const BIT64SZ: usize = 8;
pub const QIDSZ: usize = BIT8SZ + BIT32SZ + BIT64SZ;
pub const STATFIXLEN: usize = BIT16SZ + QIDSZ + 5 * BIT16SZ + 4 * BIT32SZ + BIT64SZ;
pub const NOTAG: u16 = !0;
pub const NOFID: u32 = !0;
pub const IOHDRSZ: usize = 24;

pub const TVERSION: u8 = 100;
pub const RVERSION: u8 = 101;
pub const TAUTH: u8 = 102;
pub const RAUTH: u8 = 103;
pub const TATTACH: u8 = 104;
pub const RATTACH: u8 = 105;
pub const RERROR: u8 = 107;
pub const TFLUSH: u8 = 108;
pub const RFLUSH: u8 = 109;
pub const TWALK: u8 = 110;
pub const RWALK: u8 = 111;
pub const TOPEN: u8 = 112;
pub const ROPEN: u8 = 113;
pub const TCREATE: u8 = 114;
pub const RCREATE: u8 = 115;
pub const TREAD: u8 = 116;
pub const RREAD: u8 = 117;
pub const TWRITE: u8 = 118;
pub const RWRITE: u8 = 119;
pub const TCLUNK: u8 = 120;
pub const RCLUNK: u8 = 121;
pub const TREMOVE: u8 = 122;
pub const RREMOVE: u8 = 123;
pub const TSTAT: u8 = 124;
pub const RSTAT: u8 = 125;
pub const TWSTAT: u8 = 126;
pub const RWSTAT: u8 = 127;
pub const TMAX: u8 = 128;

pub const QTDIR: u8 = 0x80;
pub const QTAPPEND: u8 = 0x40;
pub const QTEXCL: u8 = 0x20;
pub const QTMOUNT: u8 = 0x10;
pub const QTAUTH: u8 = 0x08;
pub const QTFILE: u8 = 0x00;

pub const DMDIR: u32 = 0x8000_0000;
pub const DMAPPEND: u32 = 0x4000_0000;
pub const DMEXCL: u32 = 0x2000_0000;
pub const DMMOUNT: u32 = 0x1000_0000;
pub const DMAUTH: u32 = 0x0800_0000;

pub const OREAD: u8 = 0;
pub const OWRITE: u8 = 1;
pub const ORDWR: u8 = 2;
pub const OEXEC: u8 = 3;
pub const OTRUNC: u8 = 16;
pub const ORCLOSE: u8 = 64;

pub const MORDER: u32 = 0x0003;
pub const MREPL: u32 = 0x0000;
pub const MBEFORE: u32 = 0x0001;
pub const MAFTER: u32 = 0x0002;
pub const MCREATE: u32 = 0x0004;
pub const MCACHE: u32 = 0x0010;
pub const MMASK: u32 = 0x0017;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Qid {
    pub path: u64,
    pub vers: u32,
    pub typ: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VersionBody {
    pub msize: u32,
    pub version: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlushBody {
    pub oldtag: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ErrorBody {
    pub ename: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OpenedBody {
    pub qid: Qid,
    pub iounit: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AuthQidBody {
    pub aqid: Qid,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AttachBody {
    pub afid: u32,
    pub uname: *const c_char,
    pub aname: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CreateBody {
    pub perm: u32,
    pub name: *const c_char,
    pub mode: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WalkBody {
    pub newfid: u32,
    pub nwname: u16,
    pub wname: [*const c_char; MAXWELEM],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WalkQidBody {
    pub nwqid: u16,
    pub wqid: [Qid; MAXWELEM],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoBody {
    pub offset: i64,
    pub count: u32,
    pub data: *const c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct StatBody {
    pub nstat: u16,
    pub stat: *const u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union Body {
    pub version: VersionBody,
    pub flush: FlushBody,
    pub error: ErrorBody,
    pub opened: OpenedBody,
    pub authqid: AuthQidBody,
    pub attach: AttachBody,
    pub create: CreateBody,
    pub walk: WalkBody,
    pub walkqid: WalkQidBody,
    pub io: IoBody,
    pub stat: StatBody,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Fcall {
    pub typ: u8,
    pub fid: u32,
    pub tag: u16,
    pub body: Body,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dir {
    pub typ: u16,
    pub dev: u32,
    pub qid: Qid,
    pub mode: u32,
    pub atime: u32,
    pub mtime: u32,
    pub length: i64,
    pub name: *const c_char,
    pub uid: *const c_char,
    pub gid: *const c_char,
    pub muid: *const c_char,
}

unsafe extern "C" {
    #[link_name = "convM2S"]
    fn c_conv_m2s(buf: *mut u8, n: u32, f: *mut Fcall) -> u32;
    #[link_name = "convS2M"]
    fn c_conv_s2m(f: *const Fcall, buf: *mut u8, n: u32) -> u32;
    #[link_name = "sizeS2M"]
    fn c_size_s2m(f: *const Fcall) -> u32;
    #[link_name = "statcheck"]
    fn c_statcheck(buf: *mut u8, n: u32) -> i32;
    #[link_name = "convM2D"]
    fn c_conv_m2d(buf: *mut u8, n: u32, d: *mut Dir, strs: *mut c_char) -> u32;
    #[link_name = "convD2M"]
    fn c_conv_d2m(d: *const Dir, buf: *mut u8, n: u32) -> u32;
    #[link_name = "sizeD2M"]
    fn c_size_d2m(d: *const Dir) -> u32;
    fn p9_sizeof_fcall() -> usize;
}

impl Fcall {
    pub fn new(typ: u8) -> Fcall {
        let mut f: Fcall = unsafe { core::mem::zeroed() };
        f.typ = typ;
        f
    }
}

impl Dir {
    pub fn new() -> Dir {
        unsafe { core::mem::zeroed() }
    }
}

impl Default for Dir {
    fn default() -> Dir {
        Dir::new()
    }
}

pub fn layout_ok() -> bool {
    unsafe { p9_sizeof_fcall() == core::mem::size_of::<Fcall>() }
}

pub fn conv_m2s(buf: &mut [u8], f: &mut Fcall) -> usize {
    unsafe { c_conv_m2s(buf.as_mut_ptr(), buf.len() as u32, f) as usize }
}

pub fn conv_s2m(f: &Fcall, buf: &mut [u8]) -> usize {
    unsafe { c_conv_s2m(f, buf.as_mut_ptr(), buf.len() as u32) as usize }
}

pub fn size_s2m(f: &Fcall) -> usize {
    unsafe { c_size_s2m(f) as usize }
}

pub fn stat_check(buf: &mut [u8]) -> bool {
    unsafe { c_statcheck(buf.as_mut_ptr(), buf.len() as u32) == 0 }
}

pub fn conv_m2d(buf: &mut [u8], d: &mut Dir, strs: &mut [u8]) -> usize {
    unsafe { c_conv_m2d(buf.as_mut_ptr(), buf.len() as u32, d, strs.as_mut_ptr() as *mut c_char) as usize }
}

pub fn conv_d2m(d: &Dir, buf: &mut [u8]) -> usize {
    unsafe { c_conv_d2m(d, buf.as_mut_ptr(), buf.len() as u32) as usize }
}

pub fn size_d2m(d: &Dir) -> usize {
    unsafe { c_size_d2m(d) as usize }
}

pub unsafe fn cstr<'a>(p: *const c_char) -> &'a str {
    if p.is_null() {
        return "";
    }
    let mut n = 0;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::str::from_utf8(core::slice::from_raw_parts(p as *const u8, n)).unwrap_or("")
    }
}

pub fn gbit16(p: &[u8]) -> u16 {
    u16::from_le_bytes([p[0], p[1]])
}

pub fn gbit32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

pub fn gbit64(p: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&p[..8]);
    u64::from_le_bytes(b)
}

pub fn pbit16(p: &mut [u8], v: u16) {
    p[..2].copy_from_slice(&v.to_le_bytes());
}

pub fn pbit32(p: &mut [u8], v: u32) {
    p[..4].copy_from_slice(&v.to_le_bytes());
}

#[allow(dead_code)]
fn _keep(_: ManuallyDrop<u8>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        assert!(layout_ok());
        assert_eq!(core::mem::size_of::<Fcall>(), 280);
        assert_eq!(core::mem::size_of::<Dir>(), 80);
        assert_eq!(STATFIXLEN, 49);
    }

    #[test]
    fn tversion_vector() {
        let mut f = Fcall::new(TVERSION);
        f.tag = NOTAG;
        let v = b"9P2000\0";
        f.body.version = VersionBody { msize: 8192, version: v.as_ptr() as *const c_char };
        let mut buf = [0u8; 64];
        let n = conv_s2m(&f, &mut buf);
        assert_eq!(n, 19);
        assert_eq!(&buf[..n], &[19, 0, 0, 0, 100, 0xff, 0xff, 0x00, 0x20, 0, 0, 6, 0, b'9', b'P', b'2', b'0', b'0', b'0']);
        let mut g = Fcall::new(0);
        let mut copy = buf[..n].to_vec();
        assert_eq!(conv_m2s(&mut copy, &mut g), n);
        assert_eq!(g.typ, TVERSION);
        assert_eq!(g.tag, NOTAG);
        unsafe {
            assert_eq!(g.body.version.msize, 8192);
            assert_eq!(cstr(g.body.version.version), "9P2000");
        }
    }

    #[test]
    fn walk_roundtrip() {
        let a = b"usr\0";
        let b = b"glenda\0";
        let mut f = Fcall::new(TWALK);
        f.tag = 7;
        f.fid = 3;
        let mut w = WalkBody { newfid: 4, nwname: 2, wname: [core::ptr::null(); MAXWELEM] };
        w.wname[0] = a.as_ptr() as *const c_char;
        w.wname[1] = b.as_ptr() as *const c_char;
        f.body.walk = w;
        let mut buf = [0u8; 128];
        let n = conv_s2m(&f, &mut buf);
        assert_eq!(n, size_s2m(&f));
        let mut copy = buf[..n].to_vec();
        let mut g = Fcall::new(0);
        assert_eq!(conv_m2s(&mut copy, &mut g), n);
        unsafe {
            assert_eq!(g.body.walk.nwname, 2);
            assert_eq!(cstr(g.body.walk.wname[1]), "glenda");
            assert_eq!(g.body.walk.newfid, 4);
        }
    }

    #[test]
    fn stat_roundtrip() {
        let name = b"hello\0";
        let who = b"none\0";
        let mut d = Dir::new();
        d.qid = Qid { path: 9, vers: 1, typ: QTFILE };
        d.mode = 0o644;
        d.length = 12;
        d.name = name.as_ptr() as *const c_char;
        d.uid = who.as_ptr() as *const c_char;
        d.gid = who.as_ptr() as *const c_char;
        d.muid = who.as_ptr() as *const c_char;
        let mut buf = [0u8; 128];
        let n = conv_d2m(&d, &mut buf);
        assert_eq!(n, size_d2m(&d));
        let mut copy = buf[..n].to_vec();
        assert!(stat_check(&mut copy));
        let mut out = Dir::new();
        let mut strs = [0u8; 64];
        assert_eq!(conv_m2d(&mut copy, &mut out, &mut strs), n);
        assert_eq!(out.length, 12);
        assert_eq!(unsafe { cstr(out.name) }, "hello");
    }
}
