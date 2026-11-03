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

#![allow(dead_code, unused_imports)]

pub mod aspace;
pub mod frame;
pub mod heap;
pub mod hw;
pub mod paging;
pub mod rng;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use limine::memory_map::{Entry, EntryType};

use crate::sync::SpinLock;
use frame::{FrameAllocator, InitError, Range};
use paging::FrameSource;

pub use crate::arch::port::mm::PageTableEntry;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageFlags {
    writable: bool,
    executable: bool,
    user_accessible: bool,
    global: bool,
    uncached: bool,
}

impl PageFlags {
    pub const fn new(writable: bool, executable: bool, user_accessible: bool, global: bool) -> Self {
        assert!(
            !(writable && executable),
            "W^X violation: a page must not be both writable and executable"
        );
        Self { writable, executable, user_accessible, global, uncached: false }
    }

    pub const MMIO: Self = Self { writable: true, executable: false, user_accessible: false, global: false, uncached: true };

    pub const KERNEL_RO: Self = Self::new(false, false, false, false);
    pub const KERNEL_RW: Self = Self::new(true, false, false, false);
    pub const KERNEL_EXEC: Self = Self::new(false, true, false, false);
    pub const USER_RO: Self = Self::new(false, false, true, false);
    pub const USER_RW: Self = Self::new(true, false, true, false);
    pub const USER_EXEC: Self = Self::new(false, true, true, false);

    pub const fn writable(&self) -> bool {
        self.writable
    }

    pub const fn executable(&self) -> bool {
        self.executable
    }

    pub const fn user_accessible(&self) -> bool {
        self.user_accessible
    }

    pub const fn global(&self) -> bool {
        self.global
    }

    pub const fn uncached(&self) -> bool {
        self.uncached
    }
}

pub const PAGE_SIZE: u64 = 4096;
const MIB: u64 = 1024 * 1024;

const HEAP_FLOOR: u64 = MIB;
const HEAP_MIN: u64 = MIB;
pub const HEAP_MAX: u64 = 64 * MIB;

pub fn select_heap_region(entries: &[&Entry]) -> Option<(u64, u64)> {
    let mut best: Option<(u64, u64)> = None;

    for entry in entries {
        if entry.entry_type != EntryType::USABLE {
            continue;
        }
        let Some(end) = entry.base.checked_add(entry.length) else { continue };
        let Some(start) = entry.base.max(HEAP_FLOOR).checked_add(PAGE_SIZE - 1) else { continue };

        let start = start & !(PAGE_SIZE - 1);
        let end = end & !(PAGE_SIZE - 1);
        if end <= start {
            continue;
        }

        let len = end - start;
        if best.map_or(true, |(_, best_len)| len > best_len) {
            best = Some((start, len));
        }
    }

    let (base, len) = best?;
    if len < HEAP_MIN {
        return None;
    }
    Some((base, len.min(HEAP_MAX)))
}

pub unsafe fn init_heap(entries: &[&Entry], hhdm_offset: u64) -> Option<usize> {
    let (base, len) = select_heap_region(entries)?;
    HEAP_BASE.store(base, Ordering::Relaxed);
    HEAP_LEN.store(len, Ordering::Relaxed);
    let virt = base.checked_add(hhdm_offset)?;
    let len = usize::try_from(len).ok()?;
    unsafe { heap::init(virt as *mut u8, len) };
    Some(len)
}

static HEAP_BASE: AtomicU64 = AtomicU64::new(0);
static HEAP_LEN: AtomicU64 = AtomicU64::new(0);
static HHDM: AtomicU64 = AtomicU64::new(0);
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);
static MAX_PHYS: AtomicU64 = AtomicU64::new(0);
static PAGING_READY: AtomicBool = AtomicBool::new(false);
static FRAMES: SpinLock<FrameAllocator> = SpinLock::new(FrameAllocator::empty());
static BOOT_RNG: SpinLock<Option<rng::Rng>> = SpinLock::new(None);

pub fn hhdm() -> u64 {
    HHDM.load(Ordering::Relaxed)
}

pub fn kernel_root() -> PhysAddr {
    PhysAddr(KERNEL_ROOT.load(Ordering::Relaxed))
}

pub fn paging_ready() -> bool {
    PAGING_READY.load(Ordering::Acquire)
}

#[derive(Clone, Copy, Default)]
pub struct GlobalFrames;

impl GlobalFrames {
    pub fn alloc(&mut self) -> Option<PhysAddr> {
        FRAMES.with(|f| f.alloc())
    }
    pub fn alloc_zeroed(&mut self) -> Option<PhysAddr> {
        let h = hhdm();
        FRAMES.with(|f| f.alloc_zeroed(h))
    }
    pub fn free(&mut self, p: PhysAddr) -> bool {
        FRAMES.with(|f| f.free(p))
    }
}

impl FrameSource for GlobalFrames {
    fn alloc_table_frame(&mut self) -> Option<PhysAddr> {
        self.alloc_zeroed()
    }
    fn free_table_frame(&mut self, frame: PhysAddr) {
        self.free(frame);
    }
}

pub fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator) -> R) -> R {
    FRAMES.with(f)
}

pub fn frame_stats() -> (u64, u64) {
    FRAMES.with(|f| (f.total_frames(), f.free_frames()))
}

pub unsafe fn init_frames(entries: &[&Entry], hhdm_offset: u64) -> Result<Range, InitError> {
    HHDM.store(hhdm_offset, Ordering::Relaxed);
    let mut usable = [Range { base: 0, len: 0 }; 128];
    let mut n = 0;
    let mut top = 0u64;
    for e in entries {
        if e.entry_type == EntryType::USABLE && n < usable.len() {
            usable[n] = Range { base: e.base, len: e.length };
            n += 1;
            top = top.max(e.base.saturating_add(e.length));
        }
    }
    MAX_PHYS.store(top, Ordering::Relaxed);
    let heap = [Range { base: HEAP_BASE.load(Ordering::Relaxed), len: HEAP_LEN.load(Ordering::Relaxed) }];
    FRAMES.with(|f| unsafe { f.init(&usable[..n], &heap, hhdm_offset) })
}

pub fn init_paging() -> Result<(u64, bool), aspace::AsError> {
    if !hw::translation_ok() {
        return Err(aspace::AsError::Unsupported);
    }
    KERNEL_ROOT.store(hw::kernel_root().0, Ordering::Relaxed);
    let mut rng = rng::Rng::from_entropy();
    let hw_seeded = rng.hardware_seeded;
    let kvm_rng = rng.fork();
    let lo = aspace::kvm::init(kvm_rng, hhdm(), MAX_PHYS.load(Ordering::Relaxed))?;
    BOOT_RNG.with(|r| *r = Some(rng));
    PAGING_READY.store(true, Ordering::Release);
    Ok((lo, hw_seeded))
}

pub fn fork_rng() -> Option<rng::Rng> {
    BOOT_RNG.with(|r| r.as_mut().map(|r| r.fork()))
}

pub fn new_address_space() -> Result<aspace::AddressSpace, aspace::AsError> {
    let mut rng = fork_rng().ok_or(aspace::AsError::Unsupported)?;
    aspace::AddressSpace::new_user(&mut rng)
}
