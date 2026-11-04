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
use core::fmt;

pub const MMIO_BASE: usize = 0xffff_ff00_0000_0000;
const MMIO_SLOT: usize = 510;

const UART0_BASE: usize = MMIO_BASE + 0x0900_0000;
const UARTDR: usize = UART0_BASE;
const UARTFR: usize = UART0_BASE + 0x18;
const UARTFR_TXFF: u32 = 1 << 5;
const UARTFR_RXFE: u32 = 1 << 4;

#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut MMIO_L1: PageTable = PageTable([0; 512]);
static mut EMPTY_L0: PageTable = PageTable([0; 512]);

const DESC_BLOCK: u64 = 0b01;
const DESC_TABLE: u64 = 0b11;
const DESC_AF: u64 = 1 << 10;
const DESC_PXN: u64 = 1 << 53;
const DESC_UXN: u64 = 1 << 54;
const ATTR_DEVICE_IDX: u64 = 2;
const PA_MASK: u64 = 0x0000_ffff_ffff_f000;

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn virt_to_phys(va: u64) -> u64 {
    let par: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "at s1e1r, {va}",
            "isb",
            "mrs {par}, par_el1",
            va = in(reg) va,
            par = out(reg) par,
            options(nostack, preserves_flags),
        );
    }
    (par & PA_MASK) | (va & 0xfff)
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn map_device_mmio() {
    let Some(hhdm) = crate::HHDM_REQUEST.get_response().map(|r| r.offset()) else {
        super::halt();
    };
    // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
    unsafe {
        let l1 = (&raw mut MMIO_L1) as *mut u64;
        let empty = (&raw mut EMPTY_L0) as *mut u64;
        let l1_pa = virt_to_phys(l1 as u64);
        let empty_pa = virt_to_phys(empty as u64);

        let ttbr1: u64;
        asm!("mrs {}, ttbr1_el1", out(reg) ttbr1, options(nomem, nostack, preserves_flags));
        let root = (hhdm + (ttbr1 & PA_MASK)) as *mut u64;
        if core::ptr::read_volatile(root.add(MMIO_SLOT)) != 0 {
            super::halt();
        }

        core::ptr::write_volatile(
            l1,
            DESC_BLOCK | (ATTR_DEVICE_IDX << 2) | DESC_AF | DESC_PXN | DESC_UXN,
        );
        asm!("dsb ishst", options(nostack));
        core::ptr::write_volatile(root.add(MMIO_SLOT), (l1_pa & PA_MASK) | DESC_TABLE);

        let mut mair: u64;
        asm!("mrs {}, mair_el1", out(reg) mair, options(nomem, nostack, preserves_flags));
        mair &= !(0xffu64 << (8 * ATTR_DEVICE_IDX));
        asm!("msr mair_el1, {}", in(reg) mair, options(nomem, nostack, preserves_flags));

        let mut tcr: u64;
        asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack, preserves_flags));
        let attrs = (tcr >> 24) & 0x3f;
        tcr &= !((1u64 << 7) | (0x3f << 0) | (3 << 14) | (0x3f << 8) | (1 << 22));
        tcr |= 16 | (attrs << 8);
        asm!("msr tcr_el1, {}", in(reg) tcr, options(nomem, nostack, preserves_flags));

        asm!("dsb ishst", options(nostack));
        asm!("msr ttbr0_el1, {}", in(reg) empty_pa & PA_MASK, options(nomem, nostack, preserves_flags));
        asm!("dsb ish", "isb", "tlbi vmalle1", "dsb ish", "isb", options(nostack));
    }
}

pub struct SerialPort;

impl SerialPort {
    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    pub unsafe fn init() -> Self {
        unsafe { map_device_mmio() };
        Self
    }

    fn flags(&self) -> u32 {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe { core::ptr::read_volatile(UARTFR as *const u32) }
    }

    pub fn write_byte(&mut self, byte: u8) {
        while self.flags() & UARTFR_TXFF != 0 {}
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe { core::ptr::write_volatile(UARTDR as *mut u32, byte as u32) };
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        if self.flags() & UARTFR_RXFE != 0 {
            return None;
        }
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        Some(unsafe { core::ptr::read_volatile(UARTDR as *const u32) } as u8)
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}
