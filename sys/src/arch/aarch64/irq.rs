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

use core::arch::{asm, global_asm};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use super::serial::MMIO_BASE;

const GICD: usize = MMIO_BASE + 0x0800_0000;
const GICC_V2: usize = MMIO_BASE + 0x0801_0000;
const GICR: usize = MMIO_BASE + 0x080a_0000;
const GICR_SGI: usize = GICR + 0x1_0000;

const INTID_PTIMER: u32 = 30;
const INTID_SPURIOUS: u32 = 1023;

const KIND_NONE: u8 = 0;
const KIND_V2: u8 = 2;
const KIND_V3: u8 = 3;
static KIND: AtomicU8 = AtomicU8::new(KIND_NONE);
static STRAY: AtomicU32 = AtomicU32::new(0);

unsafe fn rd(addr: usize) -> u32 {
    unsafe { read_volatile(addr as *const u32) }
}
unsafe fn wr(addr: usize, v: u32) {
    unsafe { write_volatile(addr as *mut u32, v) };
}

unsafe fn gic_kind() -> u8 {
    match unsafe { (rd(GICD + 0xfe8) >> 4) & 0xf } {
        1 | 2 => KIND_V2,
        3 | 4 => KIND_V3,
        _ => KIND_NONE,
    }
}

unsafe fn gicv2_enable() {
    unsafe {
        wr(GICD, 0);
        write_volatile((GICD + 0x400 + INTID_PTIMER as usize) as *mut u8, 0x80);
        wr(GICD + 0x100, 1 << INTID_PTIMER);
        wr(GICD, 1);
        wr(GICC_V2 + 0x04, 0xff);
        wr(GICC_V2 + 0x00, 1);
    }
}

unsafe fn gicv3_enable() -> bool {
    unsafe {
        let mut sre: u64;
        asm!("mrs {}, S3_0_C12_C12_5", out(reg) sre, options(nomem, nostack, preserves_flags));
        sre |= 1;
        asm!("msr S3_0_C12_C12_5, {}", "isb", in(reg) sre, options(nomem, nostack, preserves_flags));
        let back: u64;
        asm!("mrs {}, S3_0_C12_C12_5", out(reg) back, options(nomem, nostack, preserves_flags));
        if back & 1 == 0 {
            return false;
        }

        wr(GICD, (1 << 4) | (1 << 1));
        let mut spins = 0u32;
        while rd(GICD) & (1 << 31) != 0 && spins < 1_000_000 {
            spins += 1;
        }

        let waker = rd(GICR + 0x14) & !(1 << 1);
        wr(GICR + 0x14, waker);
        let mut spins = 0u32;
        while rd(GICR + 0x14) & (1 << 2) != 0 {
            spins += 1;
            if spins > 1_000_000 {
                return false;
            }
        }

        wr(GICR_SGI + 0x080, rd(GICR_SGI + 0x080) | (1 << INTID_PTIMER));
        write_volatile((GICR_SGI + 0x400 + INTID_PTIMER as usize) as *mut u8, 0x80);
        wr(GICR_SGI + 0x100, 1 << INTID_PTIMER);

        asm!("msr S3_0_C4_C6_0, {}", in(reg) 0xffu64, options(nomem, nostack, preserves_flags));
        asm!("msr S3_0_C12_C12_7, {}", "isb", in(reg) 1u64, options(nomem, nostack, preserves_flags));
    }
    true
}

unsafe fn ack() -> u32 {
    match KIND.load(Ordering::Relaxed) {
        KIND_V3 => {
            let v: u64;
            unsafe { asm!("mrs {}, S3_0_C12_C12_0", out(reg) v, options(nomem, nostack, preserves_flags)) };
            v as u32
        }
        _ => unsafe { rd(GICC_V2 + 0x0c) },
    }
}

unsafe fn eoi(iar: u32) {
    match KIND.load(Ordering::Relaxed) {
        KIND_V3 => unsafe {
            asm!("msr S3_0_C12_C12_1, {}", in(reg) iar as u64, options(nomem, nostack, preserves_flags));
        },
        _ => unsafe { wr(GICC_V2 + 0x10, iar) },
    }
}

unsafe fn arm(deadline: u64) {
    unsafe {
        asm!(
            "msr cntp_cval_el0, {d}",
            "msr cntp_ctl_el0, {en}",
            "isb",
            d = in(reg) deadline,
            en = in(reg) 1u64,
            options(nomem, nostack, preserves_flags),
        );
    }
}

