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

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec;
use alloc::vec::Vec;

use p9fcall::{Dir, MAXWELEM, OREAD, QTDIR};

use crate::io::Chan;
use crate::mnt::{self, MntChan};
use crate::p9::Stat;
use crate::sched;
use crate::sync::SpinLock;

pub struct MntFile {
    pub chan: Arc<MntChan>,
    pub path: String,
    pub pos: SpinLock<u64>,
    pub access: u8,
}

static ROOTS: SpinLock<BTreeMap<u32, Weak<MntChan>>> = SpinLock::new(BTreeMap::new());
static ERRS: SpinLock<Vec<&'static str>> = SpinLock::new(Vec::new());

const MAX_ERRS: usize = 256;
const DIR_CHUNK: usize = 8192;

pub fn intern(s: &str) -> &'static str {
    ERRS.with(|e| {
        if let Some(k) = e.iter().find(|k| **k == s) {
            return *k;
        }
        if e.len() >= MAX_ERRS {
            return "remote file system error";
        }
        let leaked: &'static str = alloc::boxed::Box::leak(String::from(s).into_boxed_str());
        e.push(leaked);
        leaked
    })
}

fn ie(s: String) -> &'static str {
    intern(&s)
}

pub fn register_root(c: &Arc<MntChan>) -> String {
    ROOTS.with(|r| {
        r.retain(|_, w| w.strong_count() > 0);
        r.insert(c.dev, Arc::downgrade(c));
    });
    alloc::format!("#M{}", c.dev)
}

pub fn is_remote(path: &str) -> bool {
    path.starts_with("#M")
}

pub fn parse(path: &str) -> Option<(u32, Vec<String>)> {
    let rest = path.strip_prefix("#M")?;
    let end = rest.find('/').unwrap_or(rest.len());
    let id: u32 = rest[..end].parse().ok()?;
    let comps = rest[end..].split('/').filter(|c| !c.is_empty()).map(String::from).collect();
    Some((id, comps))
}

fn root_chan(id: u32) -> Result<Arc<MntChan>, &'static str> {
    ROOTS.with(|r| r.get(&id).and_then(|w| w.upgrade())).ok_or("file does not exist")
}

pub fn walk_to(path: &str) -> Result<Arc<MntChan>, &'static str> {
    let (id, comps) = parse(path).ok_or("file does not exist")?;
    let root = root_chan(id)?;
    let mut cur = root;
    if comps.is_empty() {
        return match mnt::mntwalk(&cur, &[]).map_err(ie)? {
            Some(wq) => wq.clone.ok_or("file does not exist"),
            None => Err("file does not exist"),
        };
    }
    for chunk in comps.chunks(MAXWELEM) {
        match mnt::mntwalk(&cur, chunk).map_err(ie)? {
            Some(wq) => match wq.clone {
                Some(c) => cur = c,
                None => return Err("file does not exist"),
            },
            None => return Err("file does not exist"),
        }
    }
    Ok(cur)
}

fn dir_to_stat(d: &Dir) -> Stat {
    Stat {
        name: String::from(unsafe { p9fcall::cstr(d.name) }),
        is_dir: d.qid.typ & QTDIR != 0,
        perm: d.mode & 0o777,
        length: d.length as u64,
        mtime: d.mtime,
        qid_path: d.qid.path,
    }
}

fn decode_stat(buf: &[u8]) -> Result<Stat, &'static str> {
    let mut b = buf.to_vec();
    let mut strs = vec![0u8; b.len() + 8];
    let mut d = Dir::new();
    if p9fcall::conv_m2d(&mut b, &mut d, &mut strs) == 0 {
        return Err("invalid directory entry received from server");
    }
    Ok(dir_to_stat(&d))
}

pub fn stat(path: &str) -> Result<Stat, &'static str> {
    let c = walk_to(path)?;
    let raw = mnt::mntstat(&c).map_err(ie)?;
    decode_stat(&raw)
}

pub fn is_dir(path: &str) -> Option<bool> {
    stat(path).ok().map(|s| s.is_dir)
}

pub fn exists(path: &str) -> bool {
    walk_to(path).is_ok()
}

pub fn open(path: &str, mode: u64) -> Result<Chan, &'static str> {
    let c = walk_to(path)?;
    let omode = (mode & 0x53) as u8;
    mnt::mntopen(&c, omode).map_err(ie)?;
    Ok(Chan::Mnt(Arc::new(MntFile { chan: c, path: String::from(path), pos: SpinLock::new(0), access: omode & 3 })))
}

pub fn create(parent: &str, name: &str, mode: u64, perm: u32) -> Result<Chan, &'static str> {
    let c = walk_to(parent)?;
    let omode = (mode & 0x53) as u8;
    mnt::mntcreate(&c, name, omode, perm).map_err(ie)?;
    let path = crate::ns::join(parent, name);
    Ok(Chan::Mnt(Arc::new(MntFile { chan: c, path, pos: SpinLock::new(0), access: omode & 3 })))
}

