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

use core::arch::{asm, global_asm};

pub mod irq;
pub mod protect;
pub mod serial;
pub mod traps;
pub mod user;

pub use user::Port;

global_asm!(
    r#"
.section .text._start, "ax"
.global _start
_start:
    mov x1, sp
    msr spsel, #1
    mov sp, x1
    mrs x0, cpacr_el1
    orr x0, x0, #(3 << 20)
    msr cpacr_el1, x0
    isb
    b kmain
"#
);

pub unsafe fn init_protection() {
    unsafe { protect::init() };
}

pub unsafe fn init_traps() {
    unsafe { traps::init() };
    unsafe { <Port as crate::arch::Hal>::init_user_mode() };
}

#[inline]
pub fn mask_interrupts() {
    unsafe { asm!("msr daifset, #0xf") };
}

#[inline]
pub fn irq_save() -> usize {
    let daif: u64;
    unsafe { asm!("mrs {}, daif", "msr daifset, #2", out(reg) daif, options(nostack)) };
    daif as usize
}

#[inline]
pub fn irq_restore(saved: usize) {
    unsafe { asm!("msr daif, {}", in(reg) saved as u64, options(nostack)) };
}

#[inline]
pub fn irq_enable() {
    unsafe { asm!("msr daifclr, #2", options(nostack)) };
}

#[inline]
pub fn idle_wait() {
    unsafe { asm!("msr daifclr, #2", "isb", "wfi", "msr daifset, #2", options(nostack)) };
}

pub unsafe fn timer_irq_start(first_deadline: u64, _hz: u64) -> bool {
    unsafe { irq::start(first_deadline) }
}

pub fn timer_irq_diag() {}

pub fn timer_fast(_on: bool) {}

pub unsafe fn timer_irq_stop() {
    unsafe { irq::stop() };
}

pub fn halt() -> ! {
    mask_interrupts();
    loop {
        unsafe { asm!("wfi") };
    }
}

fn psci(id: u64) {
    for conduit in 0..2 {
        let mut ret = id;
        unsafe {
            if conduit == 0 {
                asm!("hvc #0", inout("x0") ret, in("x1") 0u64, in("x2") 0u64, in("x3") 0u64, clobber_abi("C"), options(nostack));
            } else {
                asm!("smc #0", inout("x0") ret, in("x1") 0u64, in("x2") 0u64, in("x3") 0u64, clobber_abi("C"), options(nostack));
            }
        }
        let _ = ret;
    }
}

pub fn power_off() -> ! {
    mask_interrupts();
    psci(0x8400_0008);
    halt()
}

pub fn reset() -> ! {
    mask_interrupts();
    psci(0x8400_0009);
    halt()
}

pub const PCI_ECAM_BASE: u64 = 0x40_1000_0000;
pub const PCI_ECAM_BUSES: u32 = 8;
pub const VIRTIO_MMIO_BASE: u64 = 0x0a00_0000;
pub const VIRTIO_MMIO_STRIDE: u64 = 0x200;
pub const VIRTIO_MMIO_SLOTS: u64 = 32;

const MMIO_WINDOW: u64 = 1 << 30;

pub fn mmio_map(phys: u64, bytes: u64) -> Option<u64> {
    if phys.checked_add(bytes)? <= MMIO_WINDOW {
        return Some(serial::MMIO_BASE as u64 + phys);
    }
    let off = phys & 0xfff;
    crate::mm::aspace::kvm::map_phys(crate::mm::PhysAddr(phys & !0xfff), (bytes + off).div_ceil(4096), crate::mm::PageFlags::MMIO)
        .map(|v| v + off)
}