pub unsafe fn start(first_deadline: u64) -> bool {
    let kind = unsafe { gic_kind() };
    let ok = match kind {
        KIND_V2 => {
            unsafe { gicv2_enable() };
            true
        }
        KIND_V3 => unsafe { gicv3_enable() },
        _ => false,
    };
    if !ok {
        return false;
    }
    KIND.store(kind, Ordering::Relaxed);
    unsafe { arm(first_deadline) };
    true
}

pub unsafe fn stop() {
    unsafe { asm!("msr cntp_ctl_el0, xzr", "isb", options(nomem, nostack, preserves_flags)) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn aarch64_irq_handler() {
    let iar = unsafe { ack() };
    let intid = iar & 0x3ff;
    if intid == INTID_SPURIOUS {
        return;
    }
    if intid == INTID_PTIMER {
        let next = crate::timer::irq();
        unsafe { arm(next) };
    } else {
        STRAY.fetch_add(1, Ordering::Relaxed);
    }
    unsafe { eoi(iar) };
    if intid == INTID_PTIMER {
        crate::sched::preempt();
    }
}

pub unsafe fn user_tick() {
    unsafe { aarch64_irq_handler() };
}

#[allow(dead_code)]
pub fn stray_count() -> u32 {
    STRAY.load(Ordering::Relaxed)
}

global_asm!(
    r#"
.section .text
.global aarch64_irq_entry
.balign 4
aarch64_irq_entry:
    sub  sp, sp, #592
    stp  x0,  x1,  [sp, #0]
    stp  x2,  x3,  [sp, #16]
    stp  x4,  x5,  [sp, #32]
    stp  x6,  x7,  [sp, #48]
    stp  x8,  x9,  [sp, #64]
    stp  x10, x11, [sp, #80]
    stp  x12, x13, [sp, #96]
    stp  x14, x15, [sp, #112]
    stp  x16, x17, [sp, #128]
    stp  x18, x29, [sp, #144]
    str  x30,      [sp, #160]
    mrs  x0, elr_el1
    mrs  x1, spsr_el1
    stp  x0, x1,   [sp, #176]
    mrs  x0, fpsr
    mrs  x1, fpcr
    stp  x0, x1,   [sp, #192]
    stp  q0,  q1,  [sp, #208]
    stp  q2,  q3,  [sp, #240]
    stp  q4,  q5,  [sp, #272]
    stp  q6,  q7,  [sp, #304]
    stp  q16, q17, [sp, #336]
    stp  q18, q19, [sp, #368]
    stp  q20, q21, [sp, #400]
    stp  q22, q23, [sp, #432]
    stp  q24, q25, [sp, #464]
    stp  q26, q27, [sp, #496]
    stp  q28, q29, [sp, #528]
    stp  q30, q31, [sp, #560]

    bl   aarch64_irq_handler

    ldp  q30, q31, [sp, #560]
    ldp  q28, q29, [sp, #528]
    ldp  q26, q27, [sp, #496]
    ldp  q24, q25, [sp, #464]
    ldp  q22, q23, [sp, #432]
    ldp  q20, q21, [sp, #400]
    ldp  q18, q19, [sp, #368]
    ldp  q16, q17, [sp, #336]
    ldp  q6,  q7,  [sp, #304]
    ldp  q4,  q5,  [sp, #272]
    ldp  q2,  q3,  [sp, #240]
    ldp  q0,  q1,  [sp, #208]
    ldp  x0, x1,   [sp, #192]
    msr  fpsr, x0
    msr  fpcr, x1
    ldp  x0, x1,   [sp, #176]
    msr  elr_el1, x0
    msr  spsr_el1, x1
    ldr  x30,      [sp, #160]
    ldp  x18, x29, [sp, #144]
    ldp  x16, x17, [sp, #128]
    ldp  x14, x15, [sp, #112]
    ldp  x12, x13, [sp, #96]
    ldp  x10, x11, [sp, #80]
    ldp  x8,  x9,  [sp, #64]
    ldp  x6,  x7,  [sp, #48]
    ldp  x4,  x5,  [sp, #32]
    ldp  x2,  x3,  [sp, #16]
    ldp  x0,  x1,  [sp, #0]
    add  sp, sp, #592
    eret
"#
);
