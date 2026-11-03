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

#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use limine::BaseRevision;
use limine::request::{
    ExecutableAddressRequest, FramebufferRequest, HhdmRequest, MemoryMapRequest, ModuleRequest, RequestsEndMarker,
    RequestsStartMarker,
};

mod arch;
#[macro_use]
mod console;
#[cfg(target_arch = "x86_64")]
mod ahci;
mod pci;
mod abi;
mod block;
mod context;
mod elf;
mod ext4grow;
mod fs;
mod gpt;
mod hardening;
mod io;
mod mnt;
mod net;
mod netfs;
mod mntfs;
mod ns;
mod p9;
mod p9srv;
mod procfs;
mod rtc;
mod klog;
mod mm;
mod note;
mod nstest;
mod sched;
mod store;
mod seg;
mod sync;
mod syscall;
mod sysmnt;
mod timer;
mod uctx;
mod userland;
mod usermode;
#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
mod virtio_blk;
mod virtio_net;
mod nic;
mod e1000;
mod rtl8139;
#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
mod virtio_input;

use crate::console::color::{failed, green, pass, red, warn};

#[used]
#[unsafe(link_section = ".requests")]
static BASE_REVISION: BaseRevision = BaseRevision::with_revision(6);

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
pub(crate) static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static KERNEL_ADDR_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MODULE_REQUEST: ModuleRequest = ModuleRequest::new();

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

#[unsafe(no_mangle)]
unsafe extern "C" fn kmain() -> ! {
    unsafe { hardening::early_init() };

    kprintln!("ifsy: boot: kmain entered");

    assert!(BASE_REVISION.is_supported());
    kprintln!("ifsy: boot: limine base revision supported {}", pass());

    if let Some(framebuffer_response) = FRAMEBUFFER_REQUEST.get_response() {
        if let Some(framebuffer) = framebuffer_response.framebuffers().next() {
            let width = framebuffer.width() as usize;
            let height = framebuffer.height() as usize;
            let pitch = framebuffer.pitch() as usize;
            let bpp = framebuffer.bpp() as usize;
            let bytes_per_pixel = bpp / 8;

            unsafe {
                console::attach_framebuffer(framebuffer.addr(), width, height, pitch, bytes_per_pixel)
            };
            kprintln!(
                "ifsy: boot: framebuffer {}x{} @ {}bpp (pitch={}) attached to console {}",
                width,
                height,
                bpp,
                pitch,
                pass()
            );
        } else {
            kprintln!("ifsy: boot: framebuffer response had no framebuffers; console is serial-only {}", warn());
        }
    } else {
        kprintln!("ifsy: boot: no framebuffer response from bootloader; console is serial-only {}", warn());
    }

    init_heap();
    heap_selftest();
    init_memory();
    klog::init();
    sched::init();
    sched::selftest();
    timer::init();
    timer::selftest();
    timer::irq_selftest();
    abi::selftest();
    if timer::irq_active() {
        timer::resync();
        sched::switch_selftest();
    }
    if let Some(disk) = probe_boot_disk() {
        mount_root(disk);
        abi::exec_selftest();
        nstest::selftest();
        if !(timer::irq_active() && usermode::selftest()) {
            attach_persistent_root();
        }
    }

    if net::init() {
        net::selftest();
        netfs::selftest();
    }

    if timer::irq_active() {
        kprintln!("ifsy: boot: setup complete, idling on the timer interrupt {}", pass());
        timer::resync();
        idle_loop();
    }
    kprintln!("ifsy: boot: setup complete, halting {}", pass());
    arch::halt();
}

fn idle_loop() -> ! {
    loop {
        net::poll();
        if timer::take_resched() {
            let _ = sched::schedule();
        }
        timer::idle();
    }
}

fn init_heap() {
    let (Some(hhdm), Some(memmap)) = (HHDM_REQUEST.get_response(), MEMMAP_REQUEST.get_response()) else {
        kprintln!("ifsy: heap: {}", red("bootloader gave no HHDM / memory map; cannot continue"));
        arch::halt();
    };

    match unsafe { mm::init_heap(memmap.entries(), hhdm.offset()) } {
        Some(size) => kprintln!("ifsy: heap: {} KiB online (HHDM offset {:#x})", size / 1024, hhdm.offset()),
        None => {
            kprintln!("ifsy: heap: {}", red("no usable memory region large enough; cannot continue"));
            arch::halt();
        }
    }
}

