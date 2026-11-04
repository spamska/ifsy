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

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

const SBI_EXT_BASE: u64 = 0x10;
const SBI_BASE_PROBE: u64 = 3;
const SBI_EXT_TIME: u64 = 0x5449_4D45;

const STIE: u64 = 1 << 5;
const IRQ_S_TIMER: u64 = 5;

static STRAY: AtomicU32 = AtomicU32::new(0);

fn sbi(eid: u64, fid: u64, a0: u64) -> (i64, u64) {
    let (err, val): (i64, u64);
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!("ecall", in("a7") eid, in("a6") fid, inlateout("a0") a0 => err, lateout("a1") val,
             options(nostack));
    }
    (err, val)
}

fn set_timer(deadline: u64) {
    let _ = sbi(SBI_EXT_TIME, 0, deadline);
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn start(first_deadline: u64) -> bool {
    let (err, present) = sbi(SBI_EXT_BASE, SBI_BASE_PROBE, SBI_EXT_TIME);
    if err != 0 || present == 0 {
        return false;
    }
    set_timer(first_deadline);
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("csrs sie, {}", in(reg) STIE, options(nomem, nostack, preserves_flags)) };
    true
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn stop() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("csrc sie, {}", in(reg) STIE, options(nomem, nostack, preserves_flags)) };
    set_timer(u64::MAX);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn riscv_irq_handler(cause: u64) {
    let code = cause & !(1u64 << 63);
    if code == IRQ_S_TIMER {
        timer_tick();
        crate::sched::preempt();
    } else {
        stray(code);
    }
}

pub fn timer_tick() {
    let next = crate::timer::irq();
    set_timer(next);
}

pub fn stray(code: u64) {
    STRAY.fetch_add(1, Ordering::Relaxed);
    if code < 64 {
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("csrc sie, {}", in(reg) 1u64 << code, options(nomem, nostack, preserves_flags)) };
    }
}

#[allow(dead_code)]
pub fn stray_count() -> u32 {
    STRAY.load(Ordering::Relaxed)
}
