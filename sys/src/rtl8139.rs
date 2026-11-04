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

use core::marker::PhantomData;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::console::color::{pass, warn};
use crate::mm;
#[cfg(target_arch = "x86_64")]
use crate::pci::{inb, inl, inw, outb, outl, outw};
use crate::pci::{cfg_read, cfg_write, find_id};

const VENDOR: u32 = 0x10ec;
const DEVICE: u32 = 0x8139;

const IDR0: u16 = 0x00;
const TSD0: u16 = 0x10;
const TSAD0: u16 = 0x20;
const RBSTART: u16 = 0x30;
const CR: u16 = 0x37;
const CAPR: u16 = 0x38;
const IMR: u16 = 0x3c;
const ISR: u16 = 0x3e;
const TCR: u16 = 0x40;
const RCR: u16 = 0x44;
const CONFIG1: u16 = 0x52;

const CR_BUFE: u8 = 1 << 0;
const CR_TE: u8 = 1 << 2;
const CR_RE: u8 = 1 << 3;
const CR_RST: u8 = 1 << 4;
const TSD_OWN: u32 = 1 << 13;
const ISR_ROK: u16 = 1 << 0;
const ISR_TOK: u16 = 1 << 2;
const ISR_RXOVW: u16 = 1 << 4;
const ISR_RER: u16 = 1 << 1;
const ISR_TER: u16 = 1 << 3;
const RX_OK: u16 = 1 << 0;

const PAGE: u64 = 4096;
const RX_LEN: usize = 8192;
const RX_PAGES: u64 = 3;
const TX_BUF: usize = 2048;
const MTU: usize = 1514;
const MIN_FRAME: usize = 60;
const DMA_LIMIT: u64 = 1 << 32;

#[derive(Clone, Copy)]
enum Reg {
    Mem(usize),
    #[cfg(target_arch = "x86_64")]
    Port(u16),
}

pub struct Rtl8139 {
    io: Reg,
    mac: [u8; 6],
    rx: u64,
    rx_phys: u64,
    tx: u64,
    rx_off: usize,
    cur_tx: usize,
    irq: u8,
}

// SAFETY: access to the inner value is serialized by the lock (or the data is only moved once), so sharing across threads cannot race.
unsafe impl Send for Rtl8139 {}

fn alloc_dma(pages: u64) -> Option<(u64, u64)> {
    let phys = mm::with_frames(|f| f.alloc_contiguous(pages, 1))?;
    if phys.0 + pages * PAGE > DMA_LIMIT {
        return None;
    }
    let virt = phys.0 + mm::hhdm();
    // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, (pages * PAGE) as usize) };
    Some((virt, phys.0))
}

impl Rtl8139 {
    fn r8(&self, off: u16) -> u8 {
        match self.io {
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Mem(b) => unsafe { read_volatile((b + off as usize) as *const u8) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Port(p) => unsafe { inb(p + off) },
        }
    }

    fn r16(&self, off: u16) -> u16 {
        match self.io {
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Mem(b) => unsafe { read_volatile((b + off as usize) as *const u16) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Port(p) => unsafe { inw(p + off) },
        }
    }

    fn r32(&self, off: u16) -> u32 {
        match self.io {
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Mem(b) => unsafe { read_volatile((b + off as usize) as *const u32) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Port(p) => unsafe { inl(p + off) },
        }
    }

    fn w8(&self, off: u16, v: u8) {
        match self.io {
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Mem(b) => unsafe { write_volatile((b + off as usize) as *mut u8, v) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Port(p) => unsafe { outb(p + off, v) },
        }
    }

    fn w16(&self, off: u16, v: u16) {
        match self.io {
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Mem(b) => unsafe { write_volatile((b + off as usize) as *mut u16, v) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Port(p) => unsafe { outw(p + off, v) },
        }
    }

    fn w32(&self, off: u16, v: u32) {
        match self.io {
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Mem(b) => unsafe { write_volatile((b + off as usize) as *mut u32, v) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Reg::Port(p) => unsafe { outl(p + off, v) },
        }
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn irq_line(&self) -> u8 {
        self.irq
    }

    pub fn enable_irq(&self) {
        self.w16(IMR, ISR_ROK | ISR_TOK | ISR_RER | ISR_TER | ISR_RXOVW);
    }

    pub fn ack_irq(&self) -> bool {
        let st = self.r16(ISR);
        self.w16(ISR, st);
        st != 0
    }

    fn rx_ready(&self) -> bool {
        self.r8(CR) & CR_BUFE == 0
    }

    fn tx_free(&self) -> bool {
        self.r32(TSD0 + 4 * self.cur_tx as u16) & TSD_OWN != 0
    }

    fn rx_advance(&mut self, total: usize) {
        self.rx_off = (self.rx_off + total + 4 + 3) & !3;
        self.w16(CAPR, (self.rx_off as u16).wrapping_sub(16));
        self.rx_off %= RX_LEN;
    }
}

pub struct Rtl8139Rx<'a> {
    dev: *mut Rtl8139,
    _p: PhantomData<&'a mut Rtl8139>,
}

pub struct Rtl8139Tx<'a> {
    dev: *mut Rtl8139,
    _p: PhantomData<&'a mut Rtl8139>,
}

impl RxToken for Rtl8139Rx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        let dev = unsafe { &mut *self.dev };
        let hdr = (dev.rx + dev.rx_off as u64) as *const u8;
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        let status = unsafe { read_volatile(hdr as *const u16) };
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        let total = unsafe { read_volatile(hdr.add(2) as *const u16) } as usize;
        let ok = status & RX_OK != 0 && total >= 4 && total <= MTU + 4 + 4;
        let r = if ok {
            // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
            f(unsafe { core::slice::from_raw_parts(hdr.add(4), total - 4) })
        } else {
            f(&[])
        };
        if ok {
            dev.rx_advance(total);
        } else {
            dev.w8(CR, CR_TE);
            dev.rx_off = 0;
            dev.w32(RBSTART, dev.rx_phys as u32);
            dev.w8(CR, CR_TE | CR_RE);
        }
        r
    }
}

impl TxToken for Rtl8139Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        // SAFETY: the pointer is valid for the lifetime of its owner and access is exclusive (&mut self or the owning lock is held).
        let dev = unsafe { &mut *self.dev };
        let i = dev.cur_tx;
        let len = len.min(MTU);
        let buf = dev.tx + (i * TX_BUF) as u64;
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        let slice = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len.max(MIN_FRAME)) };
        slice[len..].fill(0);
        let r = f(&mut slice[..len]);
        fence(Ordering::SeqCst);
        dev.w32(TSD0 + 4 * i as u16, len.max(MIN_FRAME) as u32);
        dev.cur_tx = (i + 1) % 4;
        r
    }
}

