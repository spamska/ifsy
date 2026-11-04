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

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::frame::FrameAllocator;
use super::hw::{self, KERNEL_SHARED_FROM, USER_VA_TOP};
use super::paging::{ENTRIES, MapError, Mapper, UnmapError};
use super::rng::Rng;
use super::{PAGE_SIZE, PageFlags, PhysAddr, VirtAddr, GlobalFrames};
use crate::sync::SpinLock;

pub const USER_MIN: u64 = 1024 * 1024;
const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;

pub const IMAGE_LO: u64 = 4 * 1024 * 1024;
const IMAGE_SPAN: u64 = 256 * GIB;
pub const IMAGE_MAX: u64 = GIB;
const HEAP_GAP_SPAN: u64 = GIB;
pub const HEAP_CEIL: u64 = 32 * TIB;
const MMAP_FLOOR: u64 = 32 * TIB;
const MMAP_TOP_LO: u64 = 64 * TIB;
const MMAP_TOP_SPAN: u64 = 32 * TIB;
const STACK_TOP_SPAN: u64 = 4 * GIB;
pub const STACK_MAX: u64 = GIB;
pub const STACK_DEFAULT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsError {
    Misaligned,
    OutOfRange,
    Overlap,
    NoMemory,
    NotMapped,
    Unsupported,
    Denied,
    Map(MapError),
}

