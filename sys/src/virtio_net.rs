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

use crate::console::color::{failed, pass, warn};
use crate::mm;

const PAGE: u64 = 4096;
const SLOTS: usize = 16;
const BUF: usize = 2048;
const BUF_PAGES: u64 = (SLOTS * BUF) as u64 / PAGE;
const MTU: usize = 1514;

const F_MAC: u32 = 1 << 5;
const DESC_WRITE: u16 = 2;
const AVAIL_NO_IRQ: u16 = 1;

const ST_ACK: u32 = 1;
const ST_DRIVER: u32 = 2;
const ST_DRIVER_OK: u32 = 4;
const ST_FEATURES_OK: u32 = 8;
const ST_FAILED: u32 = 128;

#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
mod mmio {
    pub const MAGIC: u64 = 0x000;
    pub const VERSION: u64 = 0x004;
    pub const DEVICE_ID: u64 = 0x008;
    pub const DEVICE_FEATURES: u64 = 0x010;
    pub const DEVICE_FEATURES_SEL: u64 = 0x014;
    pub const DRIVER_FEATURES: u64 = 0x020;
    pub const DRIVER_FEATURES_SEL: u64 = 0x024;
    pub const GUEST_PAGE_SIZE: u64 = 0x028;
    pub const QUEUE_SEL: u64 = 0x030;
    pub const QUEUE_NUM_MAX: u64 = 0x034;
    pub const QUEUE_NUM: u64 = 0x038;
    pub const QUEUE_ALIGN: u64 = 0x03c;
    pub const QUEUE_PFN: u64 = 0x040;
    pub const QUEUE_READY: u64 = 0x044;
    pub const QUEUE_NOTIFY: u64 = 0x050;
    pub const STATUS: u64 = 0x070;
    pub const QUEUE_DESC_LOW: u64 = 0x080;
    pub const QUEUE_DESC_HIGH: u64 = 0x084;
    pub const QUEUE_DRIVER_LOW: u64 = 0x090;
    pub const QUEUE_DRIVER_HIGH: u64 = 0x094;
    pub const QUEUE_DEVICE_LOW: u64 = 0x0a0;
    pub const QUEUE_DEVICE_HIGH: u64 = 0x0a4;
    pub const CONFIG: u64 = 0x100;
    pub const MAGIC_VALUE: u32 = 0x7472_6976;
    pub const ID_NET: u32 = 1;
}

#[cfg(target_arch = "x86_64")]
mod pio {
    pub const HOST_FEATURES: u16 = 0;
    pub const GUEST_FEATURES: u16 = 4;
    pub const QUEUE_PFN: u16 = 8;
    pub const QUEUE_SIZE: u16 = 12;
    pub const QUEUE_SEL: u16 = 14;
    pub const QUEUE_NOTIFY: u16 = 16;
    pub const STATUS: u16 = 18;
    pub const CONFIG: u16 = 20;
    pub const VENDOR: u32 = 0x1af4;
    pub const DEV_NET: u32 = 0x1000;
    pub const DEV_NET_MODERN: u32 = 0x1041;
}

#[derive(Clone, Copy)]
enum Bus {
    #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
    Mmio(u64),
    #[cfg(target_arch = "x86_64")]
    Pio(u16),
}

impl Bus {
    fn notify(&self, queue: u16) {
        match *self {
            #[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Bus::Mmio(r) => unsafe { write_volatile((r + mmio::QUEUE_NOTIFY) as *mut u32, queue as u32) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
            Bus::Pio(p) => unsafe { crate::pci::outw(p + pio::QUEUE_NOTIFY, queue) },
        }
    }
}

struct Queue {
    mem: u64,
    phys: u64,
    size: u16,
    avail_off: u64,
    used_off: u64,
    avail_idx: u16,
    last_used: u16,
}

fn align_page(n: u64) -> u64 {
    n.div_ceil(PAGE) * PAGE
}

fn layout(size: u16) -> (u64, u64, u64) {
    let n = size as u64;
    let avail = 16 * n;
    let used = align_page(avail + 6 + 2 * n);
    (avail, used, used + align_page(6 + 8 * n))
}

fn alloc_pages(pages: u64) -> Option<(u64, u64)> {
    let phys = mm::with_frames(|f| f.alloc_contiguous(pages, 1))?;
    let virt = phys.0 + mm::hhdm();
    // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
    unsafe { core::ptr::write_bytes(virt as *mut u8, 0, (pages * PAGE) as usize) };
    Some((virt, phys.0))
}

