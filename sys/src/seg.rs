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
use alloc::vec::Vec;

use crate::mm::aspace::{self, AsError, Kind};
use crate::mm::{PAGE_SIZE, PageFlags, PhysAddr};
use crate::sched;
use crate::sync::SpinLock;

const SG_TYPE: u64 = 0x07;
const SG_RONLY: u64 = 0x20;
const MAX_SEG_BYTES: u64 = 256 << 20;
const MAX_NAMED: usize = 64;
const MAX_CLASS: usize = 64;

struct Named {
    name: String,
    base: PhysAddr,
    pages: u64,
}

static NAMED: SpinLock<Vec<Named>> = SpinLock::new(Vec::new());

fn fail(msg: &str) -> i64 {
    sched::set_errstr_current(msg);
    -1
}

fn as_msg(e: AsError) -> &'static str {
    match e {
        AsError::Overlap => "segments overlap",
        AsError::NoMemory => "out of memory",
        AsError::Misaligned | AsError::OutOfRange => "bad arg in system call",
        AsError::NotMapped => "bad address in system call",
        AsError::Denied => "permission denied",
        _ => "segment operation failed",
    }
}

fn round_up(len: u64) -> Option<u64> {
    len.checked_add(PAGE_SIZE - 1).map(|v| v & !(PAGE_SIZE - 1))
}

fn named_segment(name: &str, pages: u64) -> Result<PhysAddr, &'static str> {
    NAMED.with(|t| {
        if let Some(n) = t.iter().find(|n| n.name == name) {
            return if pages <= n.pages { Ok(n.base) } else { Err("bad arg in system call") };
        }
        if t.len() >= MAX_NAMED {
            return Err("too many segments");
        }
        let base = aspace::shared_new(pages).ok_or("out of memory")?;
        aspace::shared_ref(base);
        t.push(Named { name: String::from(name), base, pages });
        Ok(base)
    })
}

pub fn sys_segattach(attr: u64, class_ptr: u64, va: u64, len: u64) -> i64 {
    let Some(me) = sched::current() else { return -1 };
    let Some(class) = sched::cstr_from_user(class_ptr, MAX_CLASS) else { return fail("bad address in system call") };
    if class.is_empty() || len == 0 || va % PAGE_SIZE != 0 {
        return fail("bad arg in system call");
    }
    let Some(bytes) = round_up(len).filter(|b| *b <= MAX_SEG_BYTES) else { return fail("bad arg in system call") };
    let pages = bytes / PAGE_SIZE;
    let flags = if attr & SG_RONLY != 0 { PageFlags::USER_RO } else { PageFlags::USER_RW };
    let _ = attr & SG_TYPE;
    match class.as_str() {
        "memory" => {
            let r = sched::with_aspace(me, |a| {
                if va == 0 {
                    a.map_anywhere(bytes, flags, Kind::Anon)
                } else {
                    a.map_region(va, bytes, flags, Kind::Anon).map(|_| va)
                }
            });
            match r {
                Some(Ok(addr)) => addr as i64,
                Some(Err(e)) => fail(as_msg(e)),
                None => fail("no address space"),
            }
        }
        "text" | "data" | "bss" | "stack" => fail("bad arg in system call"),
        c if c == "physical" || c.starts_with("physical") => fail("permission denied"),
        c => {
            let (base, anonymous) = if c == "shared" {
                match aspace::shared_new(pages) {
                    Some(b) => (b, true),
                    None => return fail("out of memory"),
                }
            } else {
                match named_segment(c, pages) {
                    Ok(b) => (b, false),
                    Err(m) => return fail(m),
                }
            };
            let r = sched::with_aspace(me, |a| {
                if va == 0 {
                    a.map_shared_anywhere(base, bytes, flags)
                } else {
                    a.map_shared(va, base, bytes, flags).map(|_| va)
                }
            });
            match r {
                Some(Ok(addr)) => addr as i64,
                Some(Err(e)) => {
                    if anonymous {
                        aspace::shared_unref(base);
                    }
                    fail(as_msg(e))
                }
                None => {
                    if anonymous {
                        aspace::shared_unref(base);
                    }
                    fail("no address space")
                }
            }
        }
    }
}

pub fn sys_segdetach(va: u64) -> i64 {
    let Some(me) = sched::current() else { return -1 };
    let r = sched::with_aspace(me, |a| {
        if let Some((base, _, _)) = a.attachment_at(va) {
            return a.detach_shared(base);
        }
        match a.region_at(va) {
            Some((start, len, Kind::Anon)) => a.unmap(start, len),
            Some(_) => Err(AsError::Denied),
            None => Err(AsError::NotMapped),
        }
    });
    match r {
        Some(Ok(())) => 0,
        Some(Err(AsError::Denied)) => fail("cannot detach this segment"),
        Some(Err(e)) => fail(as_msg(e)),
        None => fail("no address space"),
    }
}

pub fn sys_segfree(va: u64, len: u64) -> i64 {
    let Some(me) = sched::current() else { return -1 };
    let Some(end) = va.checked_add(len) else { return fail("bad arg in system call") };
    let Some(from) = round_up(va) else { return fail("bad arg in system call") };
    let to = end & !(PAGE_SIZE - 1);
    if to < from {
        return fail("bad arg in system call");
    }
    if to == from {
        return 0;
    }
    let r = sched::with_aspace(me, |a| match a.region_at(from) {
        Some((start, rlen, Kind::Anon)) if start.checked_add(rlen).is_some_and(|e| e >= to) => a.unmap(from, to - from),
        Some(_) => Err(AsError::Denied),
        None => Err(AsError::NotMapped),
    });
    match r {
        Some(Ok(())) => 0,
        Some(Err(AsError::Denied)) => fail("cannot free part of this segment"),
        Some(Err(e)) => fail(as_msg(e)),
        None => fail("no address space"),
    }
}

pub fn sys_segflush(va: u64, len: u64) -> i64 {
    let Some(me) = sched::current() else { return -1 };
    let Some(end) = va.checked_add(len) else { return fail("bad arg in system call") };
    if len == 0 {
        return 0;
    }
    let covered = sched::with_aspace(me, |a| {
        let mut cur = va & !(PAGE_SIZE - 1);
        while cur < end {
            match a.region_at(cur) {
                Some((s, l, _)) => match s.checked_add(l) {
                    Some(e) if e > cur => cur = e,
                    _ => return false,
                },
                None => return false,
            }
        }
        true
    });
    match covered {
        Some(true) => {
            sync_icache();
            0
        }
        Some(false) => fail("bad address in system call"),
        None => fail("no address space"),
    }
}

#[cfg(target_arch = "aarch64")]
fn sync_icache() {
    // SAFETY: cache/barrier maintenance instructions only; they touch no Rust-visible memory
    // and are valid at EL1.
    unsafe { core::arch::asm!("dsb ish", "ic ialluis", "dsb ish", "isb") };
}

#[cfg(target_arch = "riscv64")]
fn sync_icache() {
    // SAFETY: `fence.i` only synchronizes the instruction stream with prior stores on this hart.
    unsafe { core::arch::asm!("fence.i") };
}

#[cfg(target_arch = "x86_64")]
fn sync_icache() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
