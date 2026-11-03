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
use core::arch::x86_64::{__cpuid, __cpuid_count, _rdtsc};

pub const ASID_BITS: u32 = 0;

pub fn kernel_root() -> PhysAddr {
    read_root()
}

pub fn translation_ok() -> bool {
    let cr4: u64;
    unsafe { asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags)) };
    cr4 & (1 << 12) == 0
}

pub fn publish() {}

pub fn flush_page(virt: VirtAddr) {
    unsafe { asm!("invlpg [{}]", in(reg) virt.0, options(nostack, preserves_flags)) };
}

pub fn flush_all() {
    unsafe {
        let cr3: u64;
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
    }
}

pub fn read_root() -> PhysAddr {
    let cr3: u64;
    unsafe { asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags)) };
    PhysAddr(cr3 & 0x000f_ffff_ffff_f000)
}

pub unsafe fn switch_root(root: PhysAddr, _asid: u16) {
    unsafe { asm!("mov cr3, {}", in(reg) root.0, options(nostack, preserves_flags)) };
}

#[target_feature(enable = "rdrand")]
unsafe fn rdrand() -> Option<u64> {
    for _ in 0..10 {
        let mut v = 0u64;
        if unsafe { core::arch::x86_64::_rdrand64_step(&mut v) } == 1 {
            return Some(v);
        }
    }
    None
}

#[target_feature(enable = "rdseed")]
unsafe fn rdseed() -> Option<u64> {
    for _ in 0..100 {
        let mut v = 0u64;
        if unsafe { core::arch::x86_64::_rdseed64_step(&mut v) } == 1 {
            return Some(v);
        }
    }
    None
}

pub fn entropy() -> (u64, bool) {
    let (max_leaf, has_rdrand) = unsafe {
        let l0 = __cpuid(0);
        let l1 = __cpuid(1);
        (l0.eax, l1.ecx & (1 << 30) != 0)
    };
    let has_rdseed = max_leaf >= 7 && unsafe { __cpuid_count(7, 0).ebx } & (1 << 18) != 0;
    unsafe {
        if has_rdseed {
            if let Some(v) = rdseed() {
                return (v, true);
            }
        }
        if has_rdrand {
            if let Some(v) = rdrand() {
                return (v, true);
            }
        }
        (_rdtsc(), false)
    }
}
