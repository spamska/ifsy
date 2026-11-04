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

use crate::console::color::{failed, pass, warn};
use crate::mm;
use crate::pci;
use crate::gpt::Sectors;
use crate::timer::Deadline;

const SECTOR: usize = 512;
const BUF_SECTORS: usize = 128;
const BUF_PAGES: u64 = 16;
const PAGE: u64 = 4096;

const CT_OFF: u64 = 0x800;
const FB_OFF: u64 = 0x400;

const HBA_CAP: u64 = 0x00;
const HBA_GHC: u64 = 0x04;
const HBA_PI: u64 = 0x0c;
const P_CLB: u64 = 0x00;
const P_CLBU: u64 = 0x04;
const P_FB: u64 = 0x08;
const P_FBU: u64 = 0x0c;
const P_IS: u64 = 0x10;
const P_IE: u64 = 0x14;
const P_CMD: u64 = 0x18;
const P_TFD: u64 = 0x20;
const P_SIG: u64 = 0x24;
const P_SSTS: u64 = 0x28;
const P_SERR: u64 = 0x30;
const P_CI: u64 = 0x38;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;
const TFD_BUSY: u32 = 0x88;
const TFD_ERR: u32 = 0x01;
const TFD_DF: u32 = 0x20;
const IS_TFES: u32 = 1 << 30;

const ATA_READ_DMA_EXT: u8 = 0x25;
const ATA_WRITE_DMA_EXT: u8 = 0x35;
const ATA_FLUSH_EXT: u8 = 0xea;
const ATA_IDENTIFY: u8 = 0xec;

pub struct Ahci {
    port: u64,
    page: u64,
    page_phys: u64,
    buf: u64,
    buf_phys: u64,
    sectors: u64,
}

fn find_controller() -> Option<(u32, u32, u32)> {
    pci::find_class(0x01_06_01)
}

