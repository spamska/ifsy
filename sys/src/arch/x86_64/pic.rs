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

pub const VECTOR_TIMER: u8 = 32;
pub const VECTOR_SPURIOUS_MASTER: u8 = 39;
pub const VECTOR_SPURIOUS_SLAVE: u8 = 47;

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;
const PIT_CH0: u16 = 0x40;
const PIT_CMD: u16 = 0x43;
const PIT_HZ: u64 = 1_193_182;

const EOI: u8 = 0x20;

const MSR_APIC_BASE: u32 = 0x1b;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_X2APIC: u64 = 1 << 10;

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags)) };
    v
}

unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe { asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags)) };
    ((hi as u64) << 32) | lo as u64
}

unsafe fn wrmsr(msr: u32, v: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") v as u32, in("edx") (v >> 32) as u32,
             options(nomem, nostack, preserves_flags))
    };
}

fn has_apic() -> bool {
    unsafe { core::arch::x86_64::__cpuid(1).edx & (1 << 9) != 0 }
}

unsafe fn route_pic_to_cpu() {
    if !has_apic() {
        return;
    }
    let base = unsafe { rdmsr(MSR_APIC_BASE) };
    if base & APIC_BASE_ENABLE != 0 {
        unsafe { wrmsr(MSR_APIC_BASE, base & !(APIC_BASE_ENABLE | APIC_BASE_X2APIC)) };
    }
}

pub fn diagnose() {
    let (base, imr, irr, isr) = unsafe {
        let base = if has_apic() { rdmsr(MSR_APIC_BASE) } else { 0 };
        outb(PIC1_CMD, 0x0a);
        let irr = inb(PIC1_CMD);
        outb(PIC1_CMD, 0x0b);
        let isr = inb(PIC1_CMD);
        (base, inb(PIC1_DATA), irr, isr)
    };
    crate::kprintln!(
        "ifsy: timer: x86_64 diag: IA32_APIC_BASE={:#x} 8259 IMR={:#04x} IRR={:#04x} ISR={:#04x} \
         (IRR bit0 set = PIC raised IRQ0 but the CPU did not take it)",
        base,
        imr,
        irr,
        isr
    );
}

unsafe fn outb(port: u16, v: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
}

unsafe fn io_wait() {
    unsafe { outb(0x80, 0) };
}

unsafe fn remap_and_mask() {
    unsafe {
        outb(PIC1_CMD, 0x11);
        io_wait();
        outb(PIC2_CMD, 0x11);
        io_wait();
        outb(PIC1_DATA, VECTOR_TIMER);
        io_wait();
        outb(PIC2_DATA, VECTOR_TIMER + 8);
        io_wait();
        outb(PIC1_DATA, 4);
        io_wait();
        outb(PIC2_DATA, 2);
        io_wait();
        outb(PIC1_DATA, 0x01);
        io_wait();
        outb(PIC2_DATA, 0x01);
        io_wait();
        outb(PIC1_DATA, 0xff);
        outb(PIC2_DATA, 0xff);
    }
}

pub unsafe fn start(hz: u64) {
    let div = PIT_HZ.div_ceil(hz.max(19)).clamp(1, 0xffff) as u16;
    unsafe {
        route_pic_to_cpu();
        remap_and_mask();
        outb(PIT_CMD, 0x34);
        outb(PIT_CH0, div as u8);
        outb(PIT_CH0, (div >> 8) as u8);
        outb(PIC1_DATA, !0x01);
    }
}

pub unsafe fn set_rate(hz: u64) {
    let div = PIT_HZ.div_ceil(hz.max(19)).clamp(1, 0xffff) as u16;
    unsafe {
        outb(PIT_CMD, 0x34);
        outb(PIT_CH0, div as u8);
        outb(PIT_CH0, (div >> 8) as u8);
    }
}

pub unsafe fn stop() {
    unsafe {
        outb(PIC1_DATA, 0xff);
        outb(PIC2_DATA, 0xff);
    }
}

pub fn eoi_master() {
    unsafe { outb(PIC1_CMD, EOI) };
}
