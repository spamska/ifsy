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
use crate::pci::{inl, outl};
use crate::pci::{cfg_read, cfg_write, find};

const VENDOR: u32 = 0x8086;
const IDS: [u32; 38] = [
    0x1000, 0x1001, 0x1004, 0x1008, 0x1009, 0x100c, 0x100d, 0x100e, 0x100f, 0x1010, 0x1011, 0x1012, 0x1013, 0x1014,
    0x1015, 0x1016, 0x1017, 0x1018, 0x1019, 0x101a, 0x101d, 0x101e, 0x1026, 0x1027, 0x1028, 0x1075, 0x1076, 0x1077,
    0x1078, 0x1079, 0x107a, 0x107b, 0x107c, 0x108a, 0x1099, 0x10b5, 0x10b9, 0x10ba,
];

const CTRL: u32 = 0x0000;
const STATUS: u32 = 0x0008;
const ICR: u32 = 0x00c0;
const IMS: u32 = 0x00d0;
const IMC: u32 = 0x00d8;
const RCTL: u32 = 0x0100;
const TCTL: u32 = 0x0400;
const TIPG: u32 = 0x0410;
const RDBAL: u32 = 0x2800;
const RDBAH: u32 = 0x2804;
const RDLEN: u32 = 0x2808;
const RDH: u32 = 0x2810;
const RDT: u32 = 0x2818;
const TDBAL: u32 = 0x3800;
const TDBAH: u32 = 0x3804;
const TDLEN: u32 = 0x3808;
const TDH: u32 = 0x3810;
const TDT: u32 = 0x3818;
const MTA: u32 = 0x5200;
const RAL0: u32 = 0x5400;
const RAH0: u32 = 0x5404;
const EERD: u32 = 0x0014;
const RAH_AV: u32 = 1 << 31;

const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_RST: u32 = 1 << 26;
const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const IRQ_TXDW: u32 = 1 << 0;
const IRQ_LSC: u32 = 1 << 2;
const IRQ_RXDMT0: u32 = 1 << 4;
const IRQ_RXT0: u32 = 1 << 7;
const DD: u8 = 1;
const TX_CMD: u8 = 0x0b;

const PAGE: u64 = 4096;
const RING: usize = 32;
const BUF: usize = 2048;
const MTU: usize = 1514;

#[derive(Clone, Copy)]
enum Io {
    Mem(usize),
    #[cfg(target_arch = "x86_64")]
    Port(u16),
}

impl Io {
    fn rd(&self, off: u32) -> u32 {
        match *self {
            Io::Mem(b) => unsafe { read_volatile((b + off as usize) as *const u32) },
            #[cfg(target_arch = "x86_64")]
            Io::Port(p) => unsafe {
                outl(p, off);
                inl(p + 4)
            },
        }
    }

    fn wr(&self, off: u32, v: u32) {
        match *self {
            Io::Mem(b) => unsafe { write_volatile((b + off as usize) as *mut u32, v) },
            #[cfg(target_arch = "x86_64")]
            Io::Port(p) => unsafe {
                outl(p, off);
                outl(p + 4, v)
            },
        }
    }
}

pub struct E1000 {
    io: Io,
    mac: [u8; 6],
    rx_ring: u64,
    tx_ring: u64,
    rx_buf: u64,
    tx_buf: u64,
    rx_head: usize,
    tx_tail: usize,
    irq: u8,
}

unsafe impl Send for E1000 {}

fn alloc_dma(pages: u64) -> Option<(u64, u64)> {
    let phys = mm::with_frames(|f| f.alloc_contiguous(pages, 1))?;
    let virt = phys.0 + mm::hhdm();
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, (pages * PAGE) as usize) };
    Some((virt, phys.0))
}

impl E1000 {
    fn rx_desc(&self, i: usize) -> *mut u8 {
        (self.rx_ring + 16 * i as u64) as *mut u8
    }

