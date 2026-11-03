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

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use crate::console::keymap::Keymap;
use crate::arch::port::{VIRTIO_MMIO_BASE as MMIO_BASE, VIRTIO_MMIO_SLOTS as MMIO_SLOTS, VIRTIO_MMIO_STRIDE as MMIO_STRIDE, mmio_map};
use crate::mm;

const PAGE_SIZE: u64 = 4096;
use crate::sync::SpinLock;

const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const GUEST_PAGE_SIZE: usize = 0x028;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_ALIGN: usize = 0x03c;
const QUEUE_PFN: usize = 0x040;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;

const MAGIC_VALUE: u32 = 0x7472_6976;
const ID_INPUT: u32 = 18;

const ST_ACK: u32 = 1;
const ST_DRIVER: u32 = 2;
const ST_DRIVER_OK: u32 = 4;
const ST_FEATURES_OK: u32 = 8;

const EV_KEY: u16 = 1;

const QSIZE: usize = 16;
const DESC_WRITE: u16 = 2;
const EVENT_SIZE: usize = 8;

const OFF_DESC: usize = 0;
const OFF_AVAIL: usize = 256;
const OFF_USED: usize = 4096;
const OFF_EVENTS: usize = 2 * 4096;

struct Kbd {
    regs: u64,
    mem: u64,
    mem_phys: u64,
    last_used: u16,
    keys: Keymap,
}

enum State {
    Untried,
    Absent,
    Ready(Kbd),
}

static STATE: SpinLock<State> = SpinLock::new(State::Untried);

unsafe fn rd(regs: u64, off: usize) -> u32 {
    unsafe { read_volatile((regs + off as u64) as *const u32) }
}

unsafe fn wr(regs: u64, off: usize, v: u32) {
    unsafe { write_volatile((regs + off as u64) as *mut u32, v) }
}

pub fn read_byte() -> Option<u8> {
    STATE.with(|st| {
        if matches!(st, State::Untried) {
            if !mm::paging_ready() {
                return None;
            }
            *st = match unsafe { init() } {
                Some(k) => State::Ready(k),
                None => State::Absent,
            };
        }
        match st {
            State::Ready(k) => unsafe { k.poll() },
            _ => None,
        }
    })
}

unsafe fn init() -> Option<Kbd> {
    let va = mmio_map(MMIO_BASE, MMIO_SLOTS * MMIO_STRIDE)?;
    for slot in 0..MMIO_SLOTS {
        let regs = va + slot * MMIO_STRIDE;
        unsafe {
            if rd(regs, MAGIC) != MAGIC_VALUE || rd(regs, DEVICE_ID) != ID_INPUT {
                continue;
            }
            if let Some(k) = setup(regs) {
                crate::kprintln!(
                    "ifsy: input: virtio keyboard at {:#x} {}",
                    MMIO_BASE + slot * MMIO_STRIDE,
                    crate::console::color::pass()
                );
                return Some(k);
            }
        }
    }
    None
}