fn heap_selftest() {
    use alloc::{boxed::Box, vec::Vec};

    let before = mm::heap::stats();
    let sum = {
        let boxed = Box::new(0x00C0_FFEEu64);
        let mut v: Vec<u32> = Vec::new();
        for i in 0..4096u32 {
            v.push(i);
        }
        let during = mm::heap::stats();
        kprintln!("ifsy: heap: self-test holding {} bytes", during.used - before.used);
        v.iter().map(|&x| u64::from(x)).sum::<u64>() + *boxed
    };
    let after = mm::heap::stats();

    let ok = sum == 8_386_560 + 0x00C0_FFEE && after.used == before.used;
    kprintln!(
        "ifsy: heap: self-test {} (used {} -> {} bytes, {} free)",
        if ok { green("PASS") } else { red("FAIL") },
        before.used,
        after.used,
        after.free
    );
}

fn probe_boot_disk() -> Option<block::Disk<block::RamDisk>> {
    use block::{BlockDevice, Disk, RamDisk, SEEK_END, SEEK_SET};

    let Some(module) = MODULE_REQUEST.get_response().and_then(|r| r.modules().first().copied()) else {
        kprintln!("ifsy: block: no boot module loaded (add `module_path:` to limine.conf) {}", warn());
        return None;
    };

    let ram = unsafe { RamDisk::from_raw(module.addr() as *mut u8, module.size() as usize) };
    let mut disk = Disk::new(ram);
    let sectors = disk.device().sector_count();
    kprintln!("ifsy: block: ramdisk online, {} sectors ({} KiB) {}", sectors, disk.size() / 1024, pass());

    let mut magic = [0u8; 2];
    let read_ok = disk.seek(1024 + 56, SEEK_SET).is_ok() && disk.read(&mut magic) == Ok(2);
    if !read_ok {
        kprintln!("ifsy: block: could not read superblock area (disk too small?) {}", failed());
    } else if u16::from_le_bytes(magic) == 0xEF53 {
        kprintln!("ifsy: block: ext4 superblock magic 0xEF53 found {}", pass());
    } else {
        kprintln!("ifsy: block: no ext4 magic (read {:#06x}) {}", u16::from_le_bytes(magic), failed());
    }

    if disk.size() >= 4096 {
        const PATTERN: [u8; 5] = [0xDE, 0xAD, 0xBE, 0xEF, 0x55];
        let mut saved = [0u8; 5];
        let mut back = [0u8; 5];

        let ok = (|| -> Result<bool, i32> {
            disk.seek(-7, SEEK_END)?;
            disk.read(&mut saved)?;
            disk.seek(-7, SEEK_END)?;
            disk.write(&PATTERN)?;
            disk.seek(-7, SEEK_END)?;
            disk.read(&mut back)?;
            let matched = back == PATTERN;
            disk.seek(-7, SEEK_END)?;
            disk.write(&saved)?;
            disk.flush()?;
            Ok(matched)
        })();
        match ok {
            Ok(true) => kprintln!("ifsy: block: write self-test {}", pass()),
            Ok(false) => kprintln!("ifsy: block: write self-test (readback mismatch) {}", failed()),
            Err(_) => kprintln!("ifsy: block: write self-test (I/O error) {}", failed()),
        }
    }

    Some(disk)
}

pub(crate) fn attach_persistent_root() {
    let Some(disk) = store::attach() else { return };
    match fs::switch_root(disk) {
        Ok(()) => {
            kprintln!("ifsy: fs: persistent ext4 root mounted at / {}", pass());
            userland::install();
        }
        Err(e) => {
            kprintln!("ifsy: fs: could not mount the root from disk (errno {}); staying live {}", e, failed());
            store::detach();
        }
    }
}

fn mount_root(disk: block::Disk<block::RamDisk>) {
    match fs::mount_root(disk) {
        Ok(()) => {
            kprintln!("ifsy: fs: ext4 root mounted at / {}", pass());
            fs::selftest();
            userland::install();
        }
        Err(e) => kprintln!("ifsy: fs: could not mount ext4 root (errno {}) {}", e, failed()),
    }
}

#[panic_handler]
fn rust_panic(info: &core::panic::PanicInfo) -> ! {
#[cfg(feature = "verbose-boot")]
    kprintln!("ifsy: PANIC: {}", info);
    arch::halt();
}

