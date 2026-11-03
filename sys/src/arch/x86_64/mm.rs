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

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const ACCESSED: u64 = 1 << 5;
const GLOBAL: u64 = 1 << 8;
const NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

const HUGE: u64 = 1 << 7;
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const fn unused() -> Self {
        Self(0)
    }

    pub const fn is_present(&self) -> bool {
        self.0 & PRESENT != 0
    }

    pub fn set(&mut self, addr: PhysAddr, flags: PageFlags) {
        let mut bits = PRESENT | ACCESSED | (addr.0 & ADDR_MASK);
        if flags.writable() {
            bits |= WRITABLE;
        }
        if flags.user_accessible() {
            bits |= USER;
        }
        if flags.global() {
            bits |= GLOBAL;
        }
        if !flags.executable() {
            bits |= NO_EXECUTE;
        }
        self.0 = bits;
    }

    pub const fn addr(&self) -> PhysAddr {
        PhysAddr(self.0 & ADDR_MASK)
    }

    pub const fn table(addr: PhysAddr) -> Self {
        Self(PRESENT | WRITABLE | USER | (addr.0 & ADDR_MASK))
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
        level > 1 && self.0 & HUGE != 0
    }

    pub fn flags(&self) -> Option<PageFlags> {
        if !self.is_present() {
            return None;
        }
        let writable = self.0 & WRITABLE != 0;
        let executable = self.0 & NO_EXECUTE == 0;
        if writable && executable {
            return None;
        }
        Some(PageFlags::new(writable, executable, self.0 & USER != 0, self.0 & GLOBAL != 0))
    }

    pub fn make_non_exec(&mut self) {
        self.0 |= NO_EXECUTE;
    }
}
