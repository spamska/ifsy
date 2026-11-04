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

use super::{PAGE_SIZE, PhysAddr};

pub const FRAME_FLOOR: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub base: u64,
    pub len: u64,
}

impl Range {
    pub const fn end(&self) -> u64 {
        self.base + self.len
    }
}

pub fn clip_usable(base: u64, len: u64, floor: u64) -> Option<Range> {
    let end = base.checked_add(len)? & !(PAGE_SIZE - 1);
    let start = base.max(floor).checked_add(PAGE_SIZE - 1)? & !(PAGE_SIZE - 1);
    (end > start).then(|| Range { base: start, len: end - start })
}

pub fn subtract(r: Range, hole: Range) -> [Option<Range>; 2] {
    if hole.len == 0 || hole.end() <= r.base || hole.base >= r.end() {
        return [Some(r), None];
    }
    let lo = (hole.base > r.base).then(|| Range { base: r.base, len: hole.base - r.base });
    let hi = (hole.end() < r.end()).then(|| Range { base: hole.end(), len: r.end() - hole.end() });
    [lo, hi]
}

pub struct FrameAllocator {
    bitmap: *mut u64,
    managed: *mut u64,
    words: usize,
    frames: u64,
    free: u64,
    hint: usize,
}
// SAFETY: access to the inner value is serialized by the lock (or the data is only moved once), so sharing across threads cannot race.
unsafe impl Send for FrameAllocator {}

#[derive(Debug, PartialEq, Eq)]
pub enum InitError {
    NoMemory,
    NoRoomForBitmap,
}

