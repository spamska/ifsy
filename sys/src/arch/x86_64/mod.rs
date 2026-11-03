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

pub mod elf;
pub mod mm;
pub mod mmu;
pub mod timer;
pub mod uctx;
pub mod userland;
pub mod usermode;

use core::arch::asm;

pub mod gdt;
pub mod idt;
pub mod pic;
pub mod protect;
pub mod ps2;
pub mod serial;
pub mod trap;
pub mod user;

pub use user::Port;

pub unsafe fn init_protection() {
    unsafe { protect::init() };
}

pub unsafe fn init_traps() {
    unsafe { gdt::init() };
    unsafe { idt::init() };
    if !unsafe { <Port as crate::arch::Hal>::init_user_mode() } {
        crate::kprintln!("ifsy: user: x86_64: cannot enable user mode on this CPU; halting {}", crate::console::color::failed());
        halt();
    }
}

#[inline]
pub fn mask_interrupts() {
    unsafe { asm!("cli") };
}

#[inline]
pub fn irq_save() -> usize {
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", "cli", out(reg) flags) };
    flags as usize
}

#[inline]
pub fn irq_restore(saved: usize) {
    if saved & (1 << 9) != 0 {
        unsafe { asm!("sti") };
    }
}

#[inline]
pub fn irq_enable() {
    unsafe { asm!("sti") };
}

#[inline]
pub fn idle_wait() {
    unsafe { asm!("sti", "hlt", "cli") };
}

pub unsafe fn timer_irq_start(_first_deadline: u64, hz: u64) -> bool {
    unsafe { pic::start(hz) };
    true
}

pub fn mmio_map(phys: u64, bytes: u64) -> Option<u64> {
    let off = phys & 0xfff;
    crate::mm::aspace::kvm::map_phys(crate::mm::PhysAddr(phys & !0xfff), (bytes + off).div_ceil(4096), crate::mm::PageFlags::MMIO)
        .map(|v| v + off)
}

static TIMER_FAST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn timer_fast(on: bool) {
    if TIMER_FAST.swap(on, core::sync::atomic::Ordering::Relaxed) != on {
        let hz = if on { 2000 } else { crate::sched::HZ };
        unsafe { pic::set_rate(hz) };
    }
}

pub fn timer_irq_diag() {
    pic::diagnose();
}

pub unsafe fn timer_irq_stop() {
    unsafe { pic::stop() };
}

pub fn halt() -> ! {
    mask_interrupts();
    loop {
        unsafe { asm!("hlt") };
    }
}

#[inline]
unsafe fn outw(port: u16, val: u16) {
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags)) };
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)) };
}

pub fn power_off() -> ! {
    mask_interrupts();
    unsafe {
        outw(0x604, 0x2000);
        outw(0xB004, 0x2000);
        outw(0x4004, 0x3400);
    }
    halt()
}

pub fn reset() -> ! {
    mask_interrupts();
    unsafe {
        outb(0x64, 0xFE);
        outb(0xCF9, 0x0E);
    }
    halt()
}
