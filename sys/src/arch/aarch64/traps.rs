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

unsafe extern "C" {
    static vector_table_el1: u8;
}

pub fn exception_class_name(ec: u64) -> &'static str {
    match ec {
        0b000000 => "Unknown",
        0b000111 => "SVE/SIMD/FP access trap",
        0b010101 => "SVC instruction (AArch64)",
        0b100000 => "Instruction Abort, lower EL",
        0b100001 => "Instruction Abort, same EL",
        0b100010 => "PC alignment fault",
        0b100100 => "Data Abort, lower EL",
        0b100101 => "Data Abort, same EL",
        0b100110 => "SP alignment fault",
        0b101100 => "Trapped floating-point exception",
        0b101111 => "SError interrupt",
        0b110000 => "Breakpoint, lower EL",
        0b110001 => "Breakpoint, same EL",
        0b111100 => "BRK instruction",
        _ => "(see ESR_EL1.EC)",
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn aarch64_trap_common(vector: u64) -> ! {
    let (esr, far, elr, spsr): (u64, u64, u64, u64);
    unsafe {
        asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, elr_el1", out(reg) elr, options(nomem, nostack, preserves_flags));
        asm!("mrs {}, spsr_el1", out(reg) spsr, options(nomem, nostack, preserves_flags));
    }
    let ec = (esr >> 26) & 0x3f;
    let iss = esr & 0x01ff_ffff;
    crate::kprintln!(
        "ifsy: {}: aarch64 vector {} ec={:#x} ({}) iss={:#x} esr={:#x} far={:#x} elr={:#x} spsr={:#x}",
        crate::console::color::red("TRAP"),
        vector,
        ec,
        exception_class_name(ec),
        iss,
        esr,
        far,
        elr,
        spsr,
    );
    let mut fp: u64;
    unsafe { asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags)) };
    crate::kprint!("ifsy: TRAP: backtrace:");
    for _ in 0..12 {
        if fp < 0xffff_0000_0000_0000 || fp & 7 != 0 {
            break;
        }
        let lr = unsafe { core::ptr::read_volatile((fp + 8) as *const u64) };
        crate::kprint!(" {:#x}", lr);
        fp = unsafe { core::ptr::read_volatile(fp as *const u64) };
    }
    crate::kprintln!();
    crate::arch::halt();
}

global_asm!(
    r#"
.section .text
.global vector_table_el1
.balign 2048
vector_table_el1:
.balign 0x80
    mov x0, #0
    bl aarch64_trap_common
.balign 0x80
    mov x0, #1
    bl aarch64_trap_common
.balign 0x80
    mov x0, #2
    bl aarch64_trap_common
.balign 0x80
    mov x0, #3
    bl aarch64_trap_common
.balign 0x80
    mov x0, #4
    bl aarch64_trap_common
.balign 0x80
    b aarch64_irq_entry
.balign 0x80
    mov x0, #6
    bl aarch64_trap_common
.balign 0x80
    mov x0, #7
    bl aarch64_trap_common
.balign 0x80
    b aarch64_user_sync_entry
.balign 0x80
    b aarch64_user_irq_entry
.balign 0x80
    mov x0, #10
    bl aarch64_trap_common
.balign 0x80
    mov x0, #11
    bl aarch64_trap_common
.balign 0x80
    mov x0, #12
    bl aarch64_trap_common
.balign 0x80
    mov x0, #13
    bl aarch64_trap_common
.balign 0x80
    mov x0, #14
    bl aarch64_trap_common
.balign 0x80
    mov x0, #15
    bl aarch64_trap_common
"#
);

pub unsafe fn init() {
    let table_addr = core::ptr::addr_of!(vector_table_el1) as u64;
    unsafe {
        asm!("msr vbar_el1, {}", in(reg) table_addr, options(nostack, preserves_flags));
        asm!("isb", options(nostack, preserves_flags));
    }
    crate::kprintln!("ifsy: hardening: aarch64: exception vector table installed (VBAR_EL1) {}", crate::console::color::pass());
}