impl From<MapError> for AsError {
    fn from(e: MapError) -> Self {
        if e == MapError::OutOfFrames { AsError::NoMemory } else { AsError::Map(e) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Heap,
    Stack,
    Anon,
    Phys,
    Shared,
}

static SHARED: SpinLock<BTreeMap<u64, (u64, usize)>> = SpinLock::new(BTreeMap::new());

pub fn shared_new(pages: u64) -> Option<PhysAddr> {
    if pages == 0 {
        return None;
    }
    let h = super::hhdm();
    let base = super::with_frames(|f| f.alloc_contiguous(pages, 1))?;
    // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
    unsafe { core::ptr::write_bytes((base.0 + h) as *mut u8, 0, (pages * PAGE_SIZE) as usize) };
    SHARED.with(|t| t.insert(base.0, (pages, 0)));
    Some(base)
}

pub fn shared_ref(base: PhysAddr) -> bool {
    SHARED.with(|t| match t.get_mut(&base.0) {
        Some(e) => {
            e.1 += 1;
            true
        }
        None => false,
    })
}

pub fn shared_unref(base: PhysAddr) {
    let gone = SHARED.with(|t| match t.get_mut(&base.0) {
        Some(e) if e.1 > 1 => {
            e.1 -= 1;
            None
        }
        Some(_) => t.remove(&base.0).map(|e| e.0),
        None => None,
    });
    if let Some(pages) = gone {
        super::with_frames(|f| f.free_contiguous(base, pages));
    }
}

pub fn shared_refs(base: PhysAddr) -> usize {
    SHARED.with(|t| t.get(&base.0).map_or(0, |e| e.1))
}

#[derive(Clone, Copy, Debug)]
struct Region {
    len: u64,
    flags: PageFlags,
    kind: Kind,
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub image_base: u64,
    pub heap_base: u64,
    pub mmap_top: u64,
    pub stack_top: u64,
}

impl Layout {
    pub fn randomize(rng: &mut Rng) -> Self {
        let image_base = IMAGE_LO + rng.aligned_below(IMAGE_SPAN, PAGE_SIZE);
        let heap_base = image_base + IMAGE_MAX + rng.aligned_below(HEAP_GAP_SPAN, PAGE_SIZE);
        let mmap_top = MMAP_TOP_LO + rng.aligned_below(MMAP_TOP_SPAN, PAGE_SIZE);
        let stack_top = USER_VA_TOP - PAGE_SIZE - rng.aligned_below(STACK_TOP_SPAN - PAGE_SIZE, PAGE_SIZE);
        Self { image_base, heap_base, mmap_top, stack_top }
    }

    pub const fn entropy_bits() -> [u32; 4] {
        [
            (IMAGE_SPAN / PAGE_SIZE).ilog2(),
            (HEAP_GAP_SPAN / PAGE_SIZE).ilog2(),
            (MMAP_TOP_SPAN / PAGE_SIZE).ilog2(),
            ((STACK_TOP_SPAN - PAGE_SIZE) / PAGE_SIZE).ilog2(),
        ]
    }
}

struct AsidPool {
    next: u32,
    generation: u64,
}
static ASIDS: SpinLock<AsidPool> = SpinLock::new(AsidPool { next: 1, generation: 1 });

fn asid_alloc() -> (u16, u64) {
    if hw::ASID_BITS == 0 {
        return (0, 1);
    }
    ASIDS.with(|p| {
        let max = (1u32 << hw::ASID_BITS) - 1;
        if p.next > max {
            p.next = 1;
            p.generation += 1;
            hw::flush_all();
        }
        let a = p.next;
        p.next += 1;
        (a as u16, p.generation)
    })
}

fn asid_generation() -> u64 {
    ASIDS.with(|p| p.generation)
}

pub struct AddressSpace {
    mapper: Mapper,
    regions: BTreeMap<u64, Region>,
    layout: Layout,
    rng: Rng,
    mmap_cursor: u64,
    brk: u64,
    asid: u16,
    asid_gen: u64,
    attached: BTreeMap<u64, (u64, u64)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Kernel,
    Read,
    Write,
}

fn page_align_up(v: u64) -> Option<u64> {
    v.checked_add(PAGE_SIZE - 1).map(|x| x & !(PAGE_SIZE - 1))
}

impl AddressSpace {
    pub fn new_user(boot_rng: &mut Rng) -> Result<Self, AsError> {
        if !super::paging_ready() {
            return Err(AsError::Unsupported);
        }
        let mut rng = boot_rng.fork();
        let layout = Layout::randomize(&mut rng);
        Self::build(layout, rng)
    }

    fn build(layout: Layout, rng: Rng) -> Result<Self, AsError> {
        let hhdm = super::hhdm();
        let root = GlobalFrames.alloc_zeroed().ok_or(AsError::NoMemory)?;
        if let Some(from) = KERNEL_SHARED_FROM {
            let src = (super::kernel_root().0 + hhdm) as *const u64;
            let dst = (root.0 + hhdm) as *mut u64;
            for i in from..ENTRIES {
                // SAFETY: i < ENTRIES, so both pointers stay inside their 4 KiB root-table frames (HHDM-mapped, distinct frames).
                unsafe { dst.add(i).write(src.add(i).read()) };
            }
        }
        let (asid, asid_gen) = asid_alloc();
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        let mapper = unsafe { Mapper::new(root, hhdm) };
        Ok(Self {
            mapper,
            regions: BTreeMap::new(),
            layout,
            mmap_cursor: layout.mmap_top,
            brk: layout.heap_base,
            rng,
            asid,
            asid_gen,
            attached: BTreeMap::new(),
        })
    }

    pub fn duplicate(&self) -> Result<Self, AsError> {
        let rng = super::fork_rng().ok_or(AsError::Unsupported)?;
        let mut child = Self::build(self.layout, rng)?;
        child.mmap_cursor = self.mmap_cursor;
        child.brk = self.brk;
        let hhdm = self.mapper.hhdm();
        for (&start, r) in &self.regions {
            if r.kind == Kind::Shared {
                continue;
            }
            if r.kind == Kind::Phys {
                let (pa, _) = self.translate(start).ok_or(AsError::NotMapped)?;
                child.map_phys(start, pa, r.len, r.flags)?;
                continue;
            }
            child.map_region(start, r.len, r.flags, r.kind)?;
            let mut va = start;
            while va < start + r.len {
                let (src, _) = self.translate(va).ok_or(AsError::NotMapped)?;
                let (dst, _) = child.translate(va).ok_or(AsError::NotMapped)?;
                // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (src.0 + hhdm) as *const u8,
                        (dst.0 + hhdm) as *mut u8,
                        PAGE_SIZE as usize,
                    )
                };
                va += PAGE_SIZE;
            }
        }
        for (&va, &(phys, len)) in &self.attached {
            let flags = self.regions.get(&va).map_or(PageFlags::USER_RW, |r| r.flags);
            child.map_shared(va, PhysAddr(phys), len, flags)?;
        }
        Ok(child)
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }
    pub fn root(&self) -> PhysAddr {
        self.mapper.root()
    }
    pub fn brk_end(&self) -> u64 {
        self.brk
    }
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    fn overlaps(&self, start: u64, len: u64) -> bool {
        let end = start + len;
        self.regions.range(..end).next_back().is_some_and(|(s, r)| s + r.len > start)
    }

