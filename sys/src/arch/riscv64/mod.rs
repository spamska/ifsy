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

pub mod irq;
pub mod protect;
pub mod serial;
pub mod traps;
pub mod user;

pub use user::Port;

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn init_protection() {
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe { protect::init() };
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn init_traps() {
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe { traps::init() };
    unsafe { enable_fpu() };
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn enable_fpu() {
    const FS_INITIAL: usize = 0x2000;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "csrs sstatus, {fs}",
            "csrw fcsr, zero",
            fs = in(reg) FS_INITIAL,
            options(nomem, nostack)
        );
    }
    crate::kprintln!("ifsy: hardening: riscv64: FPU enabled (sstatus.FS) {}", crate::console::color::pass());
}

#[inline]
pub fn mask_interrupts() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("csrci sstatus, 0x2") };
}

#[inline]
pub fn irq_save() -> usize {
    let old: usize;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("csrrci {}, sstatus, 0x2", out(reg) old, options(nostack)) };
    old
}

#[inline]
pub fn irq_restore(saved: usize) {
    if saved & 0x2 != 0 {
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("csrsi sstatus, 0x2", options(nostack)) };
    }
}

#[inline]
pub fn irq_enable() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("csrsi sstatus, 0x2", options(nostack)) };
}

#[inline]
pub fn idle_wait() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("csrsi sstatus, 0x2", "wfi", "csrci sstatus, 0x2", options(nostack)) };
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn timer_irq_start(first_deadline: u64, _hz: u64) -> bool {
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe { irq::start(first_deadline) }
}

pub fn timer_irq_diag() {}

pub fn timer_fast(_on: bool) {}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn timer_irq_stop() {
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe { irq::stop() };
}

pub fn halt() -> ! {
    mask_interrupts();
    loop {
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("wfi") };
    }
}

fn sbi_call2(eid: usize, fid: usize, arg0: usize, arg1: usize) -> (isize, usize) {
    let (err, val): (isize, usize);
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 as isize => err,
            inlateout("a1") arg1 => val,
            in("a6") fid,
            in("a7") eid,
            options(nostack),
        );
    }
    (err, val)
}

const SBI_EXT_SRST: usize = 0x5352_5354;
const SBI_EXT_LEGACY_SHUTDOWN: usize = 0x08;

pub fn power_off() -> ! {
    mask_interrupts();
    sbi_call2(SBI_EXT_SRST, 0, 0, 0);
    sbi_call2(SBI_EXT_LEGACY_SHUTDOWN, 0, 0, 0);
    halt()
}

pub fn reset() -> ! {
    mask_interrupts();
    sbi_call2(SBI_EXT_SRST, 0, 1, 0);
    halt()
}

pub const PCI_ECAM_BASE: u64 = 0x3000_0000;
pub const PCI_ECAM_BUSES: u32 = 16;
pub const VIRTIO_MMIO_BASE: u64 = 0x1000_1000;
pub const VIRTIO_MMIO_STRIDE: u64 = 0x1000;
pub const VIRTIO_MMIO_SLOTS: u64 = 8;

pub fn mmio_map(phys: u64, bytes: u64) -> Option<u64> {
    crate::mm::aspace::kvm::map_phys(crate::mm::PhysAddr(phys), bytes.div_ceil(4096), crate::mm::PageFlags::KERNEL_RW)
}
