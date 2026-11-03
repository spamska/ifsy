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

use crate::arch::CAPS;
use crate::console::color::{failed, pass, warn};
use crate::mm::aspace::Kind;
use crate::mm::{self, PageFlags};
use crate::sched;

fn program() -> &'static [u8] {
    crate::arch::port::usermode::test_program()
}

const CODE_OFF: usize = 0x80;

fn elf_image(code: &[u8]) -> Vec<u8> {
    let mut out = alloc::vec![0u8; CODE_OFF];
    out.extend_from_slice(code);
    let len = out.len() as u64;
    out[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    out[4] = 2;
    out[5] = 1;
    out[6] = 1;
    out[16..18].copy_from_slice(&3u16.to_le_bytes());
    out[18..20].copy_from_slice(&crate::elf::MACHINE.to_le_bytes());
    out[20..24].copy_from_slice(&1u32.to_le_bytes());
    out[24..32].copy_from_slice(&(CODE_OFF as u64).to_le_bytes());
    out[32..40].copy_from_slice(&64u64.to_le_bytes());
    out[52..54].copy_from_slice(&64u16.to_le_bytes());
    out[54..56].copy_from_slice(&56u16.to_le_bytes());
    out[56..58].copy_from_slice(&1u16.to_le_bytes());
    let ph = 64;
    out[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
    out[ph + 4..ph + 8].copy_from_slice(&5u32.to_le_bytes());
    out[ph + 8..ph + 16].copy_from_slice(&0u64.to_le_bytes());
    out[ph + 16..ph + 24].copy_from_slice(&0u64.to_le_bytes());
    out[ph + 24..ph + 32].copy_from_slice(&0u64.to_le_bytes());
    out[ph + 32..ph + 40].copy_from_slice(&len.to_le_bytes());
    out[ph + 40..ph + 48].copy_from_slice(&len.to_le_bytes());
    out[ph + 48..ph + 56].copy_from_slice(&4096u64.to_le_bytes());
    out
}

pub fn selftest() -> bool {
    if !CAPS.user_mode {
        kprintln!("ifsy: user: ring 3 test skipped (no user mode on this architecture yet) {}", warn());
        return false;
    }
    if !sched::switching_active() {
        kprintln!("ifsy: user: ring 3 test skipped (context switching is off) {}", warn());
        return false;
    }
    let image = elf_image(program());
    let wrote = crate::fs::with_root(|_| crate::fs::write_file("/init", &image)).is_some_and(|r| r.is_ok());
    if !wrote {
        kprintln!("ifsy: user: ring 3 test skipped (could not write /init to the root file system) {}", warn());
        return false;
    }
    if sched::spawn(test_main, 0).is_none() {
        kprintln!("ifsy: user: could not start the test thread {}", failed());
        return false;
    }
    true
}

fn run(me: sched::Tid, buf: u64, path: &str, args: &[&str], timeout_ms: i64) -> Result<String, &'static str> {
    let t0 = crate::timer::uptime_ms();
    sched::spawn_user(path, args).ok_or("could not start the process")?;
    sched::sys_alarm(timeout_ms);
    let n = sched::sys_wait(buf, 192);
    let n = sched::finish_blocked_syscall(me).unwrap_or(n);
    sched::sys_alarm(0);
    let took = crate::timer::uptime_ms() - t0;
    if n <= 0 {
        kprintln!("ifsy: user: await returned {} after {} ms (errstr {:?})", n, took, sched::errstr_of(me).unwrap_or_default());
        sched::dump_threads();
        return Err("no exit message (timed out or interrupted)");
    }
    kprintln!("ifsy: user: process ended after {} ms", took);
    let mut raw = alloc::vec![0u8; 192];
    let read = sched::with_aspace(me, |a| a.read_bytes(buf, &mut raw).is_ok()).unwrap_or(false);
    if !read {
        return Err("could not read the exit record");
    }
    let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let text = String::from_utf8_lossy(&raw[..len]).into_owned();
    Ok(text.splitn(5, ' ').nth(4).map(String::from).unwrap_or_default())
}

fn test_main(_: usize) {
    let Some(me) = sched::current() else { return };
    let buf = mm::new_address_space().ok().and_then(|space| {
        sched::attach_aspace(me, space).then_some(())?;
        sched::with_aspace(me, |a| a.map_anywhere(4096, PageFlags::USER_RW, Kind::Anon).ok()).flatten()
    });
    let Some(buf) = buf else {
        kprintln!("ifsy: user: ring 3 test skipped (no address space for the test thread) {}", warn());
        crate::attach_persistent_root();
        return;
    };
    let free0 = mm::frame_stats().1;

    crate::console::set_muted(true);
    let mut ok = true;
    let mut report = |name: &str, got: Result<String, &'static str>, expect: &str| {
        let good = matches!(&got, Ok(m) if m.contains(expect));
        match &got {
            Ok(m) => kprintln!("ifsy: user: {} -> {:?} {}", name, m, if good { pass() } else { failed() }),
            Err(e) => kprintln!("ifsy: user: {} -> {} {}", name, e, failed()),
        }
        ok &= good;
    };

    report("ring 3: syscall, int $64, SSE across switches, rfork, await, console I/O", run(me, buf, "/init", &["init"], 30_000), "ring3 ok");
    report("ring 3: page fault becomes a note", run(me, buf, "/init", &["init", "crash"], 30_000), "sys: trap: fault read addr=0x0");

#[cfg(feature = "rc-selftest")]
    {
        if crate::userland::rc_ready() {
            report(
                "rc: scripted self-test (/rc/selftest)",
                run(me, buf, "/bin/rc", &["rc", "/rc/selftest"], 120_000),
                "rcselftest:ok",
            );
        } else {
            kprintln!("ifsy: user: rc self-test skipped (/bin/rc and /rc/selftest were not installed) {}", warn());
        }
    }

    sched::yield_now();
    sched::yield_now();
    let free1 = mm::frame_stats().1;
    if free0.abs_diff(free1) > 16 {
        kprintln!("ifsy: user: frames not returned after the user programs exited (free {} -> {}) {}", free0, free1, warn());
    }
    crate::console::set_muted(false);
    if !ok {
        kprintln!("ifsy: user: output of the programs during the tests:");
        crate::console::flush_held();
    }
    kprintln!("ifsy: user: ring 3 self-test {}", if ok { pass() } else { failed() });

    crate::attach_persistent_root();
    shell(me, buf);
}

