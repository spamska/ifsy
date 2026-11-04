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

mod font;
pub mod keymap;
mod text;

use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::arch::serial::SerialPort;
use crate::sync::SpinLock;
use text::TextConsole;

struct Console {
    serial: Option<SerialPort>,
    text: Option<TextConsole>,
    muted: bool,
    held: Vec<u8>,
    text_on: bool,
}

impl Console {
    const fn new() -> Self {
        Self { serial: None, text: None, muted: false, held: Vec::new(), text_on: true }
    }
}

const HELD_MAX: usize = 8192;

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

static VERBOSE: AtomicBool = AtomicBool::new(cfg!(feature = "verbose-boot"));

static FAILS: AtomicUsize = AtomicUsize::new(0);

pub fn failures() -> usize {
    FAILS.load(Ordering::Relaxed)
}

const SHOWN: &[&str] = &[
    "ifsy: boot: framebuffer",
    "ifsy: heap: self-test PASS",
    "ifsy: frames: self-test PASS",
    "ifsy: aspace: self-test PASS",
    "ifsy: timer: interrupt self-test",
    "ifsy: abi: sched interface",
    "ifsy: abi: exec:",
    "ifsy: fs: remount persistence",
    "ifsy: userland: installed",
    "ifsy: user: ring 3 self-test",
    "ifsy: boot: setup complete",
];

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

struct Probe {
    buf: [u8; 256],
    len: usize,
    esc: bool,
}

impl Write for Probe {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            if self.esc {
                self.esc = !b.is_ascii_alphabetic();
            } else if b == 0x1b {
                self.esc = true;
            } else if self.len < self.buf.len() {
                self.buf[self.len] = b;
                self.len += 1;
            }
        }
        Ok(())
    }
}

fn quiet_allows(args: fmt::Arguments) -> bool {
    let mut p = Probe { buf: [0; 256], len: 0, esc: false };
    let _ = p.write_fmt(args);
    let msg = &p.buf[..p.len];
    let failed = contains(msg, b"FAIL");
    if failed {
        FAILS.fetch_add(1, Ordering::Relaxed);
    }
    failed
        || contains(msg, b"PANIC")
        || contains(msg, b"FAULT")
        || contains(msg, b"TRAP")
        || SHOWN.iter().any(|pre| msg.starts_with(pre.as_bytes()))
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if let Some(serial) = &mut self.serial {
            let _ = serial.write_str(s);
        }
        if self.text_on {
            if let Some(text) = &mut self.text {
                let _ = text.write_str(s);
            }
        }
        Ok(())
    }
}

pub mod color {
    use core::fmt::{self, Display};

    const RESET: &str = "\x1b[0m";

    pub struct Paint<T>(&'static str, T);

    impl<T: Display> Display for Paint<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}{}{}", self.0, self.1, RESET)
        }
    }

    pub fn red<T: Display>(t: T) -> Paint<T> {
        Paint("\x1b[91m", t)
    }

    pub fn green<T: Display>(t: T) -> Paint<T> {
        Paint("\x1b[92m", t)
    }

    pub fn yellow<T: Display>(t: T) -> Paint<T> {
        Paint("\x1b[93m", t)
    }

    pub struct Tag(&'static str, &'static str);

    impl Display for Tag {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "[{}{}{}]", self.0, self.1, RESET)
        }
    }

    pub fn pass() -> Tag {
        Tag("\x1b[92m", "PASS")
    }

    pub fn failed() -> Tag {
        Tag("\x1b[91m", "FAILED")
    }

    pub fn warn() -> Tag {
        Tag("\x1b[93m", "WARN")
    }
}

