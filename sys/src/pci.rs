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

#[cfg(target_arch = "x86_64")]
mod pio {
    use core::arch::asm;

    /// # Safety
    /// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    pub unsafe fn outb(port: u16, v: u8) {
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
    }

    /// # Safety
    /// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    pub unsafe fn outw(port: u16, v: u16) {
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe { asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags)) };
    }

    /// # Safety
    /// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    pub unsafe fn outl(port: u16, v: u32) {
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe { asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem, nostack, preserves_flags)) };
    }

    /// # Safety
    /// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    pub unsafe fn inb(port: u16) -> u8 {
        let v: u8;
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe { asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags)) };
        v
    }

    /// # Safety
    /// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    pub unsafe fn inw(port: u16) -> u16 {
        let v: u16;
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe { asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack, preserves_flags)) };
        v
    }

    /// # Safety
    /// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    pub unsafe fn inl(port: u16) -> u32 {
        let v: u32;
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe { asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack, preserves_flags)) };
        v
    }

    fn addr(b: u32, d: u32, f: u32, off: u32) -> u32 {
        0x8000_0000 | (b << 16) | (d << 11) | (f << 8) | (off & 0xfc)
    }

    pub fn cfg_read(b: u32, d: u32, f: u32, off: u32) -> u32 {
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe {
            outl(0xcf8, addr(b, d, f, off));
            inl(0xcfc)
        }
    }

    pub fn cfg_write(b: u32, d: u32, f: u32, off: u32, v: u32) {
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        unsafe {
            outl(0xcf8, addr(b, d, f, off));
            outl(0xcfc, v);
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub use pio::*;

#[cfg(not(target_arch = "x86_64"))]
mod ecam {
    use core::ptr::{read_volatile, write_volatile};
    use core::sync::atomic::{AtomicU64, Ordering};

    use crate::arch::port::{PCI_ECAM_BASE, PCI_ECAM_BUSES, mmio_map};

    static BASE: AtomicU64 = AtomicU64::new(0);

    fn base() -> u64 {
        let b = BASE.load(Ordering::Relaxed);
        if b != 0 {
            return b;
        }
        let b = mmio_map(PCI_ECAM_BASE, (PCI_ECAM_BUSES as u64) << 20).unwrap_or(0);
        BASE.store(b, Ordering::Relaxed);
        b
    }

    fn addr(b: u32, d: u32, f: u32, off: u32) -> Option<u64> {
        let base = base();
        if base == 0 || b >= PCI_ECAM_BUSES {
            return None;
        }
        Some(base + ((b as u64) << 20 | (d as u64) << 15 | (f as u64) << 12 | (off & 0xffc) as u64))
    }

    pub fn cfg_read(b: u32, d: u32, f: u32, off: u32) -> u32 {
        match addr(b, d, f, off) {
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            Some(a) => unsafe { read_volatile(a as *const u32) },
            None => 0xffff_ffff,
        }
    }

    pub fn cfg_write(b: u32, d: u32, f: u32, off: u32, v: u32) {
        if let Some(a) = addr(b, d, f, off) {
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            unsafe { write_volatile(a as *mut u32, v) };
        }
    }

    pub const BUSES: u32 = PCI_ECAM_BUSES;
}

#[cfg(not(target_arch = "x86_64"))]
pub use ecam::{cfg_read, cfg_write};

#[cfg(target_arch = "x86_64")]
const BUSES: u32 = 256;
#[cfg(not(target_arch = "x86_64"))]
const BUSES: u32 = ecam::BUSES;

pub fn find(mut hit: impl FnMut(u32, u32, u32) -> bool) -> Option<(u32, u32, u32)> {
    for b in 0..BUSES {
        for d in 0..32u32 {
            for f in 0..8u32 {
                if cfg_read(b, d, f, 0) & 0xffff == 0xffff {
                    if f == 0 {
                        break;
                    }
                    continue;
                }
                if hit(b, d, f) {
                    return Some((b, d, f));
                }
                if f == 0 && (cfg_read(b, d, 0, 0x0c) >> 16) & 0x80 == 0 {
                    break;
                }
            }
        }
    }
    None
}

pub fn find_class(class: u32) -> Option<(u32, u32, u32)> {
    find(|b, d, f| cfg_read(b, d, f, 8) >> 8 == class)
}

pub fn find_id(vendor: u32, device: u32) -> Option<(u32, u32, u32)> {
    find(|b, d, f| cfg_read(b, d, f, 0) == (device << 16 | vendor))
}
