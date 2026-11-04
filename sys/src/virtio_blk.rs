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

use crate::arch::port::{VIRTIO_MMIO_BASE as MMIO_BASE, VIRTIO_MMIO_SLOTS as MMIO_SLOTS, VIRTIO_MMIO_STRIDE as MMIO_STRIDE, mmio_map};
use crate::console::color::{pass, warn};
use crate::gpt::Sectors;
use crate::mm;
use crate::timer::Deadline;

const SECTOR: usize = 512;
const PAGE: u64 = 4096;

const MAGIC: u64 = 0x000;
const VERSION: u64 = 0x004;
const DEVICE_ID: u64 = 0x008;
const DEVICE_FEATURES: u64 = 0x010;
const DEVICE_FEATURES_SEL: u64 = 0x014;
const DRIVER_FEATURES: u64 = 0x020;
const DRIVER_FEATURES_SEL: u64 = 0x024;
const GUEST_PAGE_SIZE: u64 = 0x028;
const QUEUE_SEL: u64 = 0x030;
const QUEUE_NUM_MAX: u64 = 0x034;
const QUEUE_NUM: u64 = 0x038;
const QUEUE_ALIGN: u64 = 0x03c;
const QUEUE_PFN: u64 = 0x040;
const QUEUE_READY: u64 = 0x044;
const QUEUE_NOTIFY: u64 = 0x050;
const INTERRUPT_STATUS: u64 = 0x060;
const INTERRUPT_ACK: u64 = 0x064;
const STATUS: u64 = 0x070;
const QUEUE_DESC_LOW: u64 = 0x080;
const QUEUE_DESC_HIGH: u64 = 0x084;
const QUEUE_DRIVER_LOW: u64 = 0x090;
const QUEUE_DRIVER_HIGH: u64 = 0x094;
const QUEUE_DEVICE_LOW: u64 = 0x0a0;
const QUEUE_DEVICE_HIGH: u64 = 0x0a4;
const CONFIG: u64 = 0x100;

const MAGIC_VALUE: u32 = 0x7472_6976;
const ID_BLOCK: u32 = 2;

const ST_ACK: u32 = 1;
const ST_DRIVER: u32 = 2;
const ST_DRIVER_OK: u32 = 4;
const ST_FEATURES_OK: u32 = 8;
const ST_FAILED: u32 = 128;

const F_FLUSH: u32 = 1 << 9;

const T_IN: u32 = 0;
const T_OUT: u32 = 1;
const T_FLUSH: u32 = 4;

const QSIZE: usize = 8;
const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

const OFF_DESC: u64 = 0;
const OFF_AVAIL: u64 = 256;
const OFF_USED: u64 = PAGE;
const OFF_REQ: u64 = 2 * PAGE;
const OFF_STATUS: u64 = OFF_REQ + 16;
const OFF_BUF: u64 = 3 * PAGE;
const BUF_PAGES: u64 = 16;
const BUF_SECTORS: usize = (BUF_PAGES * PAGE) as usize / SECTOR;
const TOTAL_PAGES: u64 = 3 + BUF_PAGES;

pub struct VirtioBlk {
    regs: u64,
    mem: u64,
    phys: u64,
    sectors: u64,
    flush: bool,
    avail_idx: u16,
    used_idx: u16,
}