    fn tx_desc(&self, i: usize) -> *mut u8 {
        (self.tx_ring + 16 * i as u64) as *mut u8
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    pub fn irq_line(&self) -> u8 {
        self.irq
    }

    pub fn enable_irq(&self) {
        self.io.wr(IMS, IRQ_RXT0 | IRQ_RXDMT0 | IRQ_TXDW | IRQ_LSC);
    }

    pub fn ack_irq(&self) -> bool {
        self.io.rd(ICR) != 0
    }

    fn rx_ready(&self) -> bool {
        unsafe { read_volatile(self.rx_desc(self.rx_head).add(12)) & DD != 0 }
    }

    fn tx_free(&self) -> bool {
        unsafe { read_volatile(self.tx_desc(self.tx_tail).add(12)) & DD != 0 }
    }
}

pub struct E1000Rx<'a> {
    dev: *mut E1000,
    _p: PhantomData<&'a mut E1000>,
}

pub struct E1000Tx<'a> {
    dev: *mut E1000,
    _p: PhantomData<&'a mut E1000>,
}

impl RxToken for E1000Rx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        let dev = unsafe { &mut *self.dev };
        let i = dev.rx_head;
        let desc = dev.rx_desc(i);
        let len = unsafe { read_volatile(desc.add(8) as *const u16) } as usize;
        let data = dev.rx_buf + (i * BUF) as u64;
        let r = f(unsafe { core::slice::from_raw_parts(data as *const u8, len.min(BUF)) });
        unsafe { write_volatile(desc.add(12), 0) };
        fence(Ordering::SeqCst);
        dev.io.wr(RDT, i as u32);
        dev.rx_head = (i + 1) % RING;
        r
    }
}

impl TxToken for E1000Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let dev = unsafe { &mut *self.dev };
        let i = dev.tx_tail;
        let len = len.min(MTU);
        let data = dev.tx_buf + (i * BUF) as u64;
        let r = f(unsafe { core::slice::from_raw_parts_mut(data as *mut u8, len) });
        let desc = dev.tx_desc(i);
        unsafe {
            write_volatile(desc.add(8) as *mut u16, len as u16);
            write_volatile(desc.add(10), 0);
            write_volatile(desc.add(11), TX_CMD);
            write_volatile(desc.add(12), 0);
        }
        fence(Ordering::SeqCst);
        dev.tx_tail = (i + 1) % RING;
        dev.io.wr(TDT, dev.tx_tail as u32);
        r
    }
}

impl Device for E1000 {
    type RxToken<'a> = E1000Rx<'a>;
    type TxToken<'a> = E1000Tx<'a>;

