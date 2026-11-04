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

use core::fmt;

use super::trap::{VECTOR_SYSCALL, vectors};

#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

impl fmt::Display for InterruptStackFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rip={:#x} cs={:#x} flags={:#x} rsp={:#x} ss={:#x}",
            self.instruction_pointer, self.code_segment, self.cpu_flags, self.stack_pointer, self.stack_segment)
    }
}

pub(super) fn classify_page_fault(frame: &InterruptStackFrame, error_code: u64, addr: u64) -> &'static str {
    const P: u64 = 1 << 0;
    const U: u64 = 1 << 2;
    const RSVD: u64 = 1 << 3;
    const I: u64 = 1 << 4;
    let from_user = frame.code_segment & 3 == 3;
    if error_code & RSVD != 0 {
        return "reserved bit set in a page-table entry (corrupt tables)";
    }
    if from_user || error_code & U != 0 {
        return if error_code & P == 0 { "user access to an unmapped page" } else { "user access violates page permissions" };
    }
    let user_addr = addr < crate::mm::hw::USER_VA_TOP;
    match (error_code & P != 0, error_code & I != 0, user_addr) {
        (true, true, true) => "SMEP VIOLATION: kernel tried to execute a user page",
        (true, false, true) if frame.cpu_flags & super::protect::RFLAGS_AC == 0 => {
            "SMAP VIOLATION: kernel touched user memory outside stac/clac"
        }
        (true, false, true) => "kernel fault on user page with AC set (bad stac window)",
        (true, true, false) => "NX violation: kernel tried to execute a non-executable page",
        (true, false, false) => "kernel write/read protection fault (W^X, RELRO or guard page)",
        (false, _, true) => "kernel access to an unmapped user address (missing copy_from_user?)",
        (false, _, false) => "kernel access to an unmapped kernel address",
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        Self { offset_low: 0, selector: 0, ist: 0, type_attr: 0, offset_mid: 0, offset_high: 0, reserved: 0 }
    }

    fn new(handler: u64, code_selector: u16, dpl: u8) -> Self {
        Self::with_ist(handler, code_selector, dpl, 0)
    }

    fn with_ist(handler: u64, code_selector: u16, dpl: u8, ist: u8) -> Self {
        const GATE_TYPE_INTERRUPT: u8 = 0xE;
        let type_attr = 0x80 | ((dpl & 0b11) << 5) | GATE_TYPE_INTERRUPT;
        Self {
            offset_low: handler as u16,
            selector: code_selector,
            ist: ist & 0b111,
            type_attr,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
struct Idt([IdtEntry; 256]);

impl Idt {
    const fn new() -> Self {
        Self([IdtEntry::missing(); 256])
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

static mut IDT: Idt = Idt::new();

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn init() {
    let cs = super::gdt::KERNEL_CS;

    // SAFETY: the pointer is valid for the lifetime of its owner and access is exclusive (&mut self or the owning lock is held).
    let idt = unsafe { &mut *core::ptr::addr_of_mut!(IDT) };
    let mut gates = 0usize;
    for (vector, stub) in vectors() {
        let ist = match vector {
            2 => super::gdt::IST_NMI,
            8 => super::gdt::IST_DOUBLE_FAULT,
            18 => super::gdt::IST_MACHINE_CHECK,
            _ => 0,
        };
        let dpl = if vector == VECTOR_SYSCALL { 3 } else { 0 };
        idt.0[vector as usize] = IdtEntry::with_ist(stub, cs, dpl, ist);
        gates += 1;
    }

    let idtr = IdtPointer {
        limit: (core::mem::size_of::<Idt>() - 1) as u16,
        base: core::ptr::addr_of!(IDT) as u64,
    };
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { core::arch::asm!("lidt [{}]", in(reg) &idtr, options(readonly, nostack, preserves_flags)) };

    crate::kprintln!("ifsy: hardening: x86_64: IDT loaded ({} gates: exceptions with IST for #DF/NMI/#MC, PIC/PIT IRQs 32/39/47, syscall vector {}) {}", gates, VECTOR_SYSCALL, crate::console::color::pass());
}
