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
use alloc::sync::Arc;
use alloc::vec;

use p9fcall::{MAFTER, MBEFORE, MMASK, MORDER, ORDWR};

use crate::io::{self, Chan};
use crate::mnt;
use crate::mntfs::{self, MntFile};
use crate::sched;
use crate::sync::SpinLock;

const USER: &str = "none";
const MAX_VERSION: usize = 128;

fn fail_s(m: &str) -> i64 {
    io::fail(m)
}

fn spec_arg(ptr: u64) -> Option<String> {
    if ptr == 0 { Some(String::new()) } else { io::user_name(ptr) }
}

fn bindmount(ismount: bool, fd: i64, afd: i64, arg0: u64, arg1: u64, flag: u32, specp: u64) -> i64 {
    if flag & !MMASK != 0 || flag & MORDER == (MBEFORE | MAFTER) {
        return fail_s(mnt::EBADARG);
    }
    let Some(ns) = sched::current_ns() else { return -1 };
    let Some(oldraw) = io::user_name(arg1) else { return fail_s("bad address in system call") };
    let old_visible = io::resolve(&oldraw);
    let old = io::ns_walk(&old_visible, false).path;
    let Some(old_is_dir) = io::path_is_dir(&old) else { return fail_s("file does not exist") };

    let new;
    let spec;
    let src;
    let root;
    let new_is_dir;
    if ismount {
        let Some(sp) = spec_arg(specp) else { return fail_s("bad address in system call") };
        let Some(bc) = io::chan_of(fd as u64) else { return fail_s("fd out of range or not open") };
        let ac = if afd >= 0 {
            match io::chan_of(afd as u64) {
                Some(Chan::Mnt(f)) => Some(f.chan.clone()),
                Some(_) => return fail_s(mnt::EBADUSEFD),
                None => return fail_s("fd out of range or not open"),
            }
        } else {
            None
        };
        let c0 = match mnt::mntattach(&bc, ac.as_ref(), &sp, USER) {
            Ok(c) => c,
            Err(e) => return fail_s(&e),
        };
        new_is_dir = c0.is_dir();
        new = crate::ns::Walked { path: mntfs::register_root(&c0), union: None };
        spec = sp;
        src = String::new();
        root = Some(c0);
    } else {
        let Some(rawnew) = io::user_name(arg0) else { return fail_s("bad address in system call") };
        let visible = io::resolve(&rawnew);
        let w = io::ns_walk(&visible, true);
        let Some(d) = io::path_is_dir(&w.path) else { return fail_s("file does not exist") };
        new_is_dir = d;
        new = w;
        spec = String::new();
        src = visible;
        root = None;
    }

    match ns.with(|n| n.cmount(&new, &old, flag, &spec, &src, root, new_is_dir, old_is_dir)) {
        Ok(()) => 0,
        Err(m) => fail_s(m),
    }
}

pub fn sys_bind(name: u64, old: u64, flag: u64) -> i64 {
    bindmount(false, -1, -1, name, old, flag as u32, 0)
}

pub fn sys_mount(fd: u64, afd: u64, old: u64, flag: u64, spec: u64) -> i64 {
    bindmount(true, fd as i32 as i64, afd as i32 as i64, 0, old, flag as u32, spec)
}

pub fn sys_old_mount(fd: u64, old: u64, flag: u64, spec: u64) -> i64 {
    bindmount(true, fd as i32 as i64, -1, 0, old, flag as u32, spec)
}

pub fn sys_unmount(name: u64, old: u64) -> i64 {
    let Some(ns) = sched::current_ns() else { return -1 };
    let Some(oldraw) = io::user_name(old) else { return fail_s("bad address in system call") };
    let oldc = io::ns_walk(&io::resolve(&oldraw), false).path;
    let r = if name == 0 {
        ns.with(|n| n.cunmount(&oldc, None))
    } else {
        let Some(nameraw) = io::user_name(name) else { return fail_s("bad address in system call") };
        let visible = io::resolve(&nameraw);
        let concrete = io::ns_walk(&visible, true).path;
        ns.with(|n| n.cunmount(&oldc, Some((&concrete, &visible))))
    };
    match r {
        Ok(()) => 0,
        Err(m) => fail_s(m),
    }
}

pub fn sys_fauth(fd: u64, aname: u64) -> i64 {
    let Some(spec) = spec_arg(aname) else { return fail_s("bad address in system call") };
    let Some(c) = io::chan_of(fd) else { return fail_s("fd out of range or not open") };
    let ac = match mnt::mntauth(&c, &spec, USER) {
        Ok(a) => a,
        Err(e) => return fail_s(&e),
    };
    io::install(Chan::Mnt(Arc::new(MntFile {
        chan: ac,
        path: String::from("#M/auth"),
        pos: SpinLock::new(0),
        access: ORDWR,
    })))
}

pub fn sys_fversion(fd: u64, msize: u64, vers: u64, arglen: u64) -> i64 {
    let n = arglen as i32;
    if n <= 0 || n as usize > MAX_VERSION {
        return fail_s(mnt::EBADARG);
    }
    let mut buf = vec![0u8; n as usize];
    if !sched::copy_from_user(vers, &mut buf) {
        return fail_s("bad address in system call");
    }
    let Some(end) = buf.iter().position(|&b| b == 0) else { return fail_s(mnt::EBADARG) };
    let Ok(v) = core::str::from_utf8(&buf[..end]) else { return fail_s(mnt::EBADARG) };
    let Some(c) = io::chan_of(fd) else { return fail_s("fd out of range or not open") };
    match mnt::mntversion(&c, Some(v), msize as i32, n as usize) {
        Ok((k, ver)) => {
            let mut out = ver.into_bytes();
            out.push(0);
            if out.len() <= n as usize && !sched::copy_to_user(vers, &out) {
                return fail_s("bad address in system call");
            }
            k as i64
        }
        Err(e) => fail_s(&e),
    }
}