// SAFETY: access to the inner value is serialized by the lock (or the data is only moved once), so sharing across threads cannot race.
unsafe impl Send for VirtioBlk {}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn rd(regs: u64, off: u64) -> u32 {
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    unsafe { read_volatile((regs + off) as *const u32) }
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn wr(regs: u64, off: u64, v: u32) {
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    unsafe { write_volatile((regs + off) as *mut u32, v) }
}

pub fn probe() -> Option<VirtioBlk> {
    let Some(va) = mmio_map(MMIO_BASE, MMIO_SLOTS * MMIO_STRIDE) else {
        kprintln!("ifsy: virtio-blk: cannot map the virtio-mmio window {}", warn());
        return None;
    };
    for slot in 0..MMIO_SLOTS {
        let regs = va + slot * MMIO_STRIDE;
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        let hit = unsafe { rd(regs, MAGIC) == MAGIC_VALUE && rd(regs, DEVICE_ID) == ID_BLOCK };
        if !hit {
            continue;
        }
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        if let Some(d) = unsafe { setup(regs) } {
            kprintln!(
                "ifsy: virtio-blk: disk at {:#x}, {} MiB {}",
                MMIO_BASE + slot * MMIO_STRIDE,
                (d.sectors * SECTOR as u64) >> 20,
                pass()
            );
            return Some(d);
        }
    }
    kprintln!("ifsy: virtio-blk: no virtio block device found {}", warn());
    None
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
unsafe fn setup(regs: u64) -> Option<VirtioBlk> {
    unsafe {
        let version = rd(regs, VERSION);
        if version != 1 && version != 2 {
            return None;
        }
        let phys = mm::with_frames(|f| f.alloc_contiguous(TOTAL_PAGES, 1))?;
        let mem = phys.0 + mm::hhdm();
        core::ptr::write_bytes(mem as *mut u8, 0, (TOTAL_PAGES * PAGE) as usize);

        wr(regs, STATUS, 0);
        wr(regs, STATUS, ST_ACK);
        wr(regs, STATUS, ST_ACK | ST_DRIVER);

        wr(regs, DEVICE_FEATURES_SEL, 0);
        let flush = rd(regs, DEVICE_FEATURES) & F_FLUSH != 0;
        let lo = if flush { F_FLUSH } else { 0 };
        if version == 2 {
            wr(regs, DEVICE_FEATURES_SEL, 1);
            if rd(regs, DEVICE_FEATURES) & 1 == 0 {
                wr(regs, STATUS, ST_FAILED);
                return None;
            }
            wr(regs, DRIVER_FEATURES_SEL, 0);
            wr(regs, DRIVER_FEATURES, lo);
            wr(regs, DRIVER_FEATURES_SEL, 1);
            wr(regs, DRIVER_FEATURES, 1);
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_FEATURES_OK);
            if rd(regs, STATUS) & ST_FEATURES_OK == 0 {
                wr(regs, STATUS, ST_FAILED);
                return None;
            }
        } else {
            wr(regs, DRIVER_FEATURES, lo);
            wr(regs, GUEST_PAGE_SIZE, PAGE as u32);
        }

        wr(regs, QUEUE_SEL, 0);
        if rd(regs, QUEUE_NUM_MAX) < QSIZE as u32 {
            wr(regs, STATUS, ST_FAILED);
            return None;
        }
        wr(regs, QUEUE_NUM, QSIZE as u32);
        if version == 1 {
            wr(regs, QUEUE_ALIGN, PAGE as u32);
            wr(regs, QUEUE_PFN, (phys.0 / PAGE) as u32);
        } else {
            let (d, a, u) = (phys.0 + OFF_DESC, phys.0 + OFF_AVAIL, phys.0 + OFF_USED);
            wr(regs, QUEUE_DESC_LOW, d as u32);
            wr(regs, QUEUE_DESC_HIGH, (d >> 32) as u32);
            wr(regs, QUEUE_DRIVER_LOW, a as u32);
            wr(regs, QUEUE_DRIVER_HIGH, (a >> 32) as u32);
            wr(regs, QUEUE_DEVICE_LOW, u as u32);
            wr(regs, QUEUE_DEVICE_HIGH, (u >> 32) as u32);
            wr(regs, QUEUE_READY, 1);
        }
        let ok = if version == 2 { ST_ACK | ST_DRIVER | ST_FEATURES_OK | ST_DRIVER_OK } else { ST_ACK | ST_DRIVER | ST_DRIVER_OK };
        wr(regs, STATUS, ok);

        let sectors = rd(regs, CONFIG) as u64 | (rd(regs, CONFIG + 4) as u64) << 32;
        if sectors == 0 {
            return None;
        }
        Some(VirtioBlk { regs, mem, phys: phys.0, sectors, flush, avail_idx: 0, used_idx: 0 })
    }
}

impl VirtioBlk {
    fn request(&mut self, ty: u32, lba: u64, len: usize) -> Result<(), ()> {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe {
            let req = (self.mem + OFF_REQ) as *mut u8;
            write_volatile(req as *mut u32, ty);
            write_volatile(req.add(4) as *mut u32, 0);
            write_volatile(req.add(8) as *mut u64, lba);
            let status = (self.mem + OFF_STATUS) as *mut u8;
            write_volatile(status, 0xff);

            let desc = (self.mem + OFF_DESC) as *mut u8;
            let put = |i: usize, addr: u64, n: u32, flags: u16, next: u16| {
                let d = desc.add(i * 16);
                write_volatile(d as *mut u64, addr);
                write_volatile(d.add(8) as *mut u32, n);
                write_volatile(d.add(12) as *mut u16, flags);
                write_volatile(d.add(14) as *mut u16, next);
            };
            put(0, self.phys + OFF_REQ, 16, DESC_NEXT, if len > 0 { 1 } else { 2 });
            let mut last = 0;
            if len > 0 {
                let w = if ty == T_IN { DESC_WRITE } else { 0 };
                put(1, self.phys + OFF_BUF, len as u32, DESC_NEXT | w, 2);
                last = 1;
            }
            let _ = last;
            put(2, self.phys + OFF_STATUS, 1, DESC_WRITE, 0);

            let avail = (self.mem + OFF_AVAIL) as *mut u16;
            write_volatile(avail.add(2 + self.avail_idx as usize % QSIZE), 0);
            fence(Ordering::SeqCst);
            self.avail_idx = self.avail_idx.wrapping_add(1);
            write_volatile(avail.add(1), self.avail_idx);
            fence(Ordering::SeqCst);
            wr(self.regs, QUEUE_NOTIFY, 0);

            let used = (self.mem + OFF_USED) as *const u16;
            let limit = Deadline::after_ms(5000);
            loop {
                fence(Ordering::SeqCst);
                if read_volatile(used.add(1)) != self.used_idx {
                    break;
                }
                if limit.expired() {
                    return Err(());
                }
                core::hint::spin_loop();
            }
            self.used_idx = self.used_idx.wrapping_add(1);
            let st = rd(self.regs, INTERRUPT_STATUS);
            if st != 0 {
                wr(self.regs, INTERRUPT_ACK, st);
            }
            fence(Ordering::SeqCst);
            if read_volatile(status) == 0 { Ok(()) } else { Err(()) }
        }
    }
}

impl Sectors for VirtioBlk {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, mut lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        if buf.len() % SECTOR != 0 || lba + (buf.len() / SECTOR) as u64 > self.sectors {
            return Err(());
        }
        for chunk in buf.chunks_mut(BUF_SECTORS * SECTOR) {
            self.request(T_IN, lba, chunk.len())?;
            // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
            unsafe { core::ptr::copy_nonoverlapping((self.mem + OFF_BUF) as *const u8, chunk.as_mut_ptr(), chunk.len()) };
            lba += (chunk.len() / SECTOR) as u64;
        }
        Ok(())
    }

    fn write(&mut self, mut lba: u64, buf: &[u8]) -> Result<(), ()> {
        if buf.len() % SECTOR != 0 || lba + (buf.len() / SECTOR) as u64 > self.sectors {
            return Err(());
        }
        for chunk in buf.chunks(BUF_SECTORS * SECTOR) {
            // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
            unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), (self.mem + OFF_BUF) as *mut u8, chunk.len()) };
            self.request(T_OUT, lba, chunk.len())?;
            lba += (chunk.len() / SECTOR) as u64;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ()> {
        if self.flush { self.request(T_FLUSH, 0, 0) } else { Ok(()) }
    }
}
