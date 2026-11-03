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

use super::{PAGE_SIZE, PageFlags, PageTableEntry, PhysAddr, VirtAddr, hw};

pub const LEVELS: usize = 4;
pub const ENTRIES: usize = 512;

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; ENTRIES],
}

pub trait FrameSource {
    fn alloc_table_frame(&mut self) -> Option<PhysAddr>;
    fn free_table_frame(&mut self, frame: PhysAddr);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    Misaligned,
    NonCanonical,
    AlreadyMapped,
    HugePage,
    OutOfFrames,
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmapError {
    Misaligned,
    NonCanonical,
    NotMapped,
    HugePage,
    Corrupt,
}

pub const fn index(level: usize, virt: VirtAddr) -> usize {
    ((virt.0 >> (12 + 9 * (level - 1))) & 0x1ff) as usize
}

pub const fn is_canonical(virt: VirtAddr) -> bool {
    let top = (virt.0 as i64) >> 47;
    top == 0 || top == -1
}

#[derive(Clone, Copy, Debug)]
pub struct Mapper {
    root: PhysAddr,
    hhdm: u64,
    pin_root_slots: bool,
}

impl Mapper {
    pub const unsafe fn new(root: PhysAddr, hhdm: u64) -> Self {
        Self { root, hhdm, pin_root_slots: false }
    }

    pub const fn pinned_root_slots(mut self) -> Self {
        self.pin_root_slots = true;
        self
    }

    pub const fn root(&self) -> PhysAddr {
        self.root
    }

    pub const fn hhdm(&self) -> u64 {
        self.hhdm
    }

    fn table_ptr(&self, phys: PhysAddr) -> *mut PageTable {
        (phys.0 + self.hhdm) as *mut PageTable
    }

    pub fn map<F: FrameSource>(
        &self,
        frames: &mut F,
        virt: VirtAddr,
        phys: PhysAddr,
        flags: PageFlags,
    ) -> Result<(), MapError> {
        if virt.0 % PAGE_SIZE != 0 || phys.0 % PAGE_SIZE != 0 {
            return Err(MapError::Misaligned);
        }
        if !is_canonical(virt) {
            return Err(MapError::NonCanonical);
        }

        let mut created: [Option<(PhysAddr, *mut PageTable, usize)>; LEVELS] = [None; LEVELS];
        let mut table = self.table_ptr(self.root);

        for level in (2..=LEVELS).rev() {
            let idx = index(level, virt);
            let entry = unsafe { &mut (*table).entries[idx] };
            if entry.is_huge(level) {
                self.rollback(frames, &created);
                return Err(MapError::HugePage);
            }
            if entry.is_present() {
                table = self.table_ptr(entry.addr());
            } else if entry.raw() != 0 {
                self.rollback(frames, &created);
                return Err(MapError::Corrupt);
            } else {
                let Some(frame) = frames.alloc_table_frame() else {
                    self.rollback(frames, &created);
                    return Err(MapError::OutOfFrames);
                };
                *entry = PageTableEntry::table(frame);
                created[LEVELS - level] = Some((frame, table, idx));
                table = self.table_ptr(frame);
            }
        }

        let idx = index(1, virt);
        let leaf = unsafe { &mut (*table).entries[idx] };
        if leaf.raw() != 0 {
            self.rollback(frames, &created);
            return Err(MapError::AlreadyMapped);
        }
        leaf.set(phys, flags);
        hw::publish();
        Ok(())
    }

    fn rollback<F: FrameSource>(
        &self,
        frames: &mut F,
        created: &[Option<(PhysAddr, *mut PageTable, usize)>; LEVELS],
    ) {
        for (frame, parent, idx) in created.iter().rev().flatten() {
            unsafe { (*(*parent)).entries[*idx].clear() };
            frames.free_table_frame(*frame);
        }
    }

    pub fn unmap<F: FrameSource>(
        &self,
        frames: &mut F,
        virt: VirtAddr,
    ) -> Result<PhysAddr, UnmapError> {
        if virt.0 % PAGE_SIZE != 0 {
            return Err(UnmapError::Misaligned);
        }
        if !is_canonical(virt) {
            return Err(UnmapError::NonCanonical);
        }

        let mut path: [(*mut PageTable, usize); LEVELS] = [(core::ptr::null_mut(), 0); LEVELS];
        let mut table = self.table_ptr(self.root);
        for level in (2..=LEVELS).rev() {
            let idx = index(level, virt);
            path[LEVELS - level] = (table, idx);
            let entry = unsafe { (*table).entries[idx] };
            if entry.is_huge(level) {
                return Err(UnmapError::HugePage);
            }
            if entry.is_present() {
                table = self.table_ptr(entry.addr());
            } else if entry.raw() != 0 {
                return Err(UnmapError::Corrupt);
            } else {
                return Err(UnmapError::NotMapped);
            }
        }

        let idx = index(1, virt);
        path[LEVELS - 1] = (table, idx);
        let leaf = unsafe { &mut (*table).entries[idx] };
        if !leaf.is_present() {
            return Err(UnmapError::NotMapped);
        }
        let phys = leaf.addr();
        leaf.clear();

        for depth in (1..LEVELS).rev() {
            let (tbl, _) = path[depth];
            let empty = unsafe { (*tbl).entries.iter().all(|e| e.raw() == 0) };
            if !empty {
                break;
            }
            if depth == 1 && (self.pin_root_slots || hw::KERNEL_SHARED_FROM.is_some_and(|s| path[0].1 >= s)) {
                break;
            }
            let (parent, pidx) = path[depth - 1];
            unsafe { (*parent).entries[pidx].clear() };
            frames.free_table_frame(PhysAddr(tbl as u64 - self.hhdm));
        }
        hw::publish();
        Ok(phys)
    }

