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

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

use linked_list_allocator::LockedHeap;

static HEAP: LockedHeap = LockedHeap::empty();

struct IrqSafeHeap;

unsafe impl GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let irq = crate::arch::irq_save();
        let p = unsafe { HEAP.alloc(layout) };
        crate::arch::irq_restore(irq);
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let irq = crate::arch::irq_save();
        unsafe { HEAP.dealloc(ptr, layout) };
        crate::arch::irq_restore(irq);
    }
}

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap;

static INITIALISED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug)]
pub struct HeapStats {
    pub size: usize,
    pub used: usize,
    pub free: usize,
}

pub unsafe fn init(start: *mut u8, size: usize) {
    if INITIALISED.swap(true, Ordering::AcqRel) {
        panic!("heap: init called twice");
    }
    let irq = crate::arch::irq_save();
    unsafe { HEAP.lock().init(start, size) };
    crate::arch::irq_restore(irq);
}

pub fn stats() -> HeapStats {
    let irq = crate::arch::irq_save();
    let stats = {
        let heap = HEAP.lock();
        HeapStats { size: heap.size(), used: heap.used(), free: heap.free() }
    };
    crate::arch::irq_restore(irq);
    stats
}