pub fn remove(path: &str) -> Result<(), &'static str> {
    let c = walk_to(path)?;
    mnt::mntremove(&c).map_err(ie)
}

pub fn wstat_raw(path: &str, raw: &[u8]) -> Result<(), &'static str> {
    let c = walk_to(path)?;
    mnt::mntwstat(&c, raw).map(|_| ()).map_err(ie)
}

pub fn wstat_fields(path: &str, mode: u32, mtime: u32, length: u64, name: &str) -> Result<(), &'static str> {
    let mut d = Dir::new();
    d.typ = u16::MAX;
    d.dev = u32::MAX;
    d.qid = p9fcall::Qid { path: u64::MAX, vers: u32::MAX, typ: u8::MAX };
    d.mode = mode;
    d.atime = u32::MAX;
    d.mtime = mtime;
    d.length = length as i64;
    let mut nm = String::from(name).into_bytes();
    nm.push(0);
    let empty = [0u8; 1];
    d.name = nm.as_ptr() as *const core::ffi::c_char;
    d.uid = empty.as_ptr() as *const core::ffi::c_char;
    d.gid = empty.as_ptr() as *const core::ffi::c_char;
    d.muid = empty.as_ptr() as *const core::ffi::c_char;
    let n = p9fcall::size_d2m(&d);
    let mut buf = vec![0u8; n];
    if p9fcall::conv_d2m(&d, &mut buf) == 0 {
        return Err("bad stat buffer");
    }
    wstat_raw(path, &buf)
}

pub fn read_dir_all(path: &str) -> Result<Vec<Stat>, &'static str> {
    let c = walk_to(path)?;
    mnt::mntopen(&c, OREAD).map_err(ie)?;
    let mut out = Vec::new();
    let mut off = 0i64;
    loop {
        let buf = mnt::mntread(&c, DIR_CHUNK, off).map_err(ie)?;
        if buf.is_empty() {
            break;
        }
        off += buf.len() as i64;
        let mut p = 0usize;
        while p + 2 <= buf.len() {
            let l = 2 + p9fcall::gbit16(&buf[p..]) as usize;
            if p + l > buf.len() {
                break;
            }
            out.push(decode_stat(&buf[p..p + l])?);
            p += l;
        }
    }
    Ok(out)
}

pub fn read_image(path: &str) -> Result<Vec<u8>, &'static str> {
    let c = walk_to(path)?;
    mnt::mntopen(&c, OREAD).map_err(ie)?;
    let mut out = Vec::new();
    loop {
        let buf = mnt::mntread(&c, DIR_CHUNK, out.len() as i64).map_err(ie)?;
        if buf.is_empty() {
            break;
        }
        out.extend_from_slice(&buf);
    }
    Ok(out)
}

fn fail(m: &str) -> i64 {
    sched::set_errstr_current(m);
    -1
}

pub fn file_read(f: &MntFile, buf: u64, n: u64, off: u64) -> i64 {
    if f.access == 1 {
        return fail("file not open for reading");
    }
    let explicit = off != crate::io::CURRENT_OFFSET;
    let at = if explicit { off } else { f.pos.with(|p| *p) };
    let n = n.min(1 << 20) as usize;
    match mnt::mntread(&f.chan, n, at as i64) {
        Ok(d) => {
            if !d.is_empty() && !sched::copy_to_user(buf, &d) {
                return fail("bad address in system call");
            }
            if !explicit {
                f.pos.with(|p| *p += d.len() as u64);
            }
            d.len() as i64
        }
        Err(m) => fail(&m),
    }
}

pub fn file_write(f: &MntFile, buf: u64, n: u64, off: u64) -> i64 {
    if f.access == OREAD {
        return fail("file not open for writing");
    }
    let n = n.min(1 << 20) as usize;
    let mut data = vec![0u8; n];
    if !sched::copy_from_user(buf, &mut data) {
        return fail("bad address in system call");
    }
    let explicit = off != crate::io::CURRENT_OFFSET;
    let at = if explicit { off } else { f.pos.with(|p| *p) };
    match mnt::mntwrite(&f.chan, &data, at as i64) {
        Ok(k) => {
            if !explicit {
                f.pos.with(|p| *p += k as u64);
            }
            k as i64
        }
        Err(m) => fail(&m),
    }
}

pub fn file_stat(f: &MntFile) -> Result<Stat, &'static str> {
    let raw = mnt::mntstat(&f.chan).map_err(ie)?;
    decode_stat(&raw)
}

pub fn file_size(f: &MntFile) -> Option<u64> {
    file_stat(f).ok().map(|s| s.length)
}