impl Queue {
    fn new(size: u16) -> Option<Queue> {
        let (avail_off, used_off, total) = layout(size);
        let (mem, phys) = alloc_pages(total / PAGE)?;
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe { write_volatile((mem + avail_off) as *mut u16, AVAIL_NO_IRQ) };
        Some(Queue { mem, phys, size, avail_off, used_off, avail_idx: 0, last_used: 0 })
    }

    /// # Safety
    /// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
    unsafe fn set_desc(&self, i: usize, addr: u64, len: u32, flags: u16) {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe {
            let d = (self.mem + 16 * i as u64) as *mut u8;
            write_volatile(d as *mut u64, addr);
            write_volatile(d.add(8) as *mut u32, len);
            write_volatile(d.add(12) as *mut u16, flags);
            write_volatile(d.add(14) as *mut u16, 0);
        }
    }

    /// # Safety
    /// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
    unsafe fn push(&mut self, id: u16) {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe {
            let avail = (self.mem + self.avail_off) as *mut u16;
            write_volatile(avail.add(2 + self.avail_idx as usize % self.size as usize), id);
            fence(Ordering::SeqCst);
            self.avail_idx = self.avail_idx.wrapping_add(1);
            write_volatile(avail.add(1), self.avail_idx);
            fence(Ordering::SeqCst);
        }
    }

    /// # Safety
    /// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
    unsafe fn pop(&mut self) -> Option<(u16, u32)> {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe {
            fence(Ordering::SeqCst);
            let used = (self.mem + self.used_off) as *const u8;
            if read_volatile(used.add(2) as *const u16) == self.last_used {
                return None;
            }
            let slot = self.last_used as usize % self.size as usize;
            let id = read_volatile(used.add(4 + slot * 8) as *const u32) as u16;
            let len = read_volatile(used.add(8 + slot * 8) as *const u32);
            self.last_used = self.last_used.wrapping_add(1);
            Some((id, len))
        }
    }
}

pub struct VirtioNet {
    bus: Bus,
    hdr: usize,
    mac: [u8; 6],
    rx: Queue,
    tx: Queue,
    rx_buf: u64,
    tx_buf: u64,
    tx_free: u32,
}

// SAFETY: access to the inner value is serialized by the lock (or the data is only moved once), so sharing across threads cannot race.
unsafe impl Send for VirtioNet {}

impl VirtioNet {
    fn finish(bus: Bus, hdr: usize, mac: [u8; 6], rx: Queue, tx: Queue) -> Option<VirtioNet> {
        let (rx_buf, rx_phys) = alloc_pages(BUF_PAGES)?;
        let (tx_buf, tx_phys) = alloc_pages(BUF_PAGES)?;
        let mut dev = VirtioNet { bus, hdr, mac, rx, tx, rx_buf, tx_buf, tx_free: (1 << SLOTS) - 1 };
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        unsafe {
            for i in 0..SLOTS {
                dev.rx.set_desc(i, rx_phys + (i * BUF) as u64, BUF as u32, DESC_WRITE);
                dev.rx.push(i as u16);
                dev.tx.set_desc(i, tx_phys + (i * BUF) as u64, 0, 0);
            }
        }
        dev.bus.notify(0);
        Some(dev)
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    fn reclaim(&mut self) {
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        while let Some((id, _)) = unsafe { self.tx.pop() } {
            self.tx_free |= 1 << (id as usize % SLOTS);
        }
    }
}

pub struct NetRx<'a> {
    dev: *mut VirtioNet,
    id: u16,
    len: usize,
    _p: PhantomData<&'a mut VirtioNet>,
}

pub struct NetTx<'a> {
    dev: *mut VirtioNet,
    _p: PhantomData<&'a mut VirtioNet>,
}

impl RxToken for NetRx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        let dev = unsafe { &mut *self.dev };
        let base = dev.rx_buf + (self.id as usize % SLOTS * BUF) as u64;
        let frame = self.len.saturating_sub(dev.hdr).min(BUF - dev.hdr);
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        let r = f(unsafe { core::slice::from_raw_parts((base + dev.hdr as u64) as *const u8, frame) });
        unsafe { dev.rx.push(self.id) };
        dev.bus.notify(0);
        r
    }
}

