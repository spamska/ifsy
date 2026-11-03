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

use core::mem::size_of;

pub const NGPR: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ureg {
    pub pc: u64,
    pub sp: u64,
    pub flags: u64,
    pub gpr: [u64; NGPR],
}

pub const UREG_SIZE: usize = size_of::<Ureg>();
use crate::arch::port::uctx as map;
pub use map::{FLAGS_INIT, FLAGS_USER_MASK, RED_ZONE};

pub const CALL_PUSHES_RETURN: bool = map::LINK.is_none();

impl Ureg {
    pub const fn zeroed() -> Self {
        Self { pc: 0, sp: 0, flags: 0, gpr: [0; NGPR] }
    }

    pub fn entry(pc: u64, sp: u64, arg0: u64, arg1: u64) -> Self {
        let mut u = Self::zeroed();
        u.pc = pc;
        u.sp = sp;
        u.flags = FLAGS_INIT;
        u.gpr[map::ARG0] = arg0;
        u.gpr[map::ARG1] = arg1;
        u
    }

    pub fn ret(&self) -> u64 {
        self.gpr[map::RET]
    }
    pub fn set_ret(&mut self, v: i64) {
        self.gpr[map::RET] = v as u64;
    }
    pub fn arg0(&self) -> u64 {
        self.gpr[map::ARG0]
    }
    pub fn arg1(&self) -> u64 {
        self.gpr[map::ARG1]
    }
    pub fn set_args(&mut self, a0: u64, a1: u64) {
        self.gpr[map::ARG0] = a0;
        self.gpr[map::ARG1] = a1;
    }
    pub fn clear_link(&mut self) {
        if let Some(l) = map::LINK {
            self.gpr[l] = 0;
        }
    }

    pub fn sanitized(mut self, kernel_flags: u64) -> Self {
        self.flags = (self.flags & FLAGS_USER_MASK) | (kernel_flags & !FLAGS_USER_MASK);
        self
    }

    pub fn to_bytes(&self) -> [u8; UREG_SIZE] {
        let mut out = [0u8; UREG_SIZE];
        out[0..8].copy_from_slice(&self.pc.to_le_bytes());
        out[8..16].copy_from_slice(&self.sp.to_le_bytes());
        out[16..24].copy_from_slice(&self.flags.to_le_bytes());
        for (i, g) in self.gpr.iter().enumerate() {
            out[24 + 8 * i..32 + 8 * i].copy_from_slice(&g.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < UREG_SIZE {
            return None;
        }
        let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        let mut u = Self::zeroed();
        u.pc = w(0);
        u.sp = w(1);
        u.flags = w(2);
        for i in 0..NGPR {
            u.gpr[i] = w(3 + i);
        }
        Some(u)
    }
}
