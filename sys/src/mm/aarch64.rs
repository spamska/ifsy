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

use super::{PageFlags, PhysAddr};

const VALID_PAGE: u64 = 0b11;
const AP_RDONLY: u64 = 1 << 7;
const AP_EL0: u64 = 1 << 6;
const SH_INNER: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
const NOT_GLOBAL: u64 = 1 << 11;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const fn unused() -> Self {
        Self(0)
    }

    pub const fn is_present(&self) -> bool {
        self.0 & 1 == 1
    }

    pub fn set(&mut self, addr: PhysAddr, flags: PageFlags) {
        let attr_index: u64 = 0 << 2;

        let mut bits = VALID_PAGE | AF | SH_INNER | attr_index | (addr.0 & ADDR_MASK);
        if !flags.writable() {
            bits |= AP_RDONLY;
        }
        if flags.user_accessible() {
            bits |= AP_EL0;
        }
        if !flags.global() {
            bits |= NOT_GLOBAL;
        }
        if !flags.executable() {
            bits |= PXN | UXN;
        } else if flags.user_accessible() {
            bits |= PXN;
        } else {
            bits |= UXN;
        }
        self.0 = bits;
    }

    pub const fn addr(&self) -> PhysAddr {
        PhysAddr(self.0 & ADDR_MASK)
    }

    pub const fn table(addr: PhysAddr) -> Self {
        Self(0b11 | (addr.0 & ADDR_MASK))
    }

    pub const fn raw(&self) -> u64 {
        self.0
    }

    pub const fn from_raw(v: u64) -> Self {
        Self(v)
    }

    pub fn clear(&mut self) {
        self.0 = 0;
    }

    pub const fn is_huge(&self, level: usize) -> bool {
        level > 1 && self.0 & 0b11 == 0b01
    }

    pub fn flags(&self) -> Option<PageFlags> {
        if !self.is_present() {
            return None;
        }
        let writable = self.0 & AP_RDONLY == 0;
        let user = self.0 & AP_EL0 != 0;
        let executable = self.0 & PXN == 0 || (user && self.0 & UXN == 0);
        if writable && executable {
            return None;
        }
        Some(PageFlags::new(writable, executable, user, self.0 & NOT_GLOBAL == 0))
    }

    pub fn make_non_exec(&mut self) {
        self.0 |= PXN | UXN;
    }
}