impl TxToken for NetTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        // SAFETY: the pointer is valid for the lifetime of its owner and access is exclusive (&mut self or the owning lock is held).
        let dev = unsafe { &mut *self.dev };
        let len = len.min(BUF - dev.hdr);
        let slot = dev.tx_free.trailing_zeros() as usize;
        if slot >= SLOTS {
            let mut scratch = [0u8; MTU];
            return f(&mut scratch[..len.min(MTU)]);
        }
        dev.tx_free &= !(1 << slot);
        let base = dev.tx_buf + (slot * BUF) as u64;
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        unsafe { core::ptr::write_bytes(base as *mut u8, 0, dev.hdr) };
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        let r = f(unsafe { core::slice::from_raw_parts_mut((base + dev.hdr as u64) as *mut u8, len) });
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe {
            let d = (dev.tx.mem + 16 * slot as u64 + 8) as *mut u32;
            write_volatile(d, (dev.hdr + len) as u32);
            dev.tx.push(slot as u16);
        }
        dev.bus.notify(1);
        r
    }
}

impl Device for VirtioNet {
    type RxToken<'a> = NetRx<'a>;
    type TxToken<'a> = NetTx<'a>;

    fn receive(&mut self, _ts: Instant) -> Option<(NetRx<'_>, NetTx<'_>)> {
        self.reclaim();
        if self.tx_free == 0 {
            return None;
        }
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        let (id, len) = unsafe { self.rx.pop() }?;
        let dev = self as *mut VirtioNet;
        Some((NetRx { dev, id, len: len as usize, _p: PhantomData }, NetTx { dev, _p: PhantomData }))
    }

    fn transmit(&mut self, _ts: Instant) -> Option<NetTx<'_>> {
        self.reclaim();
        if self.tx_free == 0 {
            return None;
        }
        Some(NetTx { dev: self as *mut VirtioNet, _p: PhantomData })
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

#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
pub fn probe() -> Option<VirtioNet> {
    use crate::arch::port::{VIRTIO_MMIO_BASE as BASE, VIRTIO_MMIO_SLOTS as SLOTS_N, VIRTIO_MMIO_STRIDE as STRIDE, mmio_map};
    let va = mmio_map(BASE, SLOTS_N * STRIDE)?;
    for slot in 0..SLOTS_N {
        let regs = va + slot * STRIDE;
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        let hit = unsafe {
            read_volatile((regs + mmio::MAGIC) as *const u32) == mmio::MAGIC_VALUE
                && read_volatile((regs + mmio::DEVICE_ID) as *const u32) == mmio::ID_NET
        };
        if !hit {
            continue;
        }
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        if let Some(d) = unsafe { setup_mmio(regs) } {
            kprintln!(
                "ifsy: net: virtio-net at {:#x}, mac {} {}",
                BASE + slot * STRIDE,
                fmt_mac(&d.mac),
                pass()
            );
            return Some(d);
        }
        kprintln!("ifsy: net: virtio-net at {:#x} would not start {}", BASE + slot * STRIDE, failed());
    }
    None
}

#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn setup_mmio(regs: u64) -> Option<VirtioNet> {
    use mmio::*;
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    let rd = |off: u64| unsafe { read_volatile((regs + off) as *const u32) };
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    let wr = |off: u64, v: u32| unsafe { write_volatile((regs + off) as *mut u32, v) };
    let version = rd(VERSION);
    if version != 1 && version != 2 {
        return None;
    }
    wr(STATUS, 0);
    wr(STATUS, ST_ACK);
    wr(STATUS, ST_ACK | ST_DRIVER);
    wr(DEVICE_FEATURES_SEL, 0);
    let mac_ok = rd(DEVICE_FEATURES) & F_MAC != 0;
    let lo = if mac_ok { F_MAC } else { 0 };
    if version == 2 {
        wr(DEVICE_FEATURES_SEL, 1);
        if rd(DEVICE_FEATURES) & 1 == 0 {
            wr(STATUS, ST_FAILED);
            return None;
        }
        wr(DRIVER_FEATURES_SEL, 0);
        wr(DRIVER_FEATURES, lo);
        wr(DRIVER_FEATURES_SEL, 1);
        wr(DRIVER_FEATURES, 1);
        wr(STATUS, ST_ACK | ST_DRIVER | ST_FEATURES_OK);
        if rd(STATUS) & ST_FEATURES_OK == 0 {
            wr(STATUS, ST_FAILED);
            return None;
        }
    } else {
        wr(DRIVER_FEATURES, lo);
        wr(GUEST_PAGE_SIZE, PAGE as u32);
    }
    let mut queues = [None, None];
    for (q, slot) in queues.iter_mut().enumerate() {
        wr(QUEUE_SEL, q as u32);
        if rd(QUEUE_NUM_MAX) < SLOTS as u32 {
            wr(STATUS, ST_FAILED);
            return None;
        }
        wr(QUEUE_NUM, SLOTS as u32);
        let queue = Queue::new(SLOTS as u16)?;
        if version == 1 {
            wr(QUEUE_ALIGN, PAGE as u32);
            wr(QUEUE_PFN, (queue.phys / PAGE) as u32);
        } else {
            let (d, a, u) = (queue.phys, queue.phys + queue.avail_off, queue.phys + queue.used_off);
            wr(QUEUE_DESC_LOW, d as u32);
            wr(QUEUE_DESC_HIGH, (d >> 32) as u32);
            wr(QUEUE_DRIVER_LOW, a as u32);
            wr(QUEUE_DRIVER_HIGH, (a >> 32) as u32);
            wr(QUEUE_DEVICE_LOW, u as u32);
            wr(QUEUE_DEVICE_HIGH, (u >> 32) as u32);
            wr(QUEUE_READY, 1);
        }
        *slot = Some(queue);
    }
    let mut mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    if mac_ok {
        for (i, b) in mac.iter_mut().enumerate() {
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            *b = unsafe { read_volatile((regs + CONFIG + i as u64) as *const u8) };
        }
    }
    let [Some(rx), Some(tx)] = queues else { return None };
    let hdr = if version == 2 { 12 } else { 10 };
    let dev = VirtioNet::finish(Bus::Mmio(regs), hdr, mac, rx, tx)?;
    let ok = if version == 2 { ST_ACK | ST_DRIVER | ST_FEATURES_OK | ST_DRIVER_OK } else { ST_ACK | ST_DRIVER | ST_DRIVER_OK };
    wr(STATUS, ok);
    dev.bus.notify(0);
    Some(dev)
}

