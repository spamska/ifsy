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
use core::sync::atomic::Ordering;

use crate::console::color::{failed, pass, warn};
use crate::elf::{self, ElfError};
use crate::mm::{self, PageFlags};
use crate::mm::aspace::Kind;
use crate::note::{Fault, NCONT, NDFLT, NRSTR, NSAVE, NoteKind};
use crate::sched::{self, RFMEM, RFPROC, Resume, State, Wait};
use crate::syscall::{self, Support, dispatch_args, dispatch_user};
use crate::timer;
use crate::uctx::{self, Ureg};

const ME: sched::Tid = sched::BOOT_TID;

struct Report {
    pass: u32,
    warn: u32,
    fail: u32,
}

impl Report {
    fn ok(&mut self, name: &str) {
        self.pass += 1;
        kprintln!("ifsy: abi: {} {}", name, pass());
    }
    fn gap(&mut self, name: &str, why: &str) {
        self.warn += 1;
        kprintln!("ifsy: abi: {} -- {} {}", name, why, warn());
    }
    fn bad(&mut self, name: &str) {
        self.fail += 1;
        kprintln!("ifsy: abi: {} {}", name, failed());
    }
    fn check(&mut self, name: &str, cond: bool) {
        if cond { self.ok(name) } else { self.bad(name) }
    }
}

fn sys(n: u32, a0: u64, a1: u64) -> i64 {
    dispatch_args(n, &[a0, a1, 0, 0, 0, 0])
}

fn put(tid: sched::Tid, va: u64, bytes: &[u8]) -> bool {
    sched::with_aspace(tid, |a| a.write_bytes(va, bytes).is_ok()).unwrap_or(false)
}

fn get(tid: sched::Tid, va: u64, n: usize) -> Vec<u8> {
    sched::with_aspace(tid, |a| {
        let mut b = alloc::vec![0u8; n];
        a.read_bytes(va, &mut b).ok().map(|_| b)
    })
    .flatten()
    .unwrap_or_default()
}

fn cstr(tid: sched::Tid, va: u64) -> String {
    let mut v = Vec::new();
    while v.len() < 128 {
        match get(tid, va + v.len() as u64, 1).first() {
            Some(&b) if b != 0 => v.push(b),
            _ => break,
        }
    }
    String::from_utf8_lossy(&v).into_owned()
}