    fn receive(&mut self, _ts: Instant) -> Option<(E1000Rx<'_>, E1000Tx<'_>)> {
        if !self.rx_ready() || !self.tx_free() {
            return None;
        }
        let dev = self as *mut E1000;
        Some((E1000Rx { dev, _p: PhantomData }, E1000Tx { dev, _p: PhantomData }))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<E1000Tx<'_>> {
        if !self.tx_free() {
            return None;
        }
        Some(E1000Tx { dev: self as *mut E1000, _p: PhantomData })
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

fn map_bar(phys: u64, len: u64) -> Option<usize> {
    crate::arch::port::mmio_map(phys, len).map(|v| v as usize)
}

fn eeprom(io: &Io, word: u32) -> u16 {
    io.wr(EERD, (word << 8) | 1);
    for _ in 0..100_000 {
        let v = io.rd(EERD);
        if v & (1 << 4) != 0 {
            return (v >> 16) as u16;
        }
    }
    0
}

fn bars(b: u32, d: u32, f: u32) -> (Option<usize>, Option<u16>) {
    let mut mem = None;
    let mut port: Option<u16> = None;
    let mut i = 0;
    while i < 6 {
        let bar = cfg_read(b, d, f, 0x10 + 4 * i);
        if bar & 1 == 1 {
            if port.is_none() && bar & !3 != 0 {
                port = Some((bar & !3) as u16);
            }
        } else if mem.is_none() && bar & !0xf != 0 {
            let mut addr = (bar & !0xf) as u64;
            if bar & 6 == 4 {
                addr |= (cfg_read(b, d, f, 0x14 + 4 * i) as u64) << 32;
                i += 1;
            }
            mem = map_bar(addr, 0x2_0000);
        }
        i += 1;
    }
    (mem, port)
}

pub fn probe() -> Option<E1000> {
    let (b, d, f) = find(|b, d, f| {
        let id = cfg_read(b, d, f, 0);
        id & 0xffff == VENDOR && IDS.contains(&(id >> 16))
    })?;
    cfg_write(b, d, f, 4, (cfg_read(b, d, f, 4) & 0xffff) | 0x7);
    let (mem, port) = bars(b, d, f);
    #[cfg(target_arch = "x86_64")]
    let chosen = match (port, mem) {
        (_, Some(m)) => Some(Io::Mem(m)),
        (Some(p), None) => Some(Io::Port(p)),
        _ => None,
    };
    #[cfg(not(target_arch = "x86_64"))]
    let chosen = {
        let _ = port;
        mem.map(Io::Mem)
    };
    let io = match chosen {
        Some(io) => io,
        None => {
            kprintln!("ifsy: net: e1000 {:02x}:{:02x}.{} has no usable BAR {}", b, d, f, warn());
            return None;
        }
    };
    let irq = (cfg_read(b, d, f, 0x3c) & 0xff) as u8;

    io.wr(IMC, 0xffff_ffff);
    io.wr(CTRL, io.rd(CTRL) | CTRL_RST);
    for _ in 0..100_000 {
        if io.rd(CTRL) & CTRL_RST == 0 {
            break;
        }
    }
    io.wr(IMC, 0xffff_ffff);
    io.rd(ICR);
    io.wr(CTRL, (io.rd(CTRL) | CTRL_SLU | CTRL_ASDE) & !(1 << 3) & !(1 << 31));

    let ral = io.rd(RAL0);
    let rah = io.rd(RAH0);
    let mut mac = [ral as u8, (ral >> 8) as u8, (ral >> 16) as u8, (ral >> 24) as u8, rah as u8, (rah >> 8) as u8];
    if mac == [0; 6] {
        for i in 0..3 {
            let w = eeprom(&io, i);
            mac[2 * i as usize] = w as u8;
            mac[2 * i as usize + 1] = (w >> 8) as u8;
        }
        io.wr(RAL0, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
        io.wr(RAH0, u32::from_le_bytes([mac[4], mac[5], 0, 0]) | RAH_AV);
    }
    for i in 0..128 {
        io.wr(MTA + 4 * i, 0);
    }

    let (rx_ring, rx_ring_p) = alloc_dma(1)?;
    let (tx_ring, tx_ring_p) = alloc_dma(1)?;
    let (rx_buf, rx_buf_p) = alloc_dma((RING * BUF) as u64 / PAGE)?;
    let (tx_buf, tx_buf_p) = alloc_dma((RING * BUF) as u64 / PAGE)?;
    let dev = E1000 { io, mac, rx_ring, tx_ring, rx_buf, tx_buf, rx_head: 0, tx_tail: 0, irq };
    unsafe {
        for i in 0..RING {
            write_volatile(dev.rx_desc(i) as *mut u64, rx_buf_p + (i * BUF) as u64);
            write_volatile(dev.tx_desc(i) as *mut u64, tx_buf_p + (i * BUF) as u64);
            write_volatile(dev.tx_desc(i).add(12), DD);
        }
    }
    io.wr(RDBAL, rx_ring_p as u32);
    io.wr(RDBAH, (rx_ring_p >> 32) as u32);
    io.wr(RDLEN, (RING * 16) as u32);
    io.wr(RDH, 0);
    io.wr(RDT, (RING - 1) as u32);
    io.wr(TDBAL, tx_ring_p as u32);
    io.wr(TDBAH, (tx_ring_p >> 32) as u32);
    io.wr(TDLEN, (RING * 16) as u32);
    io.wr(TDH, 0);
    io.wr(TDT, 0);
    io.wr(TIPG, 0x0060_200a);
    io.wr(TCTL, TCTL_EN | TCTL_PSP | (0x10 << 4) | (0x40 << 12));
    io.wr(RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
    let _ = io.rd(STATUS);
    kprintln!(
        "ifsy: net: e1000 at {:02x}:{:02x}.{}, mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} {}",
        b, d, f, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], pass()
    );
    Some(dev)
}
