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
use core::ptr::addr_of_mut;

use super::gdt::{self, KERNEL_CS, STAR_USER_BASE};
use super::trap;
use crate::arch::Hal;
use crate::uctx::Ureg;

const MSR_EFER: u32 = 0xC000_0080;
const MSR_STAR: u32 = 0xC000_0081;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_SFMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1 << 0;

const SFMASK: u64 = (1 << 9) | (1 << 8) | (1 << 10) | (1 << 14) | (1 << 18);

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;

unsafe extern "C" {
    fn syscall_entry();
    static mut x86_kernel_rsp: u64;
}

pub struct Port;

#[repr(C, align(16))]
#[derive(Clone)]
pub struct FpuState([u8; 512]);

unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    (u64::from(hi) << 32) | u64::from(lo)
}

unsafe fn wrmsr(msr: u32, v: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
             options(nomem, nostack, preserves_flags))
    };
}

impl Hal for Port {
    type FpuState = FpuState;

    const SYSCALL_ABI: &'static str =
        "Plan 9 amd64: `syscall` or `int $64`, number in rbp, arguments on the user stack from 8(%rsp), result in rax";

    unsafe fn init_user_mode() -> bool {
        let (max_ext, ..) = cpuid(0x8000_0000);
        if max_ext < 0x8000_0001 || cpuid(0x8000_0001).3 & (1 << 11) == 0 {
            crate::kprintln!("ifsy: user: x86_64: CPU has no SYSCALL instruction {}", crate::console::color::failed());
            return false;
        }

        unsafe {
            let (mut cr0, mut cr4): (u64, u64);
            asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
            asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
            cr0 = (cr0 & !CR0_EM) | CR0_MP;
            cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;
            asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
            asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
            asm!("clts", options(nomem, nostack, preserves_flags));
            asm!("fninit", options(nomem, nostack));

            wrmsr(MSR_EFER, rdmsr(MSR_EFER) | EFER_SCE);
            wrmsr(MSR_STAR, (u64::from(STAR_USER_BASE) << 48) | (u64::from(KERNEL_CS) << 32));
            wrmsr(MSR_LSTAR, syscall_entry as *const () as u64);
            wrmsr(MSR_SFMASK, SFMASK);
        }

        crate::kprintln!(
            "ifsy: user: x86_64: SSE on, SYSCALL armed (STAR {:#x} LSTAR {:#x} SFMASK {:#x}); ABI: {} {}",
            (u64::from(STAR_USER_BASE) << 48) | (u64::from(KERNEL_CS) << 32),
            syscall_entry as *const () as usize,
            SFMASK,
            Self::SYSCALL_ABI,
            crate::console::color::pass()
        );
        true
    }

    fn set_kernel_stack(top: u64) {
        gdt::set_kernel_stack(top);
        unsafe { addr_of_mut!(x86_kernel_rsp).write_volatile(top) };
    }

    unsafe fn enter_user(regs: &Ureg) -> ! {
        unsafe { trap::enter_user(regs) }
    }

    fn fpu_initial() -> FpuState {
        let mut s = FpuState([0; 512]);
        s.0[0..2].copy_from_slice(&0x037Fu16.to_le_bytes());
        s.0[24..28].copy_from_slice(&0x1F80u32.to_le_bytes());
        s
    }

    unsafe fn fpu_save(state: &mut FpuState) {
        unsafe { asm!(".byte 0x48, 0x0f, 0xae, 0x07", in("rdi") state.0.as_mut_ptr(), options(nostack, preserves_flags)) };
    }

    unsafe fn fpu_restore(state: &FpuState) {
        unsafe { asm!(".byte 0x48, 0x0f, 0xae, 0x0f", in("rdi") state.0.as_ptr(), options(nostack, preserves_flags)) };
    }
}

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "mov {t:r}, rbx",
            "cpuid",
            "xchg {t:r}, rbx",
            t = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}
