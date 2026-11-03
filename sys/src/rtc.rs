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

use core::sync::atomic::{AtomicU64, Ordering};

static BASE_EPOCH: AtomicU64 = AtomicU64::new(0);
static BASE_UP: AtomicU64 = AtomicU64::new(0);
static BASE_SET: AtomicU64 = AtomicU64::new(0);

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(target_arch = "x86_64")]
fn cmos(reg: u8) -> u8 {
    let v: u8;
    unsafe {
        core::arch::asm!("out dx, al", in("dx") 0x70u16, in("al") reg, options(nomem, nostack, preserves_flags));
        core::arch::asm!("in al, dx", in("dx") 0x71u16, out("al") v, options(nomem, nostack, preserves_flags));
    }
    v
}

#[cfg(target_arch = "x86_64")]
fn read_rtc() -> Option<u64> {
    let busy = || cmos(0x0a) & 0x80 != 0;
    let mut spins = 0u32;
    while busy() && spins < 2_000_000 {
        spins += 1;
    }
    let snap = || (cmos(0x00), cmos(0x02), cmos(0x04), cmos(0x07), cmos(0x08), cmos(0x09), cmos(0x32));
    let mut a = snap();
    for _ in 0..4 {
        let b = snap();
        if a == b {
            break;
        }
        a = b;
    }
    let status_b = cmos(0x0b);
    let bin = status_b & 0x04 != 0;
    let h24 = status_b & 0x02 != 0;
    let dec = |v: u8| -> i64 { if bin { v as i64 } else { ((v >> 4) as i64) * 10 + (v & 0x0f) as i64 } };
    let (s, mi, h_raw, d, mo, y, c) = a;
    let pm = h_raw & 0x80 != 0;
    let mut h = dec(h_raw & 0x7f);
    if !h24 {
        h %= 12;
        if pm {
            h += 12;
        }
    }
    let century = dec(c);
    let year = if (19..=29).contains(&century) { century * 100 + dec(y) } else { 2000 + dec(y) };
    let (mo, d) = (dec(mo), dec(d));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || year < 1970 {
        return None;
    }
    let days = days_from_civil(year, mo, d);
    Some((days * 86_400 + h * 3600 + dec(mi) * 60 + dec(s)) as u64)
}

#[cfg(not(target_arch = "x86_64"))]
fn read_rtc() -> Option<u64> {
    None
}

pub fn wall_secs() -> u64 {
    let up = crate::timer::uptime_ms() / 1000;
    if BASE_SET.load(Ordering::Acquire) == 0 {
        BASE_EPOCH.store(read_rtc().unwrap_or(0), Ordering::Relaxed);
        BASE_UP.store(up, Ordering::Relaxed);
        BASE_SET.store(1, Ordering::Release);
    }
    BASE_EPOCH.load(Ordering::Relaxed) + up.saturating_sub(BASE_UP.load(Ordering::Relaxed))
}