fn shell(me: sched::Tid, buf: u64) {
    if !crate::userland::rc_ready() {
        kprintln!("ifsy: user: no shell: /bin/rc was not installed {}", warn());
        return;
    }
    if crate::console::failures() == 0 && !crate::console::verbose() {
        crate::console::clear_screen();
    } else {
        kprintln!("ifsy: boot: screen left as is: some self-tests failed (or verbose-boot is on)");
    }
    crate::console::enable_cursor();
    // /bin/init  runs the services and the login prompt. If it is missing or keeps
    // failing, fall back to a bare root shell on the console, as before.
    let mut use_init = crate::fs::with_root(|_| crate::fs::read_file("/bin/init").is_ok()).unwrap_or(false);
    let mut bad = 0;
    loop {
        let t0 = crate::timer::uptime_ms();
        let started = if use_init {
            sched::spawn_user("/bin/init", &["init"])
        } else {
            sched::spawn_user("/bin/rc", &["rc", "-i"])
        };
        if started.is_none() {
            kprintln!("ifsy: user: could not start {} {}", if use_init { "init" } else { "rc" }, failed());
            return;
        }
        let n = sched::sys_wait(buf, 192);
        let n = sched::finish_blocked_syscall(me).unwrap_or(n);
        if n <= 0 || crate::timer::uptime_ms() - t0 < 500 {
            bad += 1;
            if use_init && bad >= 2 {
                kprintln!("ifsy: user: init keeps failing; falling back to a root shell {}", warn());
                use_init = false;
                bad = 0;
            }
            if bad >= 5 {
                kprintln!("ifsy: user: rc keeps failing; giving up {}", failed());
                return;
            }
            sched::sys_sleep(100);
            let _ = sched::finish_blocked_syscall(me);
        } else {
            bad = 0;
        }
    }
}
