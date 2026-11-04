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

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;

pub const NNOTE: usize = 5;

pub const NCONT: u64 = 0;
pub const NDFLT: u64 = 1;
pub const NSAVE: u64 = 2;
pub const NRSTR: u64 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoteKind {
    User,
    Exit,
    Debug,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Note {
    pub msg: String,
    pub kind: NoteKind,
}

impl Note {
    pub fn alarm() -> Self {
        Self { msg: String::from("alarm"), kind: NoteKind::User }
    }
    pub fn user(msg: &str) -> Self {
        Self { msg: String::from(msg), kind: NoteKind::User }
    }
    pub fn kill() -> Self {
        Self { msg: String::from("sys: kill"), kind: NoteKind::Exit }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fault {
    Read(u64),
    Write(u64),
    Exec(u64),
    Misaligned(u64),
    DivideError,
    InvalidOpcode,
    Other(&'static str),
}

impl Fault {
    pub fn text(&self) -> String {
        match self {
            Fault::Read(a) => format!("sys: trap: fault read addr={:#x}", a),
            Fault::Write(a) => format!("sys: trap: fault write addr={:#x}", a),
            Fault::Exec(a) => format!("sys: trap: fault instruction addr={:#x}", a),
            Fault::Misaligned(a) => format!("sys: trap: misaligned access addr={:#x}", a),
            Fault::DivideError => String::from("sys: trap: divide error"),
            Fault::InvalidOpcode => String::from("sys: trap: invalid opcode"),
            Fault::Other(s) => format!("sys: trap: {}", s),
        }
    }
}

pub struct NoteState {
    pub handler: u64,
    pub notified: bool,
    pub ureg_va: u64,
    pub saved_flags: u64,
    /// Frame stashed by noted(NSAVE) for a later noted(NRSTR); 0 when none.
    pub sv_ureg_va: u64,
    pub sv_flags: u64,
    pub last: Option<Note>,
    pub queue: VecDeque<Note>,
}

impl NoteState {
    pub const fn new() -> Self {
        Self { handler: 0, notified: false, ureg_va: 0, saved_flags: 0, sv_ureg_va: 0, sv_flags: 0, last: None, queue: VecDeque::new() }
    }

    pub fn reset_for_exec(&mut self) {
        self.handler = 0;
        self.notified = false;
        self.ureg_va = 0;
        self.last = None;
    }
}
