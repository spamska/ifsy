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

pub const KERNEL_SHARED_FROM: Option<usize> = Some(256);

use crate::mm::{PhysAddr, VirtAddr};
use core::arch::asm;

pub const ASID_BITS: u32 = 16;
const SATP_SV48: u64 = 9 << 60;

pub fn kernel_root() -> PhysAddr {
    read_root()
}

pub fn translation_ok() -> bool {
    let v: u64;
    unsafe { asm!("csrr {}, satp", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v >> 60 == 9
}

pub fn publish() {
    unsafe { asm!("fence rw, rw", options(nostack, preserves_flags)) };
}

pub fn flush_page(virt: VirtAddr) {
    unsafe { asm!("sfence.vma {}, zero", in(reg) virt.0, options(nostack, preserves_flags)) };
}

pub fn flush_all() {
    unsafe { asm!("sfence.vma zero, zero", options(nostack, preserves_flags)) };
}

pub fn read_root() -> PhysAddr {
    let v: u64;
    unsafe { asm!("csrr {}, satp", out(reg) v, options(nomem, nostack, preserves_flags)) };
    PhysAddr((v & 0x0000_0fff_ffff_ffff) << 12)
}

pub unsafe fn switch_root(root: PhysAddr, asid: u16) {
    let satp = SATP_SV48 | ((asid as u64) << 44) | (root.0 >> 12);
    unsafe { asm!("csrw satp, {}", "sfence.vma zero, zero", in(reg) satp, options(nostack)) };
}

pub fn entropy() -> (u64, bool) {
    let t: u64;
    unsafe { asm!("rdtime {}", out(reg) t, options(nomem, nostack, preserves_flags)) };
    (t, false)
}