fn init_memory() {
    use mm::frame::InitError;
    let (Some(hhdm), Some(memmap)) = (HHDM_REQUEST.get_response(), MEMMAP_REQUEST.get_response()) else { return };

    match unsafe { mm::init_frames(memmap.entries(), hhdm.offset()) } {
        Ok(bm) => {
            let (total, free) = mm::frame_stats();
            kprintln!(
                "ifsy: frames: {} frames tracked, {} free ({} MiB), bitmap {} KiB {}",
                total, free, free * 4096 / (1024 * 1024), bm.len / 1024, pass()
            );
        }
        Err(e) => {
            let why = if e == InitError::NoMemory { "no usable memory" } else { "no room for bitmap" };
            kprintln!("ifsy: frames: {} {}", why, failed());
            return;
        }
    }
    frame_selftest();

    match mm::init_paging() {
        Ok((lo, hw)) => {
            kprintln!("ifsy: paging: kernel VM window at {:#x} {}", lo, pass());
            if hw {
                kprintln!("ifsy: kaslr: boot RNG seeded from hardware {}", pass());
            } else {
                kprintln!("ifsy: kaslr: no hardware RNG, seeded from timer jitter only {}", warn());
            }
        }
        Err(e) => {
            kprintln!("ifsy: paging: unsupported translation regime ({:?}); address spaces disabled {}", e, warn());
            return;
        }
    }
    if let Some(k) = KERNEL_ADDR_REQUEST.get_response() {
        kprintln!("ifsy: kaslr: kernel image virt {:#x} phys {:#x}", k.virtual_base(), k.physical_base());
    }
    wx_audit(hhdm.offset());
    aspace_selftest();
}

fn frame_selftest() {
    let before = mm::frame_stats().1;
    let ok = mm::with_frames(|f| {
        let (Some(a), Some(b)) = (f.alloc(), f.alloc()) else { return false };
        let dbl = f.free(a) && !f.free(a);
        let foreign = !f.free(mm::PhysAddr(0x1000));
        let Some(c) = f.alloc_contiguous(16, 16) else { return false };
        let aligned = c.0 % (16 * 4096) == 0;
        let freed = f.free_contiguous(c, 16) && f.free(b);
        dbl && foreign && aligned && freed
    });
    let after = mm::frame_stats().1;
    kprintln!(
        "ifsy: frames: self-test {} (free {} -> {})",
        if ok && before == after { green("PASS") } else { red("FAIL") },
        before, after
    );
}

fn wx_audit(hhdm: u64) {
    use mm::paging::Mapper;
    let m = unsafe { Mapper::new(mm::kernel_root(), hhdm) };
    let fixed = m.enforce_wx(mm::hw::KERNEL_SHARED_FROM.unwrap_or(0), 512);
    if fixed != 0 {
        mm::hw::flush_all();
        kprintln!("ifsy: paging: bootloader left {} W+X leaves in the kernel half; made non-executable {}", fixed, warn());
    }
    let (mut leaves, mut bad) = (0u64, 0u64);
    let from = mm::hw::KERNEL_SHARED_FROM.unwrap_or(0);
    m.for_each_leaf(from, 512, &mut |_, _, e| {
        leaves += 1;
        if e.flags().is_none() {
            bad += 1;
        }
    });
    if bad == 0 {
        kprintln!("ifsy: paging: W^X audit of kernel mappings: {} leaves, 0 violations {}", leaves, pass());
    } else {
        kprintln!("ifsy: paging: W^X audit: {} of {} leaves are W+X or malformed {}", bad, leaves, failed());
    }
}