    pub fn protect(&self, virt: VirtAddr, flags: PageFlags) -> Result<(), UnmapError> {
        if virt.0 % PAGE_SIZE != 0 {
            return Err(UnmapError::Misaligned);
        }
        if !is_canonical(virt) {
            return Err(UnmapError::NonCanonical);
        }
        let mut table = self.table_ptr(self.root);
        for level in (2..=LEVELS).rev() {
            let entry = unsafe { (*table).entries[index(level, virt)] };
            if entry.is_huge(level) {
                return Err(UnmapError::HugePage);
            }
            if !entry.is_present() {
                return Err(if entry.raw() != 0 { UnmapError::Corrupt } else { UnmapError::NotMapped });
            }
            table = self.table_ptr(entry.addr());
        }
        let leaf = unsafe { &mut (*table).entries[index(1, virt)] };
        if !leaf.is_present() {
            return Err(UnmapError::NotMapped);
        }
        let phys = leaf.addr();
        leaf.set(phys, flags);
        hw::publish();
        Ok(())
    }

    pub fn translate(&self, virt: VirtAddr) -> Option<(PhysAddr, PageFlags)> {
        if !is_canonical(virt) {
            return None;
        }
        let mut table = self.table_ptr(self.root);
        for level in (1..=LEVELS).rev() {
            let entry = unsafe { (*table).entries[index(level, virt)] };
            if entry.is_huge(level) {
                let size = 1u64 << (12 + 9 * (level - 1));
                let base = entry.addr().0 & !(size - 1);
                return Some((PhysAddr(base + (virt.0 & (size - 1))), entry.flags()?));
            }
            if !entry.is_present() {
                return None;
            }
            if level == 1 {
                return Some((PhysAddr(entry.addr().0 + (virt.0 & (PAGE_SIZE - 1))), entry.flags()?));
            }
            table = self.table_ptr(entry.addr());
        }
        None
    }

    pub fn for_each_leaf(&self, from: usize, to: usize, f: &mut dyn FnMut(VirtAddr, u64, PageTableEntry)) {
        self.walk(self.root, LEVELS, 0, from, to.min(ENTRIES), f);
    }

    fn walk(
        &self,
        tbl: PhysAddr,
        level: usize,
        base: u64,
        from: usize,
        to: usize,
        f: &mut dyn FnMut(VirtAddr, u64, PageTableEntry),
    ) {
        let table = self.table_ptr(tbl);
        for i in from..to {
            let entry = unsafe { (*table).entries[i] };
            let va = base | ((i as u64) << (12 + 9 * (level - 1)));
            if entry.is_huge(level) || (level == 1 && entry.is_present()) {
                let canon = if va & (1 << 47) != 0 { va | 0xffff_0000_0000_0000 } else { va };
                f(VirtAddr(canon), 1u64 << (12 + 9 * (level - 1)), entry);
            } else if level > 1 && entry.is_present() {
                self.walk(entry.addr(), level - 1, va, 0, ENTRIES, f);
            }
        }
    }

    pub fn enforce_wx(&self, from: usize, to: usize) -> u64 {
        self.fix_wx(self.root, LEVELS, from, to.min(ENTRIES))
    }

    fn fix_wx(&self, tbl: PhysAddr, level: usize, from: usize, to: usize) -> u64 {
        let table = self.table_ptr(tbl);
        let mut fixed = 0;
        for i in from..to {
            let entry = unsafe { &mut (*table).entries[i] };
            if entry.is_huge(level) || (level == 1 && entry.is_present()) {
                if entry.is_present() || entry.is_huge(level) {
                    if entry.flags().is_none() {
                        entry.make_non_exec();
                        fixed += 1;
                    }
                }
            } else if level > 1 && entry.is_present() {
                fixed += self.fix_wx(entry.addr(), level - 1, 0, ENTRIES);
            }
        }
        fixed
    }

    pub fn free_tables<F: FrameSource>(&self, frames: &mut F, limit: usize) {
        let root = self.table_ptr(self.root);
        for i in 0..limit.min(ENTRIES) {
            let entry = unsafe { &mut (*root).entries[i] };
            if !entry.is_huge(LEVELS) && entry.is_present() {
                self.free_subtree(frames, entry.addr(), LEVELS - 1);
                frames.free_table_frame(entry.addr());
            }
            entry.clear();
        }
    }

    fn free_subtree<F: FrameSource>(&self, frames: &mut F, tbl: PhysAddr, level: usize) {
        if level < 2 {
            return;
        }
        let table = self.table_ptr(tbl);
        for i in 0..ENTRIES {
            let entry = unsafe { (*table).entries[i] };
            if !entry.is_huge(level) && entry.is_present() {
                self.free_subtree(frames, entry.addr(), level - 1);
                frames.free_table_frame(entry.addr());
            }
        }
    }
}