#[cfg(target_arch = "x86_64")]
pub fn probe() -> Option<VirtioNet> {
    use crate::pci::{cfg_read, cfg_write, find_id, inb, inl, outb, outl, outw, inw};
    if find_id(pio::VENDOR, pio::DEV_NET_MODERN).is_some() && find_id(pio::VENDOR, pio::DEV_NET).is_none() {
        kprintln!("ifsy: net: only a modern (non-transitional) virtio-net is present; use disable-modern=on {}", warn());
        return None;
    }
    let (b, d, f) = find_id(pio::VENDOR, pio::DEV_NET)?;
    cfg_write(b, d, f, 4, (cfg_read(b, d, f, 4) & 0xffff) | 0x5);
    let bar0 = cfg_read(b, d, f, 0x10);
    if bar0 & 1 == 0 {
        kprintln!("ifsy: net: virtio-net {:02x}:{:02x}.{} has no I/O port BAR {}", b, d, f, warn());
        return None;
    }
    let base = (bar0 & !3) as u16;
    // SAFETY: x86 port I/O to a fixed legacy/PCI port; it has no memory effects, and the kernel runs at CPL 0.
    unsafe {
        outb(base + pio::STATUS, 0);
        outb(base + pio::STATUS, ST_ACK as u8);
        outb(base + pio::STATUS, (ST_ACK | ST_DRIVER) as u8);
        let mac_ok = inl(base + pio::HOST_FEATURES) & F_MAC != 0;
        outl(base + pio::GUEST_FEATURES, if mac_ok { F_MAC } else { 0 });
        let mut queues = [None, None];
        for (q, slot) in queues.iter_mut().enumerate() {
            outw(base + pio::QUEUE_SEL, q as u16);
            let size = inw(base + pio::QUEUE_SIZE);
            if size == 0 || size > 1024 || !size.is_power_of_two() {
                outb(base + pio::STATUS, ST_FAILED as u8);
                return None;
            }
            let queue = Queue::new(size)?;
            outl(base + pio::QUEUE_PFN, (queue.phys / PAGE) as u32);
            *slot = Some(queue);
        }
        let mut mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        if mac_ok {
            for (i, m) in mac.iter_mut().enumerate() {
                *m = inb(base + pio::CONFIG + i as u16);
            }
        }
        let [Some(rx), Some(tx)] = queues else { return None };
        let dev = VirtioNet::finish(Bus::Pio(base), 10, mac, rx, tx)?;
        outb(base + pio::STATUS, (ST_ACK | ST_DRIVER | ST_DRIVER_OK) as u8);
        dev.bus.notify(0);
        kprintln!("ifsy: net: virtio-net at {:02x}:{:02x}.{}, mac {} {}", b, d, f, fmt_mac(&dev.mac), pass());
        Some(dev)
    }
}

fn fmt_mac(m: &[u8; 6]) -> alloc::string::String {
    alloc::format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5])
}