fn aspace_selftest() {
    use mm::PageFlags;
    use mm::aspace::{AsError, Kind, Layout, kvm};

    let bits = Layout::entropy_bits();
    kprintln!(
        "ifsy: kaslr: layout entropy bits: image {} heap {} mmap {} stack {}",
        bits[0], bits[1], bits[2], bits[3]
    );
    let free0 = mm::frame_stats().1;

    let (Ok(mut a), Ok(b)) = (mm::new_address_space(), mm::new_address_space()) else {
        kprintln!("ifsy: aspace: could not create address spaces {}", failed());
        return;
    };
    let (la, lb) = (a.layout(), b.layout());
    kprintln!("ifsy: aspace: A image {:#x} heap {:#x} mmap {:#x} stack {:#x}", la.image_base, la.heap_base, la.mmap_top, la.stack_top);
    kprintln!("ifsy: aspace: B image {:#x} heap {:#x} mmap {:#x} stack {:#x}", lb.image_base, lb.heap_base, lb.mmap_top, lb.stack_top);
    let differ = la.image_base != lb.image_base && la.stack_top != lb.stack_top && la.mmap_top != lb.mmap_top;
    kprintln!("ifsy: kaslr: per-process layouts differ {}", if differ { pass() } else { failed() });

    let mut ok = true;
    let mut check = |name: &str, cond: bool| {
        if !cond {
            kprintln!("ifsy: aspace: check failed: {} {}", name, failed());
            ok = false;
        }
    };

    let base = a.map_image(3 * 4096).unwrap_or(0);
    check("image at randomised base", base == la.image_base);
    let msg = b"ifsy";
    check("write_bytes spanning pages", a.write_bytes(base + 4096 - 3, msg).is_ok());
    let mut back = [0u8; 4];
    check("read_bytes", a.read_bytes(base + 4096 - 3, &mut back).is_ok() && &back == msg);
    check("text RX", a.protect(base, 4096, PageFlags::USER_EXEC).is_ok());
    check("flags updated", a.translate(base).is_some_and(|(_, f)| f.executable() && !f.writable()));
    check("partial unmap splits region", a.unmap(base + 4096, 4096).is_ok() && a.region_count() == 2);
    check("hole unmapped", a.translate(base + 4096).is_none() && a.translate(base + 2 * 4096).is_some());
    check("overlap rejected", a.map_region(base, 4096, PageFlags::USER_RW, Kind::Anon) == Err(AsError::Overlap));
    check("null page rejected", a.map_region(0, 4096, PageFlags::USER_RW, Kind::Anon) == Err(AsError::OutOfRange));

    let h0 = a.brk(0).unwrap_or(0);
    check("brk starts at heap base", h0 == la.heap_base);
    check("brk grow", a.brk(h0 + 5 * 4096) == Ok(h0 + 5 * 4096));
    check("brk shrink", a.brk(h0 + 4096) == Ok(h0 + 4096) && a.translate(h0 + 2 * 4096).is_none());

    let sp = a.map_stack(mm::aspace::STACK_DEFAULT).unwrap_or(0);
    check("stack top randomised", sp == la.stack_top);
    check("stack guard unmapped", a.translate(sp - mm::aspace::STACK_DEFAULT - 4096).is_none());
    let m1 = a.map_anywhere(8 * 4096, PageFlags::USER_RW, Kind::Anon).unwrap_or(0);
    let m2 = a.map_anywhere(8 * 4096, PageFlags::USER_RW, Kind::Anon).unwrap_or(0);
    check("mmap top-down, non-adjacent", m1 != 0 && m2 != 0 && m2 + 8 * 4096 < m1);

    let s1 = kvm::alloc_stack(4);
    let s2 = kvm::alloc_stack(4);
    check("kvm stacks", matches!((s1, s2), (Some(x), Some(y)) if x != y));
    if let Some(top) = s1 {
        let low = top - 4 * 4096;
        check("kvm guard below", kvm::translate(low - 4096).is_none());
        unsafe { ((top - 8) as *mut u64).write_volatile(0xC0FFEE) };
        check("kvm stack writable", unsafe { ((top - 8) as *const u64).read_volatile() } == 0xC0FFEE);
        check("kvm free", kvm::free(low));
    }
    if let Some(top) = s2 {
        kvm::free(top - 4 * 4096);
    }

    #[cfg(not(target_arch = "aarch64"))]
    {
        let kroot = mm::hw::read_root();
        unsafe { a.activate() };
        let in_a = mm::hw::read_root() == a.root();
        let mut bb = b;
        unsafe { bb.activate() };
        let in_b = mm::hw::read_root() == bb.root();
        unsafe { mm::hw::switch_root(kroot, 0) };
        check("root switch A->B->kernel", in_a && in_b && mm::hw::read_root() == kroot);
        drop(bb);
    }
    drop(a);
    #[cfg(target_arch = "aarch64")]
    drop(b);

    let free1 = mm::frame_stats().1;
    check("all frames returned on drop", free1 == free0);
    kprintln!(
        "ifsy: aspace: self-test {} (free frames {} -> {})",
        if ok { green("PASS") } else { red("FAIL") }, free0, free1
    );
}