impl FrameAllocator {
    pub const fn empty() -> Self {
        Self { bitmap: core::ptr::null_mut(), managed: core::ptr::null_mut(), words: 0, frames: 0, free: 0, hint: 0 }
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    pub unsafe fn init(
        &mut self,
        usable: &[Range],
        reserved: &[Range],
        hhdm: u64,
    ) -> Result<Range, InitError> {
        let mut top = 0u64;
        for u in usable {
            if let Some(r) = clip_usable(u.base, u.len, FRAME_FLOOR) {
                top = top.max(r.end());
            }
        }
        if top == 0 {
            return Err(InitError::NoMemory);
        }
        let frames = top / PAGE_SIZE;
        let words = frames.div_ceil(64) as usize;
        let bitmap_bytes = ((words as u64) * 16).div_ceil(PAGE_SIZE) * PAGE_SIZE;

        let mut pieces = [None::<Range>; MAX_PIECES];
        let mut n = 0usize;
        for u in usable {
            if let Some(r) = clip_usable(u.base, u.len, FRAME_FLOOR) {
                if n < MAX_PIECES {
                    pieces[n] = Some(r);
                    n += 1;
                }
            }
        }
        for hole in reserved {
            let mut next = [None::<Range>; MAX_PIECES];
            let mut k = 0usize;
            for piece in pieces[..n].iter().flatten() {
                for sub in subtract(*piece, *hole).into_iter().flatten() {
                    if k < MAX_PIECES {
                        next[k] = Some(sub);
                        k += 1;
                    }
                }
            }
            pieces = next;
            n = k;
        }

        let bm = pieces[..n]
            .iter()
            .flatten()
            .find(|p| p.len >= bitmap_bytes)
            .map(|p| Range { base: p.base, len: bitmap_bytes })
            .ok_or(InitError::NoRoomForBitmap)?;

        let ptr = (bm.base + hhdm) as *mut u64;
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        unsafe {
            core::ptr::write_bytes(ptr, 0xFF, words);
            core::ptr::write_bytes(ptr.add(words), 0x00, words);
        }
        self.bitmap = ptr;
        // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
        // SAFETY: the allocation behind ptr holds 2*words u64s (bitmap then managed), so ptr+words is in bounds.
        self.managed = unsafe { ptr.add(words) };
        self.words = words;
        self.frames = frames;
        self.free = 0;
        self.hint = 0;

        for p in pieces[..n].iter().flatten() {
            let mut a = p.base;
            while a < p.end() {
                let f = a / PAGE_SIZE;
                self.clear(f);
                let (w, b) = Self::word_bit(f);
                // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
                unsafe { *self.managed.add(w) |= b };
                a += PAGE_SIZE;
            }
        }
        let mut a = bm.base;
        while a < bm.end() {
            let f = a / PAGE_SIZE;
            self.set(f);
            let (w, b) = Self::word_bit(f);
            // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
            unsafe { *self.managed.add(w) &= !b };
            a += PAGE_SIZE;
        }
        Ok(bm)
    }

    fn word_bit(frame: u64) -> (usize, u64) {
        ((frame / 64) as usize, 1u64 << (frame % 64))
    }

    fn is_set(&self, frame: u64) -> bool {
        if frame >= self.frames {
            // Out-of-range frames are never allocatable; report them as in use.
            return true;
        }
        let (w, b) = Self::word_bit(frame);
        // SAFETY: frame < self.frames, so w < self.words, the length of the bitmap allocated in init.
        unsafe { *self.bitmap.add(w) & b != 0 }
    }

    fn set(&mut self, frame: u64) {
        if frame >= self.frames {
            return;
        }
        if !self.is_set(frame) {
            self.free -= 1;
        }
        let (w, b) = Self::word_bit(frame);
        // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
        unsafe { *self.bitmap.add(w) |= b };
    }

    fn clear(&mut self, frame: u64) {
        if frame >= self.frames {
            return;
        }
        if self.is_set(frame) {
            self.free += 1;
        }
        let (w, b) = Self::word_bit(frame);
        // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
        unsafe { *self.bitmap.add(w) &= !b };
    }

    pub fn total_frames(&self) -> u64 {
        if self.managed.is_null() {
            return 0;
        }
        let mut n = 0u64;
        for w in 0..self.words {
            // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
            n += unsafe { *self.managed.add(w) }.count_ones() as u64;
        }
        n
    }

    pub fn free_frames(&self) -> u64 {
        self.free
    }

    pub fn alloc(&mut self) -> Option<PhysAddr> {
        if self.free == 0 {
            return None;
        }
        for i in 0..self.words {
            let w = (self.hint + i) % self.words;
            // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
            let bits = unsafe { *self.bitmap.add(w) };
            if bits != u64::MAX {
                let bit = (!bits).trailing_zeros() as u64;
                let frame = (w as u64) * 64 + bit;
                if frame < self.frames {
                    self.set(frame);
                    self.hint = w;
                    return Some(PhysAddr(frame * PAGE_SIZE));
                }
            }
        }
        None
    }

    pub fn alloc_contiguous(&mut self, count: u64, align: u64) -> Option<PhysAddr> {
        if count == 0 || !align.is_power_of_two() || self.free < count {
            return None;
        }
        let mut start = 0u64;
        while start.checked_add(count)? <= self.frames {
            let mut used_at = None;
            for f in start..start + count {
                if self.is_set(f) {
                    used_at = Some(f);
                    break;
                }
            }
            match used_at {
                Some(f) => start = (f + 1).next_multiple_of(align),
                None => {
                    for f in start..start + count {
                        self.set(f);
                    }
                    return Some(PhysAddr(start * PAGE_SIZE));
                }
            }
        }
        None
    }

    pub fn free_contiguous(&mut self, addr: PhysAddr, count: u64) -> bool {
        if addr.0 % PAGE_SIZE != 0 || count == 0 {
            return false;
        }
        let first = addr.0 / PAGE_SIZE;
        let Some(last) = first.checked_add(count) else { return false };
        if last > self.frames {
            return false;
        }
        for f in first..last {
            let (w, b) = Self::word_bit(f);
            // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
            if !self.is_set(f) || unsafe { *self.managed.add(w) } & b == 0 {
                return false;
            }
        }
        for f in first..last {
            self.clear(f);
        }
        self.hint = (first / 64) as usize;
        true
    }

    pub fn alloc_zeroed(&mut self, hhdm: u64) -> Option<PhysAddr> {
        let f = self.alloc()?;
        // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
        unsafe { core::ptr::write_bytes((f.0 + hhdm) as *mut u8, 0, PAGE_SIZE as usize) };
        Some(f)
    }

    pub fn free(&mut self, addr: PhysAddr) -> bool {
        if addr.0 % PAGE_SIZE != 0 {
            return false;
        }
        let frame = addr.0 / PAGE_SIZE;
        if frame >= self.frames || !self.is_set(frame) {
            return false;
        }
        let (w, b) = Self::word_bit(frame);
        // SAFETY: w is a word index below self.words (derived from a frame number checked against self.frames, or the loop bound), within the bitmap/managed arrays allocated in init; &mut self or the frame lock gives exclusive access.
        if unsafe { *self.managed.add(w) } & b == 0 {
            return false;
        }
        self.clear(frame);
        self.hint = (frame / 64) as usize;
        true
    }
}

const MAX_PIECES: usize = 128;
