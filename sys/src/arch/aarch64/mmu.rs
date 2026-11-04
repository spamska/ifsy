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

pub const KERNEL_SHARED_FROM: Option<usize> = None;

use crate::mm::{PhysAddr, VirtAddr};
use core::arch::asm;

pub fn kernel_root() -> PhysAddr {
    let v: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mrs {}, ttbr1_el1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    PhysAddr(v & 0x0000_ffff_ffff_f000)
}

pub fn translation_ok() -> bool {
    let tcr: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack, preserves_flags)) };
    (tcr >> 16) & 0x3f == 16 && (tcr >> 30) & 3 == 0b10
}

pub fn publish() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("dsb ishst", "isb", options(nostack, preserves_flags)) };
}

pub const ASID_BITS: u32 = 8;

pub fn flush_page(virt: VirtAddr) {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "dsb ishst",
            "tlbi vaae1is, {}",
            "dsb ish",
            "isb",
            in(reg) virt.0 >> 12,
            options(nostack, preserves_flags)
        )
    };
}

pub fn flush_all() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("dsb ishst", "tlbi vmalle1is", "dsb ish", "isb", options(nostack, preserves_flags)) };
}

pub fn read_root() -> PhysAddr {
    let v: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mrs {}, ttbr0_el1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    PhysAddr(v & 0x0000_ffff_ffff_f000)
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn switch_root(root: PhysAddr, asid: u16) {
    let v = root.0 | ((asid as u64) << 48);
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("msr ttbr0_el1, {}", "isb", in(reg) v, options(nostack, preserves_flags)) };
}

pub fn entropy() -> (u64, bool) {
    let isar0: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack, preserves_flags)) };
    if (isar0 >> 60) & 0xf != 0 {
        for _ in 0..10 {
            let (v, nzcv): (u64, u64);
            // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
            unsafe {
                asm!("mrs {v}, s3_3_c2_c4_0", "mrs {f}, nzcv", v = out(reg) v, f = out(reg) nzcv,
                     options(nomem, nostack))
            };
            if nzcv & (1 << 30) == 0 {
                return (v, true);
            }
        }
    }
    let t: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mrs {}, cntpct_el0", out(reg) t, options(nomem, nostack, preserves_flags)) };
    (t, false)
}