static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new());

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn init_serial() {
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    let port = unsafe { SerialPort::init() };
    CONSOLE.with(|c| c.serial = Some(port));
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn attach_framebuffer(base: *mut u8, width: usize, height: usize, pitch: usize, bytes_per_pixel: usize) {
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    let text = unsafe { TextConsole::new(base, width, height, pitch, bytes_per_pixel) };
    CONSOLE.with(|c| c.text = Some(text));
}

pub fn read_byte() -> Option<u8> {
    let b = CONSOLE.with(|c| c.serial.as_mut().and_then(|s| s.read_byte()));
    #[cfg(target_arch = "x86_64")]
    if b.is_none() {
        return crate::arch::port::ps2::read_byte();
    }
    #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
    if b.is_none() {
        return crate::virtio_input::read_byte();
    }
    b
}

pub fn size() -> (usize, usize) {
    CONSOLE
        .with(|c| c.text.as_ref().map(|t| t.dims()))
        .filter(|&(w, h)| w > 0 && h > 0)
        .unwrap_or((80, 24))
}

const BLINK_TICKS: u64 = crate::sched::HZ / 2;
static BLINK_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn tick() {
    if BLINK_COUNT.fetch_add(1, Ordering::Relaxed) + 1 >= BLINK_TICKS {
        BLINK_COUNT.store(0, Ordering::Relaxed);
        CONSOLE.with(|c| {
            if let Some(t) = &mut c.text {
                t.blink();
            }
        });
    }
}

#[derive(Clone, Copy)]
pub struct DisplayInfo {
    pub cols: usize,
    pub rows: usize,
    pub width: usize,
    pub height: usize,
    pub cell_w: usize,
    pub cell_h: usize,
    pub default_cols: usize,
    pub default_rows: usize,
    pub max_cols: usize,
    pub max_rows: usize,
}

pub fn display_info() -> Option<DisplayInfo> {
    CONSOLE.with(|c| {
        let t = c.text.as_ref()?;
        let (cols, rows) = t.dims();
        let (width, height, cell_w, cell_h) = t.geometry();
        let (default_cols, default_rows) = t.default_grid();
        let (max_cols, max_rows) = t.max_grid();
        Some(DisplayInfo { cols, rows, width, height, cell_w, cell_h, default_cols, default_rows, max_cols, max_rows })
    })
}

pub fn set_grid(size: Option<(usize, usize)>) -> Result<(usize, usize), &'static str> {
    CONSOLE.with(|c| {
        let t = c.text.as_mut().ok_or("no framebuffer: this machine has only the serial console")?;
        let (cols, rows) = size.unwrap_or_else(|| t.default_grid());
        t.set_grid(cols, rows)?;
        Ok((cols, rows))
    })
}

pub fn enable_cursor() {
    CONSOLE.with(|c| {
        if let Some(text) = &mut c.text {
            text.set_cursor_enabled(true);
        }
    });
}

pub fn clear_screen() {
    CONSOLE.with(|c| {
        if let Some(text) = &mut c.text {
            text.clear();
        }
    });
}

pub fn set_muted(muted: bool) {
    CONSOLE.with(|c| c.muted = muted);
}

pub fn flush_held() {
    let held = CONSOLE.with(|c| core::mem::take(&mut c.held));
    write_bytes(&held);
}

pub fn write_bytes(bytes: &[u8]) {
    CONSOLE.with(|c| {
        if c.muted {
            c.held.extend_from_slice(bytes);
            if c.held.len() > HELD_MAX {
                let cut = c.held.len() - HELD_MAX;
                c.held.drain(..cut);
            }
            return;
        }
        if let Some(serial) = &mut c.serial {
            for &b in bytes {
                if b == b'\n' {
                    serial.write_byte(b'\r');
                }
                serial.write_byte(b);
            }
        }
        if let Some(text) = &mut c.text {
            let _ = text.write_str(&alloc::string::String::from_utf8_lossy(bytes));
        }
    });
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let on_screen = quiet_allows(args) || VERBOSE.load(Ordering::Relaxed);
    CONSOLE.with(|c| {
        c.text_on = on_screen;
        let _ = c.write_fmt(args);
        c.text_on = true;
    });
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprintln {
    () => {
        $crate::kprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}