impl Device for Rtl8139 {
    type RxToken<'a> = Rtl8139Rx<'a>;
    type TxToken<'a> = Rtl8139Tx<'a>;

    fn receive(&mut self, _ts: Instant) -> Option<(Rtl8139Rx<'_>, Rtl8139Tx<'_>)> {
        if !self.rx_ready() || !self.tx_free() {
            return None;
        }
        let dev = self as *mut Rtl8139;
        Some((Rtl8139Rx { dev, _p: PhantomData }, Rtl8139Tx { dev, _p: PhantomData }))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Rtl8139Tx<'_>> {
        if !self.tx_free() {
            return None;
        }
        Some(Rtl8139Tx { dev: self as *mut Rtl8139, _p: PhantomData })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        caps.max_burst_size = Some(1);
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}

pub fn probe() -> Option<Rtl8139> {
    let (b, d, f) = find_id(VENDOR, DEVICE)?;
    cfg_write(b, d, f, 4, (cfg_read(b, d, f, 4) & 0xffff) | 0x7);
    let bar0 = cfg_read(b, d, f, 0x10);
    let bar1 = cfg_read(b, d, f, 0x14);
    #[cfg(target_arch = "x86_64")]
    let chosen = if bar0 & 1 == 1 {
        Some(Reg::Port((bar0 & !3) as u16))
    } else if bar1 & 1 == 0 && bar1 & !0xf != 0 {
        crate::arch::port::mmio_map((bar1 & !0xf) as u64, 0x100).map(|v| Reg::Mem(v as usize))
    } else {
        None
    };
    #[cfg(not(target_arch = "x86_64"))]
    let chosen = if bar1 & 1 == 0 && bar1 & !0xf != 0 {
        crate::arch::port::mmio_map((bar1 & !0xf) as u64, 0x100).map(|v| Reg::Mem(v as usize))
    } else {
        None
    };
    let Some(io) = chosen else {
        kprintln!("ifsy: net: rtl8139 {:02x}:{:02x}.{} has no usable BAR {}", b, d, f, warn());
        return None;
    };
    let irq = (cfg_read(b, d, f, 0x3c) & 0xff) as u8;
    let Some((rx, rx_p)) = alloc_dma(RX_PAGES) else {
        kprintln!("ifsy: net: rtl8139 needs a receive buffer below 4 GiB {}", warn());
        return None;
    };
    let Some((tx, tx_p)) = alloc_dma(2) else {
        kprintln!("ifsy: net: rtl8139 needs transmit buffers below 4 GiB {}", warn());
        return None;
    };
    let mut dev = Rtl8139 { io, mac: [0; 6], rx, rx_phys: rx_p, tx, rx_off: 0, cur_tx: 0, irq };
    dev.w8(CONFIG1, 0);
    dev.w8(CR, CR_RST);
    for _ in 0..100_000 {
        if dev.r8(CR) & CR_RST == 0 {
            break;
        }
    }
    for i in 0..6 {
        dev.mac[i] = dev.r8(IDR0 + i as u16);
    }
    dev.w32(RBSTART, rx_p as u32);
    for i in 0..4u16 {
        dev.w32(TSAD0 + 4 * i, (tx_p + (i as u64) * TX_BUF as u64) as u32);
    }
    dev.w16(IMR, 0);
    dev.w32(TCR, 0x0300_0700);
    dev.w32(RCR, 0x0000_e78e);
    dev.w8(CR, CR_TE | CR_RE);
    let m = dev.mac;
    kprintln!(
        "ifsy: net: rtl8139 at {:02x}:{:02x}.{}, mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} {}",
        b, d, f, m[0], m[1], m[2], m[3], m[4], m[5], pass()
    );
    Some(dev)
}