    fn check(start: u64, len: u64) -> Result<u64, AsError> {
        if start % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 || len == 0 {
            return Err(AsError::Misaligned);
        }
        let end = start.checked_add(len).ok_or(AsError::OutOfRange)?;
        if start < USER_MIN || end > USER_VA_TOP {
            return Err(AsError::OutOfRange);
        }
        Ok(end)
    }

    pub fn map_region(&mut self, start: u64, len: u64, flags: PageFlags, kind: Kind) -> Result<(), AsError> {
        let end = Self::check(start, len)?;
        if self.overlaps(start, len) {
            return Err(AsError::Overlap);
        }
        let mut frames = GlobalFrames;
        let mut va = start;
        while va < end {
            let res = match frames.alloc_zeroed() {
                Some(f) => match self.mapper.map(&mut frames, VirtAddr(va), f, flags) {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        frames.free(f);
                        Err(AsError::from(e))
                    }
                },
                None => Err(AsError::NoMemory),
            };
            if let Err(e) = res {
                self.unmap_pages(start, va, true);
                return Err(e);
            }
            va += PAGE_SIZE;
        }
        self.regions.insert(start, Region { len, flags, kind });
        Ok(())
    }

    pub fn map_phys(&mut self, start: u64, phys: PhysAddr, len: u64, flags: PageFlags) -> Result<(), AsError> {
        let end = Self::check(start, len)?;
        if self.overlaps(start, len) {
            return Err(AsError::Overlap);
        }
        let mut frames = GlobalFrames;
        let mut va = start;
        while va < end {
            if let Err(e) = self.mapper.map(&mut frames, VirtAddr(va), PhysAddr(phys.0 + (va - start)), flags) {
                self.unmap_pages(start, va, false);
                return Err(e.into());
            }
            va += PAGE_SIZE;
        }
        self.regions.insert(start, Region { len, flags, kind: Kind::Phys });
        Ok(())
    }

    fn unmap_pages(&mut self, from: u64, to: u64, free_frames: bool) {
        let mut frames = GlobalFrames;
        let mut va = from;
        while va < to {
            if let Ok(pa) = self.mapper.unmap(&mut frames, VirtAddr(va)) {
                if free_frames {
                    frames.free(pa);
                }
                hw::flush_page(VirtAddr(va));
            }
            va += PAGE_SIZE;
        }
    }

    fn split_at(&mut self, addr: u64) {
        let Some((&s, &r)) = self.regions.range(..addr).next_back() else { return };
        if s + r.len > addr {
            self.regions.insert(s, Region { len: addr - s, ..r });
            self.regions.insert(addr, Region { len: s + r.len - addr, ..r });
        }
    }

    fn keys_in(&self, start: u64, end: u64) -> Vec<u64> {
        self.regions.range(start..end).map(|(k, _)| *k).collect()
    }

    pub fn unmap(&mut self, start: u64, len: u64) -> Result<(), AsError> {
        let end = Self::check(start, len)?;
        self.split_at(start);
        self.split_at(end);
        for k in self.keys_in(start, end) {
            let r = self.regions.remove(&k).unwrap();
            self.unmap_pages(k, k + r.len, !matches!(r.kind, Kind::Phys | Kind::Shared));
        }
        Ok(())
    }

    pub fn map_shared(&mut self, start: u64, phys: PhysAddr, len: u64, flags: PageFlags) -> Result<(), AsError> {
        self.map_phys(start, phys, len, flags)?;
        if let Some(r) = self.regions.get_mut(&start) {
            r.kind = Kind::Shared;
        }
        shared_ref(phys);
        self.attached.insert(start, (phys.0, len));
        Ok(())
    }

    pub fn map_shared_anywhere(&mut self, phys: PhysAddr, len: u64, flags: PageFlags) -> Result<u64, AsError> {
        if len == 0 || len % PAGE_SIZE != 0 {
            return Err(AsError::Misaligned);
        }
        let gap = (self.rng.below(256) + 1) * PAGE_SIZE;
        let addr = self
            .mmap_cursor
            .checked_sub(len)
            .and_then(|a| a.checked_sub(gap))
            .filter(|a| *a >= MMAP_FLOOR)
            .ok_or(AsError::NoMemory)?;
        self.map_shared(addr, phys, len, flags)?;
        self.mmap_cursor = addr;
        Ok(addr)
    }

    pub fn detach_shared(&mut self, va: u64) -> Result<(), AsError> {
        let (phys, len) = self.attached.remove(&va).ok_or(AsError::NotMapped)?;
        self.unmap(va, len)?;
        shared_unref(PhysAddr(phys));
        Ok(())
    }

    pub fn attachment_at(&self, va: u64) -> Option<(u64, u64, u64)> {
        let (&base, &(phys, len)) = self.attached.range(..=va).next_back()?;
        (va < base + len).then_some((base, phys, len))
    }

    pub fn region_at(&self, va: u64) -> Option<(u64, u64, Kind)> {
        let (&s, r) = self.regions.range(..=va).next_back()?;
        (va < s + r.len).then_some((s, r.len, r.kind))
    }

    pub fn is_free(&self, start: u64, len: u64) -> bool {
        Self::check(start, len).is_ok() && !self.overlaps(start, len)
    }

    pub fn protect(&mut self, start: u64, len: u64, flags: PageFlags) -> Result<(), AsError> {
        let end = Self::check(start, len)?;
        self.split_at(start);
        self.split_at(end);
        for k in self.keys_in(start, end) {
            let r = self.regions[&k];
            let mut va = k;
            while va < k + r.len {
                self.mapper.protect(VirtAddr(va), flags).map_err(|e| match e {
                    UnmapError::NotMapped => AsError::NotMapped,
                    _ => AsError::Unsupported,
                })?;
                hw::flush_page(VirtAddr(va));
                va += PAGE_SIZE;
            }
            self.regions.get_mut(&k).unwrap().flags = flags;
        }
        Ok(())
    }

    pub fn map_anywhere(&mut self, len: u64, flags: PageFlags, kind: Kind) -> Result<u64, AsError> {
        if len == 0 || len % PAGE_SIZE != 0 {
            return Err(AsError::Misaligned);
        }
        let gap = (self.rng.below(256) + 1) * PAGE_SIZE;
        let addr = self
            .mmap_cursor
            .checked_sub(len)
            .and_then(|a| a.checked_sub(gap))
            .filter(|a| *a >= MMAP_FLOOR)
            .ok_or(AsError::NoMemory)?;
        self.map_region(addr, len, flags, kind)?;
        self.mmap_cursor = addr;
        Ok(addr)
    }

    pub fn map_stack(&mut self, size: u64) -> Result<u64, AsError> {
        if size == 0 || size > STACK_MAX {
            return Err(AsError::OutOfRange);
        }
        let size = page_align_up(size).ok_or(AsError::OutOfRange)?;
        let top = self.layout.stack_top;
        self.map_region(top - size, size, PageFlags::USER_RW, Kind::Stack)?;
        Ok(top)
    }

    pub fn map_image(&mut self, len: u64) -> Result<u64, AsError> {
        let len = page_align_up(len).ok_or(AsError::OutOfRange)?;
        if len == 0 || len > IMAGE_MAX {
            return Err(AsError::OutOfRange);
        }
        let base = self.layout.image_base;
        self.map_region(base, len, PageFlags::USER_RW, Kind::Image)?;
        Ok(base)
    }

    pub fn brk(&mut self, new_end: u64) -> Result<u64, AsError> {
        if new_end == 0 {
            return Ok(self.brk);
        }
        let new_end = page_align_up(new_end).ok_or(AsError::OutOfRange)?;
        if new_end < self.layout.heap_base || new_end > HEAP_CEIL {
            return Err(AsError::OutOfRange);
        }
        if new_end > self.brk {
            self.map_region(self.brk, new_end - self.brk, PageFlags::USER_RW, Kind::Heap)?;
        } else if new_end < self.brk {
            self.unmap(new_end, self.brk - new_end)?;
        }
        self.brk = new_end;
        Ok(new_end)
    }

    pub fn translate(&self, va: u64) -> Option<(PhysAddr, PageFlags)> {
        self.mapper.translate(VirtAddr(va))
    }

    pub fn write_bytes(&self, va: u64, data: &[u8]) -> Result<(), AsError> {
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        self.copy(va, data.len(), Need::Kernel, |dst, off, n| unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(off), dst, n)
        })
    }

    pub fn read_bytes(&self, va: u64, out: &mut [u8]) -> Result<(), AsError> {
        let p = out.as_mut_ptr();
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        self.copy(va, out.len(), Need::Kernel, |src, off, n| unsafe {
            core::ptr::copy_nonoverlapping(src, p.add(off), n)
        })
    }

    pub fn user_read(&self, va: u64, out: &mut [u8]) -> Result<(), AsError> {
        let p = out.as_mut_ptr();
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        self.copy(va, out.len(), Need::Read, |src, off, n| unsafe {
            core::ptr::copy_nonoverlapping(src, p.add(off), n)
        })
    }
    pub fn user_write(&self, va: u64, data: &[u8]) -> Result<(), AsError> {
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        self.copy(va, data.len(), Need::Write, |dst, off, n| unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr().add(off), dst, n)
        })
    }

    pub fn user_read_cstr(&self, va: u64, max: usize) -> Result<Vec<u8>, AsError> {
        let mut out = Vec::new();
        let mut cur = va;
        while out.len() < max {
            let chunk = ((PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize).min(max - out.len());
            let mut buf = alloc::vec![0u8; chunk];
            self.user_read(cur, &mut buf)?;
            if let Some(p) = buf.iter().position(|&b| b == 0) {
                out.extend_from_slice(&buf[..p]);
                return Ok(out);
            }
            out.extend_from_slice(&buf);
            cur += chunk as u64;
        }
        Ok(out)
    }

    fn copy(&self, va: u64, len: usize, need: Need, mut f: impl FnMut(*mut u8, usize, usize)) -> Result<(), AsError> {
        let mut done = 0usize;
        while done < len {
            let cur = va.checked_add(done as u64).ok_or(AsError::OutOfRange)?;
            if cur >= USER_VA_TOP {
                return Err(AsError::OutOfRange);
            }
            let (pa, fl) = self.translate(cur).ok_or(AsError::NotMapped)?;
            let allowed = match need {
                Need::Kernel => true,
                Need::Read => fl.user_accessible(),
                Need::Write => fl.user_accessible() && fl.writable(),
            };
            if !allowed {
                return Err(AsError::Denied);
            }
            let n = ((PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize).min(len - done);
            f((pa.0 + self.mapper.hhdm()) as *mut u8, done, n);
            done += n;
        }
        Ok(())
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    pub unsafe fn activate(&mut self) {
        if self.asid_gen != asid_generation() {
            let (a, g) = asid_alloc();
            self.asid = a;
            self.asid_gen = g;
        }
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { hw::switch_root(self.mapper.root(), self.asid) };
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        if KERNEL_SHARED_FROM.is_some() && hw::read_root() == self.mapper.root() {
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            unsafe { hw::switch_root(super::kernel_root(), 0) };
        }
        let mut frames = GlobalFrames;
        let regions = core::mem::take(&mut self.regions);
        let attached = core::mem::take(&mut self.attached);
        for (k, r) in regions {
            self.unmap_pages(k, k + r.len, !matches!(r.kind, Kind::Phys | Kind::Shared));
        }
        for (_, (phys, _)) in attached {
            shared_unref(PhysAddr(phys));
        }
        self.mapper.free_tables(&mut frames, KERNEL_SHARED_FROM.unwrap_or(ENTRIES));
        frames.free(self.mapper.root());
        hw::flush_all();
    }
}

pub mod kvm {
    use super::*;
    const WINDOWS: [u64; 2] = [0xffff_e000_0000_0000, 0xffff_c000_0000_0000];
    const WINDOW_LEN: u64 = 16 * TIB;
    const TRIES: usize = 64;

    struct Kvm {
        mapper: Mapper,
        regions: BTreeMap<u64, Region>,
        rng: Rng,
        lo: u64,
    }
    static KVM: SpinLock<Option<Kvm>> = SpinLock::new(None);

    fn root_index(va: u64) -> usize {
        ((va >> 39) & 0x1ff) as usize
    }

    pub fn init(mut rng: Rng, hhdm: u64, max_phys: u64) -> Result<u64, AsError> {
        let mut frames = GlobalFrames;
        let hh_lo = root_index(hhdm);
        let hh_hi = root_index(hhdm.saturating_add(max_phys));
        let lo = WINDOWS
            .into_iter()
            .find(|w| {
                let (a, b) = (root_index(*w), root_index(*w + WINDOW_LEN - 1));
                b < hh_lo || a > hh_hi
            })
            .ok_or(AsError::Unsupported)?;
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        let mapper = unsafe { Mapper::new(super::super::kernel_root(), hhdm) }.pinned_root_slots();
        let root = (mapper.root().0 + hhdm) as *mut u64;
        for i in root_index(lo)..=root_index(lo + WINDOW_LEN - 1) {
            // SAFETY: the C struct is plain old data for which the all-zero bit pattern is valid.
            unsafe {
                if root.add(i).read() == 0 {
                    let t = frames.alloc_zeroed().ok_or(AsError::NoMemory)?;
                    root.add(i).write(super::super::PageTableEntry::table(t).raw());
                }
            }
        }
        hw::publish();
        let _ = rng.next_u64();
        KVM.with(|k| *k = Some(Kvm { mapper, regions: BTreeMap::new(), rng, lo }));
        Ok(lo)
    }

    pub fn alloc(pages: u64, flags: PageFlags) -> Option<u64> {
        KVM.with(|k| {
            let k = k.as_mut()?;
            let len = pages.checked_mul(PAGE_SIZE)?;
            let span = len + 2 * PAGE_SIZE;
            for _ in 0..TRIES {
                let base = k.lo + k.rng.aligned_below(WINDOW_LEN - span, PAGE_SIZE) + PAGE_SIZE;
                let end = base + len;
                let hit = k.regions.range(..end + 2 * PAGE_SIZE).next_back().is_some_and(|(s, r)| s + r.len + 2 * PAGE_SIZE > base);
                if hit {
                    continue;
                }
                let mut frames = GlobalFrames;
                let mut va = base;
                while va < end {
                    let ok = frames.alloc_zeroed().is_some_and(|f| {
                        k.mapper.map(&mut frames, VirtAddr(va), f, flags).is_ok() || {
                            frames.free(f);
                            false
                        }
                    });
                    if !ok {
                        let mut u = base;
                        while u < va {
                            if let Ok(pa) = k.mapper.unmap(&mut frames, VirtAddr(u)) {
                                frames.free(pa);
                                hw::flush_page(VirtAddr(u));
                            }
                            u += PAGE_SIZE;
                        }
                        return None;
                    }
                    va += PAGE_SIZE;
                }
                k.regions.insert(base, Region { len, flags, kind: Kind::Anon });
                return Some(base);
            }
            None
        })
    }

    pub fn map_phys(phys: PhysAddr, pages: u64, flags: PageFlags) -> Option<u64> {
        KVM.with(|k| {
            let k = k.as_mut()?;
            let len = pages.checked_mul(PAGE_SIZE)?;
            let span = len + 2 * PAGE_SIZE;
            for _ in 0..TRIES {
                let base = k.lo + k.rng.aligned_below(WINDOW_LEN - span, PAGE_SIZE) + PAGE_SIZE;
                let end = base + len;
                let hit = k.regions.range(..end + 2 * PAGE_SIZE).next_back().is_some_and(|(s, r)| s + r.len + 2 * PAGE_SIZE > base);
                if hit {
                    continue;
                }
                let mut frames = GlobalFrames;
                let mut off = 0;
                while off < len {
                    if k.mapper.map(&mut frames, VirtAddr(base + off), PhysAddr(phys.0 + off), flags).is_err() {
                        let mut u = 0;
                        while u < off {
                            let _ = k.mapper.unmap(&mut frames, VirtAddr(base + u));
                            hw::flush_page(VirtAddr(base + u));
                            u += PAGE_SIZE;
                        }
                        return None;
                    }
                    hw::flush_page(VirtAddr(base + off));
                    off += PAGE_SIZE;
                }
                k.regions.insert(base, Region { len, flags, kind: Kind::Phys });
                return Some(base);
            }
            None
        })
    }

    pub fn alloc_stack(pages: u64) -> Option<u64> {
        alloc(pages, PageFlags::new(true, false, false, true)).map(|b| b + pages * PAGE_SIZE)
    }

    pub fn free(base: u64) -> bool {
        KVM.with(|k| {
            let Some(k) = k.as_mut() else { return false };
            let Some(r) = k.regions.remove(&base) else { return false };
            let mut frames = GlobalFrames;
            let mut va = base;
            while va < base + r.len {
                if let Ok(pa) = k.mapper.unmap(&mut frames, VirtAddr(va)) {
                    frames.free(pa);
                    hw::flush_page(VirtAddr(va));
                }
                va += PAGE_SIZE;
            }
            true
        })
    }

    pub fn translate(va: u64) -> Option<PhysAddr> {
        KVM.with(|k| k.as_ref()?.mapper.translate(VirtAddr(va)).map(|(p, _)| p))
    }
}
