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
use core::ptr::{addr_of, addr_of_mut};

pub const KERNEL_CS: u16 = 0x08;
pub const KERNEL_DS: u16 = 0x10;
pub const USER_DS: u16 = 0x18 | 3;
pub const USER_CS: u16 = 0x20 | 3;
const TSS_SEL: u16 = 0x28;

pub const STAR_USER_BASE: u16 = 0x10;

pub const IST_DOUBLE_FAULT: u8 = 1;
pub const IST_NMI: u8 = 2;
pub const IST_MACHINE_CHECK: u8 = 3;

const IST_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct IstStack([u8; IST_STACK_SIZE]);

static mut IST1: IstStack = IstStack([0; IST_STACK_SIZE]);
static mut IST2: IstStack = IstStack([0; IST_STACK_SIZE]);
static mut IST3: IstStack = IstStack([0; IST_STACK_SIZE]);

const TSS_SIZE: usize = 104;
const TSS_RSP0: usize = 4;
const TSS_IST1: usize = 36;
const TSS_IOMAP_BASE: usize = 102;

#[repr(C, align(16))]
struct Tss([u8; TSS_SIZE]);

static mut TSS: Tss = Tss([0; TSS_SIZE]);

const GDT_LEN: usize = 7;

#[repr(C, align(16))]
struct Gdt([u64; GDT_LEN]);

static mut GDT: Gdt = Gdt([0; GDT_LEN]);

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

fn tss_write_u64(offset: usize, value: u64) {
    unsafe {
        let p = (addr_of_mut!(TSS.0) as *mut u8).add(offset) as *mut u64;
        p.write_unaligned(value);
    }
}

fn tss_read_u64(offset: usize) -> u64 {
    unsafe {
        let p = (addr_of!(TSS.0) as *const u8).add(offset) as *const u64;
        p.read_unaligned()
    }
}

pub fn set_kernel_stack(top: u64) {
    tss_write_u64(TSS_RSP0, top);
}

pub fn kernel_stack() -> u64 {
    tss_read_u64(TSS_RSP0)
}

fn stack_top(base: *const IstStack) -> u64 {
    base as u64 + IST_STACK_SIZE as u64
}

pub unsafe fn init() {
    let (ist1, ist2, ist3) = unsafe { (stack_top(addr_of!(IST1)), stack_top(addr_of!(IST2)), stack_top(addr_of!(IST3))) };
    tss_write_u64(TSS_IST1, ist1);
    tss_write_u64(TSS_IST1 + 8, ist2);
    tss_write_u64(TSS_IST1 + 16, ist3);
    unsafe {
        let p = (addr_of_mut!(TSS.0) as *mut u8).add(TSS_IOMAP_BASE) as *mut u16;
        p.write_unaligned(TSS_SIZE as u16);
    }

    let tss_base = addr_of!(TSS) as u64;
    let tss_limit = (TSS_SIZE - 1) as u64;
    let tss_low = (tss_limit & 0xffff)
        | ((tss_base & 0x00ff_ffff) << 16)
        | (0x89u64 << 40)
        | (((tss_limit >> 16) & 0xf) << 48)
        | (((tss_base >> 24) & 0xff) << 56);
    let tss_high = tss_base >> 32;

    let gdt = unsafe { &mut *addr_of_mut!(GDT) };
    gdt.0[0] = 0;
    gdt.0[1] = 0x00af_9a00_0000_ffff;
    gdt.0[2] = 0x00cf_9200_0000_ffff;
    gdt.0[3] = 0x00cf_f200_0000_ffff;
    gdt.0[4] = 0x00af_fa00_0000_ffff;
    gdt.0[5] = tss_low;
    gdt.0[6] = tss_high;

    let ptr = DescriptorPointer { limit: (GDT_LEN * 8 - 1) as u16, base: addr_of!(GDT) as u64 };
    unsafe {
        asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
        asm!(
            "push {sel}",
            "lea {tmp}, [2f + rip]",
            "push {tmp}",
            "retfq",
            "2:",
            sel = in(reg) u64::from(KERNEL_CS),
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        asm!(
            "mov ds, {0:x}",
            "mov es, {0:x}",
            "mov ss, {0:x}",
            in(reg) KERNEL_DS,
            options(nostack, preserves_flags),
        );
        asm!("ltr {0:x}", in(reg) TSS_SEL, options(nostack, preserves_flags));
    }

    crate::kprintln!(
        "ifsy: hardening: x86_64: GDT + TSS loaded (user CS {:#x} SS {:#x}, IST: #DF/NMI/#MC) {}",
        USER_CS,
        USER_DS,
        crate::console::color::pass()
    );
}