fn word(tid: sched::Tid, va: u64) -> Option<u32> {
    let v = get(tid, va, 4);
    (v.len() == 4).then(|| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

fn env() -> Option<u64> {
    sched::init();
    let a = mm::new_address_space().ok()?;
    if !sched::attach_aspace(ME, a) {
        return None;
    }
    sched::with_aspace(ME, |a| {
        let h = a.brk(0).ok()?;
        a.brk(h + 2 * 4096).ok()?;
        Some(h)
    })
    .flatten()
}

pub fn selftest() {
    let mut r = Report { pass: 0, warn: 0, fail: 0 };

    contract_table(&mut r);
    if env().is_some() {
        user_memory(&mut r);
        fork_semantics(&mut r);
        exit_and_wait(&mut r);
        semaphores(&mut r);
        notes(&mut r);
    } else {
        r.gap("user-memory ABI (errstr, brk, rfork copy, exits/await text, semaphores)", "skipped: no address space (paging unavailable)");
    }
    alarm(&mut r);
    elf_loader(&mut r);
    clock(&mut r);
    platform(&mut r);

    sched::init();

    let tag = if r.fail > 0 { failed() } else if r.warn > 0 { warn() } else { pass() };
    kprintln!(
        "ifsy: abi: sched interface: {} pass, {} warn, {} fail {}",
        r.pass, r.warn, r.fail, tag
    );
}

fn contract_table(r: &mut Report) {
    let t = syscall::TABLE;
    let unique = t.iter().enumerate().all(|(i, a)| t[i + 1..].iter().all(|b| a.number != b.number));
    r.check("syscall numbers are unique", unique);

    sched::init();
    syscall::QUIET.store(true, Ordering::Relaxed);
    let unknown_ok = dispatch_args(syscall::UNKNOWN, &[0; 6]) == -1;
    let missing_ok = t.iter().filter(|s| syscall::support(s.number) == Support::Missing).all(|s| sys(s.number, 0, 0) == -1);
    syscall::QUIET.store(false, Ordering::Relaxed);
    sched::init();
    r.check("unknown / missing syscalls fail with -1", unknown_ok && missing_ok);

    let (mut full, mut missing) = (0u32, 0u32);
    for s in t {
        match syscall::support(s.number) {
            Support::Full => full += 1,
            Support::Missing => missing += 1,
            Support::Partial(why) => {
                r.gap(&alloc::format!("{} partial", s.name), why);
            }
        }
    }
    if missing == 0 {
        r.ok("every syscall in the table is implemented");
    } else {
        r.gap(
            "syscall coverage",
            &alloc::format!("{} of {} implemented in full, {} not implemented yet (files, notes, segments, ...)", full, t.len(), missing),
        );
    }
}

fn user_memory(r: &mut Report) {
    let Some(h) = env() else { return };

    put(ME, h, b"first\0");
    let a = sys(syscall::ERRSTR, h, 128);
    let old_empty = get(ME, h, 1) == [0u8];
    put(ME, h, b"second\0");
    let b = sys(syscall::ERRSTR, h, 128);
    r.check(
        "errstr swaps the per-thread error string",
        a == 0 && old_empty && b == 0 && cstr(ME, h) == "first" && sched::errstr_of(ME).as_deref() == Some("second"),
    );
    r.check(
        "bad user pointers are refused, not dereferenced",
        sys(syscall::ERRSTR, 0, 64) == -1 && sys(syscall::ERRSTR, 0x10, 64) == -1 && sys(syscall::ERRSTR, h + 0x10_0000, 64) == -1,
    );

    let ro = sched::with_aspace(ME, |a| {
        let base = a.map_image(4096).ok()?;
        a.write_bytes(base, b"x\0").ok()?;
        a.protect(base, 4096, PageFlags::USER_EXEC).ok()?;
        Some(base)
    })
    .flatten();
    let kern = sched::with_aspace(ME, |a| a.map_anywhere(4096, PageFlags::KERNEL_RW, Kind::Anon).ok()).flatten();
    match (ro, kern) {
        (Some(ro), Some(kern)) => r.check(
            "copy_to_user honours read-only and kernel-only pages",
            sys(syscall::ERRSTR, ro, 64) == -1
                && sys(syscall::ERRSTR, kern, 64) == -1
                && sched::errstr_of(ME).as_deref() == Some("second"),
        ),
        _ => r.bad("could not set up read-only / kernel-only test pages"),
    }

    let end = sched::with_aspace(ME, |a| a.brk_end()).unwrap_or(0);
    let grow = sys(syscall::BRK_, end + 4096, 0) == 0 && sched::with_aspace(ME, |a| a.brk_end()) == Some(end + 4096);
    let refuse = sys(syscall::BRK_, h - 4096, 0) == -1 && sys(syscall::BRK_, 0, 0) == -1;
    let seg = sys(syscall::SEGBRK, 0, end) == end as i64;
    r.check("brk_ / segbrk move the calling thread's heap", grow && refuse && seg);
}

fn fork_semantics(r: &mut Report) {
    sched::init();
    let free0 = mm::frame_stats().1;
    let Some(h) = env() else { return };

    put(ME, h, b"parent\0");
    let c2 = sys(syscall::RFORK, RFPROC, 0);
    let copied = get(2, h, 7) == b"parent\0".as_slice() && !sched::shares_aspace(ME, 2);
    put(ME, h, b"PARENT\0");
    let isolated = get(2, h, 7) == b"parent\0".as_slice();
    r.check("rfork copies the address space", c2 == 2 && copied && isolated);

    let c3 = sys(syscall::RFORK, RFPROC | RFMEM, 0);
    put(ME, h, b"later\0");
    r.check(
        "rfork(RFMEM) shares it",
        c3 == 3 && sched::shares_aspace(ME, 3) && cstr(3, h) == "later",
    );
    r.check("rfork without RFPROC makes no thread", sys(syscall::RFORK, 0, 0) == 0 && sched::thread_count() == 3);

    sched::init();
    r.check("frames are returned when threads and spaces go away", mm::frame_stats().1 == free0);
}

fn exit_and_wait(r: &mut Report) {
    let Some(h) = env() else { return };
    put(ME, h + 16, b"boom\0");
    let child = sys(syscall::RFORK, RFPROC, 0);
    let blocked = sys(syscall::AWAIT, h + 64, 64) == 0 && sched::current() == Some(2);
    sys(syscall::EXITS, h + 16, 0);
    let text = cstr(ME, h + 64);
    r.check(
        "exits(msg) reaches a blocked await(buf) as \"pid utime stime rtime msg\"",
        child == 2
            && blocked
            && sched::current() == Some(ME)
            && text == "2 0 0 0 boom"
            && sched::pending_ret(ME) == Some(text.len() as i64)
            && sched::state_of(2).is_none(),
    );

    let child = sys(syscall::RFORK, RFPROC, 0);
    sys(syscall::SLEEP, 0, 0);
    let running = sched::current() == Some(child as sched::Tid);
    sys(syscall::EXITS, 0, 0);
    let zombie = sched::state_of(3) == Some(State::Zombie) && sched::current() == Some(ME);
    let n = sys(syscall::AWAIT, h + 64, 64);
    let text = cstr(ME, h + 64);
    r.check(
        "await(buf) collects a zombie without blocking",
        child == 3 && running && zombie && n == text.len() as i64 && text.starts_with("3 ") && sched::thread_count() == 1,
    );
    r.check("await with no children fails", sys(syscall::AWAIT, h + 64, 64) == -1);

    let child = sys(syscall::RFORK, RFPROC, 0);
    sys(syscall::SLEEP, 0, 0);
    sys(syscall::EXITS, 0, 0);
    put(ME, h + 64, &[0xAA; 8]);
    let n = sys(syscall::AWAIT, h + 64, 3);
    let got = get(ME, h + 64, 4);
    r.check("await(buf, n) never writes past n", child == 4 && n == 2 && got == [b'4', b' ', 0, 0xAA]);
}

fn semaphores(r: &mut Report) {
    let Some(h) = env() else { return };
    put(ME, h, &0u32.to_le_bytes());
    let child = sys(syscall::RFORK, RFPROC | RFMEM, 0);

    let parked = sys(syscall::SEMACQUIRE, h, 1) == 0
        && sched::current() == Some(2)
        && matches!(sched::state_of(ME), Some(State::Sleeping(Wait::Sem(..))));
    let rel = sys(syscall::SEMRELEASE, h, 1);
    r.check(
        "semrelease hands a unit to a sleeping semacquire",
        child == 2
            && parked
            && rel == 0
            && sched::pending_ret(ME) == Some(1)
            && sched::state_of(ME) == Some(State::Ready)
            && word(2, h) == Some(0),
    );

    sys(syscall::SEMRELEASE, h, 2);
    let two = word(2, h) == Some(2);
    let a = sys(syscall::SEMACQUIRE, h, 0);
    let b = sys(syscall::SEMACQUIRE, h, 1);
    let c = sys(syscall::SEMACQUIRE, h, 0);
    r.check(
        "semaphore counts: release adds, acquire takes, empty try returns 0",
        two && a == 1 && b == 1 && c == 0 && word(2, h) == Some(0),
    );
    r.check(
        "bad or misaligned semaphore addresses fail with -1",
        sys(syscall::SEMACQUIRE, 0x10, 1) == -1 && sys(syscall::SEMACQUIRE, h + 1, 1) == -1 && sys(syscall::SEMRELEASE, h + 2, 1) == -1,
    );

    let parked = sys(syscall::TSEMACQUIRE, h, 20) == 0 && sched::current() == Some(ME);
    let early = sched::tick();
    let still = matches!(sched::state_of(2), Some(State::Sleeping(Wait::Sem(..))));
    sched::tick();
    r.check(
        "tsemacquire times out after its deadline with 0",
        parked && !early && still && sched::state_of(2) == Some(State::Ready) && sched::pending_ret(2) == Some(0),
    );
}

fn alarm(r: &mut Report) {
    sched::init();
    let first = sys(syscall::ALARM, 50, 0);
    let again = sys(syscall::ALARM, 30, 0);
    let (t0, t1, t2) = (sched::tick(), sched::tick(), sched::tick());
    let note = sched::take_note(ME).map(|n| n.msg);
    let none_after = sched::take_note(ME).is_none();
    let cancelled = {
        sys(syscall::ALARM, 20, 0);
        let left = sys(syscall::ALARM, 0, 0);
        sched::tick();
        sched::tick();
        left == 20 && sched::take_note(ME).is_none()
    };
    let _ = (t0, t1, t2);
    r.check(
        "alarm returns the time left and queues a note when due",
        first == 0 && again == 50 && note.as_deref() == Some("alarm") && none_after && cancelled,
    );
}

fn clock(r: &mut Report) {
    sched::init();
    let parked = sys(syscall::SLEEP, 30, 0) == 0 && sched::current().is_none();
    let (a, b, c) = (sched::tick(), sched::tick(), sched::tick());
    r.check(
        "sleep(30 ms) parks the thread and wakes it on the third tick",
        parked && !a && !b && c && sched::pending_ret(ME) == Some(0) && sched::schedule() == Some(ME),
    );
    if timer::freq() == 0 {
        r.gap("nsec()", "timer not started");
    } else {
        let (x, y) = (sys(syscall::NSEC, 0, 0), sys(syscall::NSEC, 0, 0));
        r.check("nsec() is positive and monotonic", x > 0 && y >= x);
    }
}

fn platform(r: &mut Report) {
    let caps = crate::arch::CAPS;
    if caps.context_switch {
        r.ok("arch context switch");
    } else {
        r.gap("arch context switch", "missing: sched::schedule() only does bookkeeping, the chosen thread does not run");
    }
    if caps.user_mode {
        r.ok("arch user mode + syscall entry");
    } else {
        r.gap("arch user mode + syscall entry", "missing on this architecture: syscalls cannot be made from user code yet (see arch/hal.rs)");
    }
    if caps.timer_irq && timer::irq_active() {
        r.ok("timer interrupt drives sched::tick");
    } else if caps.timer_irq {
        r.gap("timer interrupt", "port has one, but no source was found or no tick arrived: falling back to timer::poll()");
    } else if timer::freq_trusted() {
        r.gap("timer interrupt", "none: the clock is polled via timer::poll(), idle loops must call it");
    } else {
        r.gap("timer interrupt", "none, and the counter frequency is assumed: sleep durations may be off");
    }
}

const PF_R: u32 = 4;
const PF_RW: u32 = 6;
const PF_RX: u32 = 5;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_GNU_RELRO: u32 = 0x6474_e552;

struct Ph {
    kind: u32,
    flags: u32,
    vaddr: u64,
    memsz: u64,
    filesz: Option<u64>,
    data: Vec<u8>,
    within: Option<(usize, u64)>,
}

impl Ph {
    fn load(vaddr: u64, flags: u32, data: Vec<u8>, memsz: u64) -> Ph {
        Ph { kind: PT_LOAD, flags, vaddr, memsz, filesz: None, data, within: None }
    }
    fn bare(kind: u32, flags: u32, vaddr: u64, memsz: u64) -> Ph {
        Ph { kind, flags, vaddr, memsz, filesz: Some(0), data: Vec::new(), within: None }
    }
}

fn build_elf(etype: u16, machine: u16, entry: u64, phs: &[Ph]) -> Vec<u8> {
    let mut out = alloc::vec![0u8; 64 + 56 * phs.len()];
    out[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    out[4] = 2;
    out[5] = 1;
    out[6] = 1;
    out[16..18].copy_from_slice(&etype.to_le_bytes());
    out[18..20].copy_from_slice(&machine.to_le_bytes());
    out[20..24].copy_from_slice(&1u32.to_le_bytes());
    out[24..32].copy_from_slice(&entry.to_le_bytes());
    out[32..40].copy_from_slice(&64u64.to_le_bytes());
    out[52..54].copy_from_slice(&64u16.to_le_bytes());
    out[54..56].copy_from_slice(&56u16.to_le_bytes());
    out[56..58].copy_from_slice(&(phs.len() as u16).to_le_bytes());
    let mut offs = Vec::new();
    for ph in phs {
        if ph.data.is_empty() {
            offs.push(0u64);
            continue;
        }
        while out.len() % 16 != 0 {
            out.push(0);
        }
        offs.push(out.len() as u64);
        out.extend_from_slice(&ph.data);
    }
    for (i, ph) in phs.iter().enumerate() {
        let off = match ph.within {
            Some((j, d)) => offs[j] + d,
            None => offs[i],
        };
        let filesz = ph.filesz.unwrap_or(ph.data.len() as u64);
        let o = 64 + 56 * i;
        out[o..o + 4].copy_from_slice(&ph.kind.to_le_bytes());
        out[o + 4..o + 8].copy_from_slice(&ph.flags.to_le_bytes());
        out[o + 8..o + 16].copy_from_slice(&off.to_le_bytes());
        out[o + 16..o + 24].copy_from_slice(&ph.vaddr.to_le_bytes());
        out[o + 24..o + 32].copy_from_slice(&ph.vaddr.to_le_bytes());
        out[o + 32..o + 40].copy_from_slice(&filesz.to_le_bytes());
        out[o + 40..o + 48].copy_from_slice(&ph.memsz.to_le_bytes());
        out[o + 48..o + 56].copy_from_slice(&4096u64.to_le_bytes());
    }
    out
}

const TEXT_LEN: usize = 0x40;

fn text_bytes() -> Vec<u8> {
    (0..TEXT_LEN).map(|i| (i as u8) ^ 0x5A).collect()
}

fn pie_headers(reloc_type: u32, needed: bool) -> Vec<Ph> {
    let mut data = alloc::vec![0u8; 0x200];
    data[..8].copy_from_slice(b"PIEDATA\0");
    let mut dynamic: Vec<(u64, u64)> = Vec::new();
    if needed {
        dynamic.push((1, 1));
    }
    dynamic.extend_from_slice(&[(7, 0x2080), (8, 48), (9, 24), (0, 0)]);
    for (i, (tag, val)) in dynamic.iter().enumerate() {
        data[0x10 + 16 * i..0x18 + 16 * i].copy_from_slice(&tag.to_le_bytes());
        data[0x18 + 16 * i..0x20 + 16 * i].copy_from_slice(&val.to_le_bytes());
    }
    let dyn_len = 16 * dynamic.len() as u64;
    for (i, (off, addend)) in [(0x2100u64, 0x2040i64), (0x2108, 0x10)].into_iter().enumerate() {
        let o = 0x80 + 24 * i;
        data[o..o + 8].copy_from_slice(&off.to_le_bytes());
        data[o + 8..o + 16].copy_from_slice(&u64::from(reloc_type).to_le_bytes());
        data[o + 16..o + 24].copy_from_slice(&addend.to_le_bytes());
    }
    alloc::vec![
        Ph::load(0, PF_RX, text_bytes(), 0x1000),
        Ph::load(0x2000, PF_RW, data, 0x2000),
        Ph { kind: PT_DYNAMIC, flags: PF_RW, vaddr: 0x2010, memsz: dyn_len, filesz: Some(dyn_len), data: Vec::new(), within: Some((1, 0x10)) },
        Ph::bare(PT_GNU_RELRO, PF_R, 0x2000, 0x1000),
    ]
}

fn pie_image() -> Vec<u8> {
    build_elf(3, elf::MACHINE, 0x10, &pie_headers(elf::R_RELATIVE, false))
}

fn try_load(image: &[u8]) -> Option<Result<elf::Loaded, ElfError>> {
    let mut a = mm::new_address_space().ok()?;
    Some(elf::load(&mut a, image))
}

fn elf_loader(r: &mut Report) {
    let free0 = mm::frame_stats().1;
    let Ok(mut a) = mm::new_address_space() else {
        r.gap("ELF loader / exec", "skipped: no address space (paging unavailable)");
        return;
    };
    let image = pie_image();
    let loaded = elf::load(&mut a, &image);
    let base = a.layout().image_base;
    match loaded {
        Err(_) => r.bad("ELF static-PIE loads"),
        Ok(l) => {
            r.check(
                "ELF static-PIE loads at the randomised image base, entry biased",
                l.pie && l.bias == base && l.entry == base + 0x10 && l.low == base && l.high == base + 0x4000,
            );
            let text = get_aspace(&a, base, TEXT_LEN);
            let (tp, dp, bss) = (a.translate(base), a.translate(base + 0x2000), a.translate(base + 0x3000));
            r.check(
                "segments get their own permissions (text RX, RELRO page RO, bss RW), nothing W+X",
                text == text_bytes()
                    && tp.is_some_and(|(_, f)| f.executable() && !f.writable() && f.user_accessible())
                    && dp.is_some_and(|(_, f)| !f.writable() && !f.executable())
                    && bss.is_some_and(|(_, f)| f.writable() && !f.executable())
                    && a.translate(base + 0x1000).is_none(),
            );
            let word = |va: u64| {
                let b = get_aspace(&a, va, 8);
                (b.len() == 8).then(|| u64::from_le_bytes(b[..8].try_into().unwrap()))
            };
            r.check(
                "R_*_RELATIVE relocations are applied by the kernel",
                word(base + 0x2100) == Some(base + 0x2040)
                    && word(base + 0x2108) == Some(base + 0x10)
                    && get_aspace(&a, base + 0x2000, 8) == b"PIEDATA\0"
                    && word(base + 0x3000) == Some(0),
            );
        }
    }
    drop(a);

    let exec = build_elf(
        2,
        elf::MACHINE,
        0x400000,
        &[
            Ph::load(0x400000, PF_RX, text_bytes(), 0x1000),
            Ph::load(0x401000, PF_RW, alloc::vec![0xAB; 0x10], 0x800),
        ],
    );
    if let Ok(mut a) = mm::new_address_space() {
        let ok = elf::load(&mut a, &exec);
        let tail = get_aspace(&a, 0x401000, 0x20);
        r.check(
            "ET_EXEC lands at its own addresses; memsz beyond filesz is zero",
            ok.is_ok_and(|l| l.entry == 0x400000 && l.bias == 0 && !l.pie)
                && tail[..0x10] == [0xAB; 0x10]
                && tail[0x10..] == [0u8; 0x10]
                && a.translate(0x401000).is_some_and(|(_, f)| f.writable() && !f.executable()),
        );
    }
    let shared = build_elf(
        2,
        elf::MACHINE,
        0x400000,
        &[Ph::load(0x400000, PF_RX, text_bytes(), 0x800), Ph::load(0x400800, PF_RW, alloc::vec![1; 8], 0x100)],
    );
    let ro_rw = build_elf(
        2,
        elf::MACHINE,
        0x400000,
        &[Ph::load(0x400000, PF_RX, text_bytes(), 0x1000), Ph::load(0x401000, PF_R, alloc::vec![1; 8], 0x400), Ph::load(0x401400, PF_RW, alloc::vec![2; 8], 0x100)],
    );
    r.check(
        "a page shared by code and data is refused; read-only + data shares as read-write",
        try_load(&shared) == Some(Err(ElfError::WriteXorExec)) && try_load(&ro_rw).is_some_and(|x| x.is_ok()),
    );

    let good = pie_headers(elf::R_RELATIVE, false);
    let mut bad_magic = image.clone();
    bad_magic[1] = b'X';
    let mut class32 = image.clone();
    class32[4] = 1;
    let mut rel_type = image.clone();
    rel_type[16] = 1;
    let wrong_cpu = build_elf(3, elf::MACHINE ^ 0x100, 0x10, &good);
    let mut interp = pie_headers(elf::R_RELATIVE, false);
    interp.push(Ph::bare(PT_INTERP, PF_R, 0, 0));
    let mut wx = pie_headers(elf::R_RELATIVE, false);
    wx[0].flags = 7;
    let mut big = pie_headers(elf::R_RELATIVE, false);
    big[0].filesz = Some(0x2000);
    let mut past_eof = pie_headers(elf::R_RELATIVE, false);
    past_eof[0].filesz = Some(0x10_0000);
    past_eof[0].memsz = 0x10_0000;
    let mut overlap = pie_headers(elf::R_RELATIVE, false);
    overlap.push(Ph::load(0x2800, PF_RW, alloc::vec![1; 8], 0x100));
    let loads = |v: Vec<Ph>| build_elf(3, elf::MACHINE, 0x10, &v);
    let cases: [(&str, Vec<u8>, ElfError); 16] = [
        ("truncated header", image[..40].to_vec(), ElfError::Truncated),
        ("bad magic", bad_magic, ElfError::BadMagic),
        ("32-bit class", class32, ElfError::BadClass),
        ("ET_REL", rel_type, ElfError::BadType),
        ("other CPU", wrong_cpu, ElfError::BadMachine),
        ("PT_INTERP", loads(interp), ElfError::Dynamic),
        ("W+X segment", loads(wx), ElfError::WriteXorExec),
        ("filesz > memsz", loads(big), ElfError::BadSegment),
        ("segment past end of file", loads(past_eof), ElfError::BadSegment),
        ("overlapping segments", loads(overlap), ElfError::BadSegment),
        ("entry in data", build_elf(3, elf::MACHINE, 0x2000, &good), ElfError::BadEntry),
        ("entry outside the image", build_elf(3, elf::MACHINE, 0x9000, &good), ElfError::BadEntry),
        ("no PT_LOAD", build_elf(3, elf::MACHINE, 0, &[Ph::bare(PT_DYNAMIC, PF_R, 0, 0)]), ElfError::NoLoad),
        (
            "ET_EXEC below the null guard",
            build_elf(2, elf::MACHINE, 0x1000, &[Ph::load(0x1000, PF_RX, text_bytes(), 0x1000)]),
            ElfError::BadAddress,
        ),
        (
            "ET_EXEC where the heap lives",
            build_elf(2, elf::MACHINE, 0x7000_0000, &[Ph::load(0x7000_0000, PF_RX, text_bytes(), 0x1000)]),
            ElfError::BadAddress,
        ),
        (
            "unsupported relocation",
            build_elf(3, elf::MACHINE, 0x10, &pie_headers(0x7fff, false)),
            ElfError::BadReloc,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, img, want) in &cases {
        if try_load(img) != Some(Err(*want)) {
            wrong.push(*name);
        }
    }
    let needed = build_elf(3, elf::MACHINE, 0x10, &pie_headers(elf::R_RELATIVE, true));
    if try_load(&needed) != Some(Err(ElfError::Dynamic)) {
        wrong.push("DT_NEEDED");
    }
    if wrong.is_empty() {
        r.ok("hostile ELF files are refused with the right error (17 cases)");
    } else {
        for w in &wrong {
            kprintln!("ifsy: abi: ELF case not refused as expected: {}", w);
        }
        r.bad("hostile ELF files are refused with the right error");
    }
    r.check("a failed load gives every frame back", mm::frame_stats().1 == free0);

    if let Ok(mut a) = mm::new_address_space() {
        let top = a.map_stack(mm::aspace::STACK_DEFAULT).unwrap_or(0);
        let args = alloc::vec![b"/bin/ls".to_vec(), b"-l".to_vec(), b"x y".to_vec()];
        let built = elf::build_stack(&a, top, &args);
        let ok = built.is_ok_and(|(sp, argv)| {
            let word = |va: u64| u64::from_le_bytes(get_aspace(&a, va, 8)[..8].try_into().unwrap_or([0; 8]));
            let s = |va: u64| {
                let mut v = Vec::new();
                while v.len() < 32 {
                    match get_aspace(&a, va + v.len() as u64, 1).first() {
                        Some(&b) if b != 0 => v.push(b),
                        _ => break,
                    }
                }
                v
            };
            sp % 16 == 0
                && argv == sp + 8
                && word(sp) == 3
                && s(word(argv)) == b"/bin/ls"
                && s(word(argv + 8)) == b"-l"
                && s(word(argv + 16)) == b"x y"
                && word(argv + 24) == 0
                && word(argv) > argv
                && word(argv + 16) < top
        });
        r.check("initial stack: argc, argv[], NULL, then the strings; 16-byte aligned", ok);
    }
}

fn get_aspace(a: &mm::aspace::AddressSpace, va: u64, n: usize) -> Vec<u8> {
    let mut b = alloc::vec![0u8; n];
    if a.read_bytes(va, &mut b).is_ok() { b } else { Vec::new() }
}

struct U {
    h: u64,
    code: u64,
    top: u64,
}

fn user_env() -> Option<U> {
    let h = env()?;
    sched::with_aspace(ME, |a| {
        let top = a.map_stack(64 * 1024).ok()?;
        let code = a.map_anywhere(4096, PageFlags::USER_RW, Kind::Anon).ok()?;
        a.write_bytes(code, &[0x90; 64]).ok()?;
        a.protect(code, 4096, PageFlags::USER_EXEC).ok()?;
        Some(U { h, code, top })
    })
    .flatten()
}

fn get_ureg(tid: sched::Tid, va: u64) -> Option<Ureg> {
    Ureg::from_bytes(&get(tid, va, uctx::UREG_SIZE))
}

fn sys3(n: u32, a0: u64) -> [u64; 6] {
    let _ = n;
    [a0, 0, 0, 0, 0, 0]
}

fn spawn_user(u: &U) -> Option<sched::Tid> {
    let c = sys(syscall::RFORK, RFPROC, 0);
    let parked = sys(syscall::AWAIT, u.h + 64, 64) == 0 && sched::current() == Some(c as sched::Tid);
    (c > 0 && parked).then_some(c as sched::Tid)
}

fn exit_text(u: &U) -> String {
    cstr(ME, u.h + 64)
}

fn notes(r: &mut Report) {
    let want_sp_mod = if uctx::CALL_PUSHES_RETURN { 8 } else { 0 };
    let Some(u) = user_env() else { return };
    let handler = u.code + 8;
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 1, 2);
    regs.gpr[3] = 0xDEAD_BEEF;
    let saved = regs;
    let a = sys(syscall::NOTIFY, handler, 0);
    let b = sys(syscall::NOTIFY, u.h, 0);
    let c = sys(syscall::NOTIFY, 0x10, 0);
    r.check(
        "notify accepts user code, refuses data and bad addresses",
        a == 0 && b == -1 && c == -1 && sched::note_state(ME) == Some((false, handler)),
    );

    sys(syscall::ALARM, 20, 0);
    sched::tick();
    let early = sched::notes_pending(ME);
    sched::tick();
    let queued = sched::notes_pending(ME);
    let res = sched::irq_return(&mut regs);
    let ureg_va = regs.arg0();
    let frame = get_ureg(ME, ureg_va);
    r.check(
        "alarm firing in user mode is delivered at interrupt return, no syscall involved",
        early == 0
            && queued == 1
            && res == Resume::Delivered
            && regs.pc == handler
            && cstr(ME, regs.arg1()) == "alarm"
            && frame == Some(saved)
            && regs.sp < saved.sp - uctx::RED_ZONE
            && regs.sp % 16 == want_sp_mod
            && (!uctx::CALL_PUSHES_RETURN || get(ME, regs.sp, 8) == [0u8; 8])
            && sched::note_state(ME) == Some((true, handler))
            && sched::notes_pending(ME) == 0,
    );

    sys(syscall::ALARM, 10, 0);
    sched::tick();
    let before = regs;
    let waits = sched::irq_return(&mut regs) == Resume::Continue && regs == before && sched::notes_pending(ME) == 1;
    r.check("a second note waits while the handler runs", waits);

    let mut edited = saved;
    edited.pc = u.code + 16;
    edited.flags = !0;
    edited.gpr[3] = 7;
    put(ME, ureg_va, &edited.to_bytes());
    let r1 = dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NCONT), &mut regs);
    let frame2 = get_ureg(ME, regs.arg0());
    let kept = (saved.flags & !uctx::FLAGS_USER_MASK) | (edited.flags & uctx::FLAGS_USER_MASK);
    r.check(
        "noted(NCONT) resumes the edited registers, believing only pc/sp/user flag bits",
        r1 == 0
            && regs.pc == handler
            && frame2.is_some_and(|f| f.pc == u.code + 16 && f.gpr[3] == 7 && f.flags == kept && f.sp == saved.sp)
            && sched::notes_pending(ME) == 0,
    );
    let r2 = dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NCONT), &mut regs);
    r.check(
        "after the last handler returns, the interrupted code continues unchanged",
        r2 == 0 && regs.pc == u.code + 16 && regs.sp == saved.sp && sched::note_state(ME) == Some((false, handler)),
    );

    let mut sc = Ureg::entry(u.code, u.top - 0x100, 0, 0);
    sc.set_ret(0x77);
    let n = dispatch_user(syscall::NSEC, &[0; 6], &mut sc);
    let stored = n > 0 && sc.ret() == n as u64;
    sc.set_ret(0x77);
    let slept = dispatch_user(syscall::SLEEP, &sys3(syscall::SLEEP, 30), &mut sc);
    let parked = slept == 0 && sc.ret() == 0x77 && sched::current().is_none();
    sched::tick();
    sched::tick();
    sched::tick();
    let back = sched::schedule() == Some(ME);
    let res = sched::resume_user(&mut sc);
    r.check(
        "dispatch_user stores the result; a call that slept gets it from resume_user",
        stored && parked && back && res == Resume::Continue && sc.ret() == 0,
    );

    let Some(u) = user_env() else { return };
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    sys(syscall::ALARM, 10, 0);
    sched::tick();
    let res = sched::irq_return(&mut regs);
    let t = exit_text(&u);
    r.check(
        "alarm with no handler kills the thread, exit string \"alarm\"",
        res == Resume::Killed && sched::state_of(c).is_none() && sched::current() == Some(ME) && t.starts_with("2 ") && t.ends_with(" alarm"),
    );

    let Some(u) = user_env() else { return };
    let handler = u.code + 8;
    sys(syscall::NOTIFY, handler, 0);
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    r.check("rfork hands the note handler to the child", sched::note_state(c) == Some((false, handler)));
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    let pc0 = regs.pc;
    let res = sched::fault_return(&mut regs, Fault::Read(0x1234));
    let text = cstr(c, regs.arg1());
    r.check(
        "a user fault becomes a \"sys: trap\" note and runs the handler",
        res == Resume::Delivered && regs.pc == handler && text == alloc::format!("sys: trap: fault read addr=0x1234 pc={:#x}", pc0),
    );
    let hpc = regs.pc;
    let res = sched::fault_return(&mut regs, Fault::Write(0x10));
    let t = exit_text(&u);
    r.check(
        "a fault inside the handler is fatal, exit string is the note plus pc",
        res == Resume::Killed
            && sched::current() == Some(ME)
            && t.ends_with(&alloc::format!("sys: trap: fault write addr=0x10 pc={:#x}", hpc)),
    );

    let Some(u) = user_env() else { return };
    let Some(_) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    let res = sched::fault_return(&mut regs, Fault::DivideError);
    r.check(
        "a fault with no handler kills the thread",
        res == Resume::Killed && exit_text(&u).contains("sys: trap: divide error pc="),
    );

    let Some(u) = user_env() else { return };
    sys(syscall::NOTIFY, u.code + 8, 0);
    let Some(_) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NCONT), &mut regs);
    r.check(
        "noted() when no handler is running is fatal",
        sched::current() == Some(ME) && exit_text(&u).ends_with(" Suicide"),
    );

    let Some(u) = user_env() else { return };
    sys(syscall::NOTIFY, u.code + 8, 0);
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    sched::postnote(c, "hello", NoteKind::User);
    sched::irq_return(&mut regs);
    let save = dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NSAVE), &mut regs);
    let saved = sched::note_state(c) == Some((false, u.code + 8));
    let restore = dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NRSTR), &mut regs);
    r.check(
        "noted(NSAVE) leaves the handler; noted(NRSTR) resumes the saved frame",
        save == 0 && saved && restore == 0 && regs.pc == u.code + 4 && sched::current() == Some(c),
    );

    let Some(u) = user_env() else { return };
    sys(syscall::NOTIFY, u.code + 8, 0);
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    sched::postnote(c, "hello", NoteKind::User);
    sched::irq_return(&mut regs);
    dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NDFLT), &mut regs);
    r.check(
        "noted(NDFLT) takes the note's default action",
        sched::current() == Some(ME) && exit_text(&u).ends_with(" hello"),
    );

    let Some(u) = user_env() else { return };
    sys(syscall::NOTIFY, u.code + 8, 0);
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    sched::postnote(c, "hello", NoteKind::User);
    sched::irq_return(&mut regs);
    let mut evil = Ureg::entry(0xffff_8000_0000_0000, regs.sp, 0, 0);
    evil.sp = u.top - 0x100;
    put(c, regs.arg0(), &evil.to_bytes());
    dispatch_user(syscall::NOTED, &sys3(syscall::NOTED, NCONT), &mut regs);
    r.check(
        "noted() with a kernel address as pc is fatal, not honoured",
        sched::current() == Some(ME) && exit_text(&u).ends_with(" Suicide"),
    );

    let Some(u) = user_env() else { return };
    let handler = u.code + 8;
    sys(syscall::NOTIFY, handler, 0);
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    sys(syscall::ALARM, 20, 0);
    let asleep = sys(syscall::SLEEP, 5000, 0) == 0 && matches!(sched::state_of(c), Some(State::Sleeping(Wait::Timer(_))));
    sched::tick();
    let still_asleep = matches!(sched::state_of(c), Some(State::Sleeping(_)));
    sched::tick();
    let woken = sched::state_of(c) == Some(State::Ready) && sched::pending_ret(c) == Some(-1);
    r.check(
        "an alarm ends a sleep from the timer tick: sleep returns -1, \"interrupted\"",
        asleep && still_asleep && woken && sched::errstr_of(c).as_deref() == Some("interrupted"),
    );
    let ran = sched::schedule() == Some(c);
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    let res = sched::resume_user(&mut regs);
    let frame = get_ureg(c, regs.arg0());
    r.check(
        "on resume the interrupted call's -1 is in the saved registers, then the handler runs",
        ran && res == Resume::Delivered && regs.pc == handler && frame.is_some_and(|f| f.ret() == u64::MAX),
    );

    let Some(u) = user_env() else { return };
    let Some(c) = spawn_user(&u) else { return r.bad("could not set up a user child") };
    let rv = sys(syscall::RENDEZVOUS, 9, 1) == 0 && matches!(sched::state_of(c), Some(State::Sleeping(Wait::Rendezvous(9))));
    let queued = sched::postnote(c, "ping", NoteKind::User);
    let rv_woken = sched::state_of(c) == Some(State::Ready) && sched::pending_ret(c) == Some(-1);
    let parent_waiting = matches!(sched::state_of(ME), Some(State::Sleeping(Wait::Child)));
    let aw = sched::postnote(ME, "ping", NoteKind::User) && sched::state_of(ME) == Some(State::Ready) && sched::pending_ret(ME) == Some(-1);
    r.check(
        "postnote wakes rendezvous and await sleepers with -1; unknown or dead targets fail",
        rv && queued && rv_woken && parent_waiting && aw && !sched::postnote(99, "x", NoteKind::User),
    );

    let Some(u) = user_env() else { return };
    let handler = u.code + 8;
    sys(syscall::NOTIFY, handler, 0);
    let child = sys(syscall::RFORK, RFPROC, 0);
    let posted = sched::postnote(child as sched::Tid, "alarm", NoteKind::User);
    sys(syscall::SLEEP, 0, 0);
    let mut regs = Ureg::entry(u.code + 4, u.top - 0x100, 0, 0);
    let res = sched::resume_user(&mut regs);
    r.check(
        "a note posted to a runnable thread is delivered when it is scheduled in",
        child == 2 && posted && sched::current() == Some(2) && res == Resume::Delivered && regs.pc == handler,
    );

    let Some(_) = user_env() else { return };
    let accepted = (0..7).filter(|_| sched::postnote(ME, "n", NoteKind::User)).count();
    let full = sched::notes_pending(ME) == 5;
    let fatal = sched::postnote(ME, "sys: kill", NoteKind::Exit) && sched::notes_pending(ME) == 5;
    let mut last = None;
    while let Some(n) = sched::take_note(ME) {
        last = Some(n);
    }
    r.check(
        "the queue holds 5 notes: ordinary ones are dropped, a fatal one replaces the newest",
        accepted == 5 && full && fatal && last.is_some_and(|n| n.msg == "sys: kill" && n.kind == NoteKind::Exit),
    );
}

