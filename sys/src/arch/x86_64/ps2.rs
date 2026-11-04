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

use core::sync::atomic::{AtomicBool, Ordering};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;

#[inline]
/// # Safety
/// x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    unsafe { core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack, preserves_flags)) };
    val
}

const LOWER: &[u8; 58] = b"\0\x1b1234567890-=\x08\tqwertyuiop[]\r\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
const UPPER: &[u8; 58] = b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\r\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

static SHIFT: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
static CAPS: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);

pub fn read_byte() -> Option<u8> {
    for _ in 0..16 {
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        let status = unsafe { inb(STATUS) };
        if status == 0xFF || status & 0x01 == 0 {
            return None;
        }
        // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
        let code = unsafe { inb(DATA) };
        if status & 0x20 != 0 {
            continue;
        }
        if code == 0xE0 || code == 0xE1 {
            EXTENDED.store(true, Ordering::Relaxed);
            continue;
        }
        if EXTENDED.swap(false, Ordering::Relaxed) {
            continue;
        }
        let release = code & 0x80 != 0;
        let key = code & 0x7f;
        match key {
            0x2a | 0x36 => SHIFT.store(!release, Ordering::Relaxed),
            0x1d => CTRL.store(!release, Ordering::Relaxed),
            0x3a if !release => {
                CAPS.fetch_xor(true, Ordering::Relaxed);
            }
            _ if release => {}
            k if (k as usize) < LOWER.len() => {
                let shift = SHIFT.load(Ordering::Relaxed);
                let mut c = if shift { UPPER[k as usize] } else { LOWER[k as usize] };
                if c == 0 {
                    continue;
                }
                if CAPS.load(Ordering::Relaxed) && c.is_ascii_alphabetic() {
                    c ^= 0x20;
                }
                if CTRL.load(Ordering::Relaxed) && c.is_ascii_alphabetic() {
                    c &= 0x1f;
                }
                return Some(c);
            }
            _ => {}
        }
    }
    None
}
