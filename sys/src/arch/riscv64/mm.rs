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

use crate::mm::{PageFlags, PhysAddr};

const VALID: u64 = 1 << 0;
const READ: u64 = 1 << 1;
const WRITE: u64 = 1 << 2;
const EXEC: u64 = 1 << 3;
const USER: u64 = 1 << 4;
const GLOBAL: u64 = 1 << 5;
const ACCESSED: u64 = 1 << 6;
const DIRTY: u64 = 1 << 7;
const PPN_SHIFT: u32 = 10;
const PTE_PPN_FIELD_MASK: u64 = 0x003f_ffff_ffff_fc00;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const fn unused() -> Self {
        Self(0)
    }

    pub const fn is_present(&self) -> bool {
        self.0 & VALID != 0
    }

    pub fn set(&mut self, addr: PhysAddr, flags: PageFlags) {
        let ppn_field = ((addr.0 >> 12) << PPN_SHIFT) & PTE_PPN_FIELD_MASK;
        let mut bits = VALID | READ | ACCESSED | DIRTY | ppn_field;
        if flags.writable() {
            bits |= WRITE;
        }
        if flags.executable() {
            bits |= EXEC;
        }
        if flags.user_accessible() {
            bits |= USER;
        }
        if flags.global() {
            bits |= GLOBAL;
        }
        self.0 = bits;
    }

    pub const fn addr(&self) -> PhysAddr {
        PhysAddr(((self.0 & PTE_PPN_FIELD_MASK) >> PPN_SHIFT) << 12)
    }

    pub const fn table(addr: PhysAddr) -> Self {
        Self(VALID | (((addr.0 >> 12) << PPN_SHIFT) & PTE_PPN_FIELD_MASK))
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
        level > 1 && self.0 & VALID != 0 && self.0 & (READ | WRITE | EXEC) != 0
    }

    pub fn flags(&self) -> Option<PageFlags> {
        if !self.is_present() || self.0 & (READ | WRITE | EXEC) == 0 {
            return None;
        }
        let writable = self.0 & WRITE != 0;
        let executable = self.0 & EXEC != 0;
        if writable && executable {
            return None;
        }
        Some(PageFlags::new(writable, executable, self.0 & USER != 0, self.0 & GLOBAL != 0))
    }

    pub fn make_non_exec(&mut self) {
        self.0 &= !EXEC;
    }
}