unsafe fn setup(regs: u64) -> Option<Kbd> {
    let version = unsafe { rd(regs, VERSION) };
    if version != 1 && version != 2 {
        return None;
    }
    let hhdm = mm::hhdm();
    let phys = mm::with_frames(|f| f.alloc_contiguous(3, 1))?;
    let mem = phys.0 + hhdm;
    unsafe { core::ptr::write_bytes(mem as *mut u8, 0, 3 * PAGE_SIZE as usize) };

    unsafe {
        wr(regs, STATUS, 0);
        wr(regs, STATUS, ST_ACK);
        wr(regs, STATUS, ST_ACK | ST_DRIVER);

        if version == 2 {
            wr(regs, DEVICE_FEATURES_SEL, 1);
            let hi = rd(regs, DEVICE_FEATURES);
            if hi & 1 == 0 {
                return None;
            }
            wr(regs, DRIVER_FEATURES_SEL, 0);
            wr(regs, DRIVER_FEATURES, 0);
            wr(regs, DRIVER_FEATURES_SEL, 1);
            wr(regs, DRIVER_FEATURES, 1);
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_FEATURES_OK);
            if rd(regs, STATUS) & ST_FEATURES_OK == 0 {
                return None;
            }
        } else {
            wr(regs, DRIVER_FEATURES, 0);
            wr(regs, GUEST_PAGE_SIZE, PAGE_SIZE as u32);
        }

        wr(regs, QUEUE_SEL, 0);
        if rd(regs, QUEUE_NUM_MAX) < QSIZE as u32 {
            return None;
        }
        wr(regs, QUEUE_NUM, QSIZE as u32);
        if version == 1 {
            wr(regs, QUEUE_ALIGN, PAGE_SIZE as u32);
            wr(regs, QUEUE_PFN, (phys.0 / PAGE_SIZE) as u32);
        } else {
            let (d, a, u) = (phys.0 + OFF_DESC as u64, phys.0 + OFF_AVAIL as u64, phys.0 + OFF_USED as u64);
            wr(regs, QUEUE_DESC_LOW, d as u32);
            wr(regs, QUEUE_DESC_HIGH, (d >> 32) as u32);
            wr(regs, QUEUE_DRIVER_LOW, a as u32);
            wr(regs, QUEUE_DRIVER_HIGH, (a >> 32) as u32);
            wr(regs, QUEUE_DEVICE_LOW, u as u32);
            wr(regs, QUEUE_DEVICE_HIGH, (u >> 32) as u32);
        }

        let desc = (mem as usize + OFF_DESC) as *mut u8;
        let avail = (mem as usize + OFF_AVAIL) as *mut u16;
        for i in 0..QSIZE {
            let d = desc.add(i * 16);
            let addr = phys.0 + (OFF_EVENTS + i * EVENT_SIZE) as u64;
            write_volatile(d as *mut u64, addr);
            write_volatile(d.add(8) as *mut u32, EVENT_SIZE as u32);
            write_volatile(d.add(12) as *mut u16, DESC_WRITE);
            write_volatile(d.add(14) as *mut u16, 0);
            write_volatile(avail.add(2 + i), i as u16);
        }
        fence(Ordering::SeqCst);
        write_volatile(avail.add(1), QSIZE as u16);
        fence(Ordering::SeqCst);

        if version == 2 {
            wr(regs, QUEUE_READY, 1);
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_FEATURES_OK | ST_DRIVER_OK);
        } else {
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_DRIVER_OK);
        }
        wr(regs, QUEUE_NOTIFY, 0);
    }
    Some(Kbd { regs, mem, mem_phys: phys.0, last_used: 0, keys: Keymap::new() })
}

impl Kbd {
    unsafe fn poll(&mut self) -> Option<u8> {
        let used = (self.mem as usize + OFF_USED) as *const u8;
        let avail = (self.mem as usize + OFF_AVAIL) as *mut u16;
        loop {
            fence(Ordering::SeqCst);
            let idx = unsafe { read_volatile(used.add(2) as *const u16) };
            if idx == self.last_used {
                return None;
            }
            let slot = self.last_used as usize % QSIZE;
            let id = unsafe { read_volatile(used.add(4 + slot * 8) as *const u32) } as usize % QSIZE;
            self.last_used = self.last_used.wrapping_add(1);

            let ev = (self.mem as usize + OFF_EVENTS + id * EVENT_SIZE) as *const u8;
            let (ty, code, value) = unsafe {
                (read_volatile(ev as *const u16), read_volatile(ev.add(2) as *const u16), read_volatile(ev.add(4) as *const u32))
            };

            unsafe {
                let aidx = read_volatile(avail.add(1));
                write_volatile(avail.add(2 + aidx as usize % QSIZE), id as u16);
                fence(Ordering::SeqCst);
                write_volatile(avail.add(1), aidx.wrapping_add(1));
                fence(Ordering::SeqCst);
                let st = rd(self.regs, INTERRUPT_STATUS);
                if st != 0 {
                    wr(self.regs, INTERRUPT_ACK, st);
                }
                wr(self.regs, QUEUE_NOTIFY, 0);
            }

            if ty == EV_KEY {
                if let Some(c) = self.keys.key(code, value) {
                    return Some(c);
                }
            }
        }
    }
}
