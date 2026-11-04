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
use core::arch::x86_64::_rdtsc;

#[inline]
pub fn counter() -> u64 {
    // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    unsafe { _rdtsc() }
}

/// # Safety
/// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
unsafe fn outb(port: u16, v: u8) {
    // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    unsafe { asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags)) };
}

/// # Safety
/// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    unsafe { asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack, preserves_flags)) };
    v
}

const PIT_HZ: u64 = 1_193_182;
const PIT_COUNT: u16 = 11_932;

fn calibrate() -> Option<u64> {
    // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    unsafe {
        let saved = inb(0x61);
        outb(0x61, (saved & !0x02) | 0x01);
        outb(0x43, 0xb0);
        outb(0x42, PIT_COUNT as u8);
        outb(0x42, (PIT_COUNT >> 8) as u8);
        let g = inb(0x61);
        outb(0x61, g & !0x01);
        outb(0x61, g | 0x01);

        let t0 = counter();
        let mut spins = 0u32;
        while inb(0x61) & 0x20 == 0 {
            spins += 1;
            if spins > 20_000_000 {
                outb(0x61, saved);
                return None;
            }
        }
        let t1 = counter();
        outb(0x61, saved);
        Some(t1.wrapping_sub(t0) * PIT_HZ / PIT_COUNT as u64)
    }
}

pub fn frequency() -> (u64, bool) {
    let mut samples = [0u64; 5];
    let mut n = 0;
    for _ in 0..5 {
        if let Some(f) = calibrate() {
            if (100_000_000..=10_000_000_000).contains(&f) {
                samples[n] = f;
                n += 1;
            }
        }
    }
    if n == 0 {
        return (1_000_000_000, false);
    }
    let s = &mut samples[..n];
    s.sort_unstable();
    (s[n / 2], true)
}
