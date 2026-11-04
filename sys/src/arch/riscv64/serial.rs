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
use core::sync::atomic::{AtomicBool, Ordering};

const SBI_EXT_DBCN: usize = 0x4442_434E;
const SBI_DBCN_WRITE_BYTE: usize = 2;
const SBI_EXT_LEGACY_PUTCHAR: usize = 0x01;
const SBI_EXT_LEGACY_GETCHAR: usize = 0x02;

static USE_LEGACY: AtomicBool = AtomicBool::new(false);

#[inline]
fn sbi_call(eid: usize, fid: usize, arg0: usize) -> isize {
    let mut a0 = arg0 as isize;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "ecall",
            inout("a0") a0,
            lateout("a1") _,
            in("a6") fid,
            in("a7") eid,
            options(nostack),
        );
    }
    a0
}

pub struct SerialPort;

impl SerialPort {
    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    pub unsafe fn init() -> Self {
        Self
    }

    pub fn write_byte(&mut self, byte: u8) {
        if !USE_LEGACY.load(Ordering::Relaxed) {
            if sbi_call(SBI_EXT_DBCN, SBI_DBCN_WRITE_BYTE, byte as usize) == 0 {
                return;
            }
            USE_LEGACY.store(true, Ordering::Relaxed);
        }
        sbi_call(SBI_EXT_LEGACY_PUTCHAR, 0, byte as usize);
    }

    pub fn read_byte(&mut self) -> Option<u8> {
        let r = sbi_call(SBI_EXT_LEGACY_GETCHAR, 0, 0);
        (0..=255).contains(&r).then_some(r as u8)
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