pub fn exec_selftest() {
    let mut r = Report { pass: 0, warn: 0, fail: 0 };
    let free0 = mm::frame_stats().1;
    let Ok(old) = mm::new_address_space() else {
        r.gap("exec", "skipped: no address space (paging unavailable)");
        return;
    };
    if !sched::attach_aspace(ME, old) {
        return;
    }
    let setup = sched::with_aspace(ME, |a| {
        let h = a.brk(0).ok()?;
        a.brk(h + 2 * 4096).ok()?;
        let code = a.map_anywhere(4096, PageFlags::USER_RW, Kind::Anon).ok()?;
        a.protect(code, 4096, PageFlags::USER_EXEC).ok()?;
        Some((h, code))
    })
    .flatten();
    let Some((h, code)) = setup else { return r.bad("exec: could not build the caller's image") };

    let elf_ok = crate::fs::with_root(|_| crate::fs::write_file("/exectest.elf", &pie_image())).is_some_and(|x| x.is_ok());
    let txt_ok = crate::fs::with_root(|_| crate::fs::write_file("/exectest.txt", b"plain text, not an ELF image\n")).is_some_and(|x| x.is_ok());
    if !elf_ok || !txt_ok {
        return r.gap("exec", "skipped: could not write test files to the root file system");
    }

    put(ME, h, b"/exectest.elf\0");
    put(ME, h + 0x40, b"/exectest.txt\0");
    put(ME, h + 0x80, b"/missing\0");
    put(ME, h + 0x200, b"exectest\0");
    put(ME, h + 0x240, b"hello world\0");
    let words: [u64; 3] = [h + 0x200, h + 0x240, 0];
    let mut raw = Vec::new();
    for w in words {
        raw.extend_from_slice(&w.to_le_bytes());
    }
    put(ME, h + 0x100, &raw);
    put(ME, h + 0x300, b"keep me\0");

    let nofile = sys_a(syscall::EXEC, h + 0x80, h + 0x100);
    let e1 = sched::errstr_of(ME).unwrap_or_default();
    let notelf = sys_a(syscall::EXEC, h + 0x40, h + 0x100);
    let e2 = sched::errstr_of(ME).unwrap_or_default();
    let badptr = sys_a(syscall::EXEC, 0x10, 0);
    let badargv = sys_a(syscall::EXEC, h, h + 0x1000_0000);
    if !(nofile == -1 && e1 == "file does not exist" && notelf == -1 && e2 == "exec header invalid" && badptr == -1 && badargv == -1) {
        kprintln!(
            "ifsy: abi: exec diag: missing={} ({:?}) not-elf={} ({:?}) badptr={} badargv={}",
            nofile, e1, notelf, e2, badptr, badargv
        );
    }
    r.check(
        "exec failures return -1 with a reason and leave the old image intact",
        nofile == -1 && e1 == "file does not exist" && notelf == -1 && e2 == "exec header invalid" && badptr == -1 && badargv == -1
            && cstr(ME, h + 0x300) == "keep me",
    );

    sys_a(syscall::NOTIFY, code, 0);
    let mut regs = Ureg::zeroed();
    let ret = dispatch_user(syscall::EXEC, &[h, h + 0x100, 0, 0, 0, 0], &mut regs);
    let base = sched::with_aspace(ME, |a| a.layout().image_base).unwrap_or(0);
    let entry_ok = regs.pc == base + 0x10;
    let argc_argv = {
        let sp = regs.sp;
        let w = |va: u64| {
            let b = get(ME, va, 8);
            if b.len() == 8 { u64::from_le_bytes(b[..8].try_into().unwrap()) } else { u64::MAX }
        };
        let argv = regs.arg1();
        regs.arg0() == 2 && sp % 16 == 0 && w(sp) == 2 && argv == sp + 8 && cstr(ME, w(argv)) == "exectest"
            && cstr(ME, w(argv + 8)) == "hello world" && w(argv + 16) == 0
    };
    let fresh = get(ME, h + 0x300, 8).is_empty();
    let text = get(ME, base, uctx::UREG_SIZE.min(TEXT_LEN));
    r.check(
        "exec loads an ELF from ext4: registers start at the entry, argc/argv on the new stack",
        ret == 0 && entry_ok && argc_argv && fresh && text == text_bytes() && regs.flags == uctx::FLAGS_INIT,
    );
    r.check(
        "exec clears the note handler and releases the old image",
        sched::note_state(ME) == Some((false, 0)) && sched::take_start(ME).is_none(),
    );

    sched::detach_aspace(ME);
    let _ = crate::fs::with_root(|_| {
        let _ = crate::fs::remove_file("/exectest.elf");
        let _ = crate::fs::remove_file("/exectest.txt");
    });
    r.check("exec leaks no frames", mm::frame_stats().1 == free0);

    let tag = if r.fail > 0 { failed() } else if r.warn > 0 { warn() } else { pass() };
    kprintln!("ifsy: abi: exec: {} pass, {} warn, {} fail {}", r.pass, r.warn, r.fail, tag);
}

fn sys_a(n: u32, a0: u64, a1: u64) -> i64 {
    dispatch_args(n, &[a0, a1, 0, 0, 0, 0])
}
