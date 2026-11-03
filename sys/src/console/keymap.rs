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

const LOWER: &[u8; 58] = b"\0\x1b1234567890-=\x08\tqwertyuiop[]\r\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
const UPPER: &[u8; 58] = b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\r\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";

const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_RIGHTSHIFT: u16 = 54;
const KEY_CAPSLOCK: u16 = 58;
const KEY_RIGHTCTRL: u16 = 97;

pub struct Keymap {
    shift: u8,
    ctrl: u8,
    caps: bool,
}

impl Keymap {
    pub const fn new() -> Self {
        Self { shift: 0, ctrl: 0, caps: false }
    }

    pub fn key(&mut self, code: u16, value: u32) -> Option<u8> {
        let down = value != 0;
        match code {
            KEY_LEFTSHIFT => self.set(0, down, true),
            KEY_RIGHTSHIFT => self.set(1, down, true),
            KEY_LEFTCTRL => self.set(0, down, false),
            KEY_RIGHTCTRL => self.set(1, down, false),
            KEY_CAPSLOCK => {
                if value == 1 {
                    self.caps = !self.caps;
                }
            }
            c if down && (c as usize) < LOWER.len() => {
                let mut ch = if self.shift != 0 { UPPER[c as usize] } else { LOWER[c as usize] };
                if ch == 0 {
                    return None;
                }
                if self.caps && ch.is_ascii_alphabetic() {
                    ch ^= 0x20;
                }
                if self.ctrl != 0 && ch.is_ascii_alphabetic() {
                    ch &= 0x1f;
                }
                return Some(ch);
            }
            _ => {}
        }
        None
    }

    fn set(&mut self, bit: u8, down: bool, shift: bool) {
        let field = if shift { &mut self.shift } else { &mut self.ctrl };
        if down {
            *field |= 1 << bit;
        } else {
            *field &= !(1 << bit);
        }
    }
}