use crate::pci::{cfg_read, cfg_write};

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn r32(a: u64) -> u32 {
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    unsafe { read_volatile(a as *const u32) }
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn w32(a: u64, v: u32) {
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    unsafe { write_volatile(a as *mut u32, v) };
}

fn wait(ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let d = Deadline::after_ms(ms);
    loop {
        if done() {
            return true;
        }
        if d.expired() {
            return done();
        }
        core::hint::spin_loop();
    }
}

fn stop_port(p: u64) -> bool {
    // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
    unsafe {
        w32(p + P_CMD, r32(p + P_CMD) & !(CMD_ST | CMD_FRE));
        wait(500, || r32(p + P_CMD) & (CMD_CR | CMD_FR) == 0)
    }
}

pub fn probe() -> Option<Ahci> {
    let Some((b, d, f)) = find_controller() else {
        kprintln!("ifsy: ahci: no AHCI controller on the PCI bus {}", warn());
        return None;
    };
    cfg_write(b, d, f, 4, (cfg_read(b, d, f, 4) & 0xffff) | 0x6);
    let bar5 = cfg_read(b, d, f, 0x24);
    let hi = if (bar5 >> 1) & 3 == 2 { cfg_read(b, d, f, 0x28) } else { 0 };
    if bar5 & 1 != 0 || hi != 0 || bar5 & !0xf == 0 {
        kprintln!("ifsy: ahci: controller {:02x}:{:02x}.{} has no usable memory BAR {}", b, d, f, warn());
        return None;
    }
    let bar = (bar5 & !0xf) as u64;
    let off = bar & (PAGE - 1);
    let pages = (off + 0x1100).div_ceil(PAGE);
    let Some(va) = mm::aspace::kvm::map_phys(mm::PhysAddr(bar - off), pages, mm::PageFlags::MMIO) else {
        kprintln!("ifsy: ahci: cannot map the controller registers {}", failed());
        return None;
    };
    let abar = va + off;
    // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
    unsafe {
        w32(abar + HBA_GHC, r32(abar + HBA_GHC) | 1 << 31);
        let s64 = r32(abar + HBA_CAP) >> 31 & 1 != 0;
        let pi = r32(abar + HBA_PI);
        for n in 0..32u64 {
            if pi >> n & 1 == 0 {
                continue;
            }
            let p = abar + 0x100 + 0x80 * n;
            let ssts = r32(p + P_SSTS);
            if ssts & 0xf != 3 || (ssts >> 8) & 0xf != 1 || r32(p + P_SIG) != 0x101 {
                continue;
            }
            if let Some(a) = bring_up(p, s64) {
                kprintln!(
                    "ifsy: ahci: {:02x}:{:02x}.{} port {}: {} sectors ({} MiB) {}",
                    b, d, f, n, a.sectors, a.sectors / 2048, pass()
                );
                return Some(a);
            }
        }
    }
    kprintln!("ifsy: ahci: no SATA disk found on the controller {}", warn());
    None
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn bring_up(p: u64, s64: bool) -> Option<Ahci> {
    if !stop_port(p) {
        return None;
    }
    let phys = mm::with_frames(|fr| fr.alloc_contiguous(1 + BUF_PAGES, 1))?.0;
    if !s64 && phys + (1 + BUF_PAGES) * PAGE > 1 << 32 {
        mm::with_frames(|fr| fr.free_contiguous(mm::PhysAddr(phys), 1 + BUF_PAGES));
        return None;
    }
    let virt = phys + mm::hhdm();
    // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, PAGE as usize) };
    let mut a = Ahci { port: p, page: virt, page_phys: phys, buf: virt + PAGE, buf_phys: phys + PAGE, sectors: 0 };
    unsafe {
        w32(p + P_CLB, phys as u32);
        w32(p + P_CLBU, (phys >> 32) as u32);
        w32(p + P_FB, (phys + FB_OFF) as u32);
        w32(p + P_FBU, ((phys + FB_OFF) >> 32) as u32);
        w32(p + P_SERR, !0);
        w32(p + P_IS, !0);
        w32(p + P_IE, 0);
        w32(p + P_CMD, r32(p + P_CMD) | CMD_FRE);
        if !wait(1000, || r32(p + P_TFD) & TFD_BUSY == 0) {
            mm::with_frames(|fr| fr.free_contiguous(mm::PhysAddr(phys), 1 + BUF_PAGES));
            return None;
        }
        w32(p + P_CMD, r32(p + P_CMD) | CMD_ST);
    }
    if a.command(ATA_IDENTIFY, 0, 0, false, SECTOR).is_err() {
        kprintln!("ifsy: ahci: IDENTIFY failed {}", failed());
        stop_port(p);
        mm::with_frames(|fr| fr.free_contiguous(mm::PhysAddr(phys), 1 + BUF_PAGES));
        return None;
    }
    let mut id = [0u16; 256];
    for (i, w) in id.iter_mut().enumerate() {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        *w = unsafe { read_volatile((a.buf as *const u16).add(i)) };
    }
    let lba48 = id[83] & (1 << 10) != 0;
    let total = if lba48 {
        id[100] as u64 | (id[101] as u64) << 16 | (id[102] as u64) << 32 | (id[103] as u64) << 48
    } else {
        id[60] as u64 | (id[61] as u64) << 16
    };
    let big_sector = id[106] & (1 << 14) != 0 && id[106] & (1 << 12) != 0 && (id[117] as u32 | (id[118] as u32) << 16) != 256;
    if !lba48 || total == 0 || big_sector {
        kprintln!("ifsy: ahci: disk is not 48-bit LBA with 512-byte sectors; not supported {}", warn());
        stop_port(p);
        mm::with_frames(|fr| fr.free_contiguous(mm::PhysAddr(phys), 1 + BUF_PAGES));
        return None;
    }
    a.sectors = total;
    Some(a)
}

impl Ahci {
    fn command(&mut self, cmd: u8, lba: u64, count: u16, write: bool, bytes: usize) -> Result<(), ()> {
        let p = self.port;
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        unsafe {
            if !wait(1000, || r32(p + P_TFD) & TFD_BUSY == 0) {
                return Err(());
            }
            w32(p + P_IS, !0);
            w32(p + P_SERR, !0);
            let hdr = self.page as *mut u32;
            let prdtl: u32 = if bytes > 0 { 1 } else { 0 };
            write_volatile(hdr, 5 | if write { 1 << 6 } else { 0 } | prdtl << 16);
            write_volatile(hdr.add(1), 0);
            write_volatile(hdr.add(2), (self.page_phys + CT_OFF) as u32);
            write_volatile(hdr.add(3), ((self.page_phys + CT_OFF) >> 32) as u32);

            let ct = (self.page + CT_OFF) as *mut u8;
            core::ptr::write_bytes(ct, 0, 0x90);
            let dev = if cmd == ATA_IDENTIFY { 0 } else { 0x40 };
            let fis: [u8; 16] = [
                0x27, 0x80, cmd, 0,
                lba as u8, (lba >> 8) as u8, (lba >> 16) as u8, dev,
                (lba >> 24) as u8, (lba >> 32) as u8, (lba >> 40) as u8, 0,
                count as u8, (count >> 8) as u8, 0, 0,
            ];
            for (i, v) in fis.iter().enumerate() {
                write_volatile(ct.add(i), *v);
            }
            if bytes > 0 {
                let prd = ct.add(0x80) as *mut u32;
                write_volatile(prd, self.buf_phys as u32);
                write_volatile(prd.add(1), (self.buf_phys >> 32) as u32);
                write_volatile(prd.add(2), 0);
                write_volatile(prd.add(3), (bytes - 1) as u32);
            }
            fence(Ordering::SeqCst);
            w32(p + P_CI, 1);
            let ok = wait(10_000, || r32(p + P_CI) & 1 == 0 || r32(p + P_IS) & IS_TFES != 0);
            fence(Ordering::SeqCst);
            let tfd = r32(p + P_TFD);
            if !ok || r32(p + P_IS) & IS_TFES != 0 || tfd & (TFD_ERR | TFD_DF) != 0 || r32(p + P_CI) & 1 != 0 {
                return Err(());
            }
        }
        Ok(())
    }
}

impl Sectors for Ahci {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, mut lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        if buf.len() % SECTOR != 0 || lba + (buf.len() / SECTOR) as u64 > self.sectors {
            return Err(());
        }
        for chunk in buf.chunks_mut(BUF_SECTORS * SECTOR) {
            let n = chunk.len() / SECTOR;
            self.command(ATA_READ_DMA_EXT, lba, n as u16, false, chunk.len())?;
            // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
            unsafe { core::ptr::copy_nonoverlapping(self.buf as *const u8, chunk.as_mut_ptr(), chunk.len()) };
            lba += n as u64;
        }
        Ok(())
    }

    fn write(&mut self, mut lba: u64, buf: &[u8]) -> Result<(), ()> {
        if buf.len() % SECTOR != 0 || lba + (buf.len() / SECTOR) as u64 > self.sectors {
            return Err(());
        }
        for chunk in buf.chunks(BUF_SECTORS * SECTOR) {
            let n = chunk.len() / SECTOR;
            // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
            unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), self.buf as *mut u8, chunk.len()) };
            self.command(ATA_WRITE_DMA_EXT, lba, n as u16, true, chunk.len())?;
            lba += n as u64;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), ()> {
        self.command(ATA_FLUSH_EXT, 0, 0, false, 0)
    }
}
