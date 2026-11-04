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

#![allow(dead_code)]

use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

const MSR_EFER: u32 = 0xC000_0080;
const EFER_NXE: u64 = 1 << 11;
const CR0_WP: u64 = 1 << 16;
const CR4_SMEP: u64 = 1 << 20;
const CR4_SMAP: u64 = 1 << 21;
pub const RFLAGS_AC: u64 = 1 << 18;
const AC_CLEAR_MASK: i32 = !(1i32 << 18);

static SMEP_ON: AtomicBool = AtomicBool::new(false);
static SMAP_ON: AtomicBool = AtomicBool::new(false);

pub fn smep_enabled() -> bool {
    SMEP_ON.load(Ordering::Relaxed)
}

pub fn smap_enabled() -> bool {
    SMAP_ON.load(Ordering::Relaxed)
}

#[inline]
pub fn clac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("clac", options(nomem, nostack)) };
    }
}

#[inline]
pub fn stac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("stac", options(nomem, nostack)) };
    }
}

pub struct UserAccess(());

impl UserAccess {
    /// # Safety
    /// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
    pub unsafe fn begin() -> Self {
        stac();
        UserAccess(())
    }
}

impl Drop for UserAccess {
    fn drop(&mut self) {
        clac();
    }
}

pub fn read_rflags() -> u64 {
    let f: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("pushfq", "pop {}", out(reg) f, options(preserves_flags)) };
    f
}

pub fn cr4() -> u64 {
    // SAFETY: architecture-specific privileged or MMIO access at kernel privilege, performed in the init order established by kmain.
    unsafe { read_cr4() }
}

fn clear_ac() {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "pushfq",
            "and qword ptr [rsp], {mask}",
            "popfq",
            mask = const AC_CLEAR_MASK,
        );
    }
}

fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    // The previous hand-written asm saved and restored only the low 32 bits of rbx
    // (`xchg r32, ebx` zero-extends), which clobbered the upper half of any pointer the
    // optimiser kept live in rbx. The intrinsic saves and restores all of rbx.
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    #[allow(unused_unsafe)]
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    let r = unsafe { core::arch::x86_64::__cpuid_count(leaf, subleaf) };
    (r.eax, r.ebx, r.ecx, r.edx)
}

fn has_nx() -> bool {
    let (max_extended, ..) = cpuid(0x8000_0000, 0);
    if max_extended < 0x8000_0001 {
        return false;
    }
    let (_, _, _, edx) = cpuid(0x8000_0001, 0);
    edx & (1 << 20) != 0
}

fn has_smep_smap() -> (bool, bool) {
    let (max_basic, ..) = cpuid(0, 0);
    if max_basic < 7 {
        return (false, false);
    }
    let (_, ebx, _, _) = cpuid(7, 0);
    (ebx & (1 << 7) != 0, ebx & (1 << 20) != 0)
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn read_msr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn write_msr(msr: u32, value: u64) {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn read_cr0() -> u64 {
    let v: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mov {}, cr0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn write_cr0(v: u64) {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mov cr0, {}", in(reg) v, options(nostack, preserves_flags)) };
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn read_cr4() -> u64 {
    let v: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mov {}, cr4", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn write_cr4(v: u64) {
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mov cr4, {}", in(reg) v, options(nostack, preserves_flags)) };
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn init() {
    if has_nx() {
        // SAFETY: architecture-specific privileged or MMIO access at kernel privilege, performed in the init order established by kmain.
        unsafe {
            let efer = read_msr(MSR_EFER);
            write_msr(MSR_EFER, efer | EFER_NXE);
        }
        crate::kprintln!("ifsy: hardening: x86_64: NX enabled (EFER.NXE) {}", crate::console::color::pass());
    } else {
        crate::kprintln!("ifsy: hardening: x86_64: NX NOT supported by this CPU -- page-table NX bits will be ignored {}", crate::console::color::warn());
    }

    // SAFETY: architecture-specific privileged or MMIO access at kernel privilege, performed in the init order established by kmain.
    unsafe {
        let cr0 = read_cr0();
        write_cr0(cr0 | CR0_WP);
    }
    crate::kprintln!("ifsy: hardening: x86_64: supervisor write-protect enabled (CR0.WP) {}", crate::console::color::pass());

    let (smep, smap) = has_smep_smap();
    let mut cr4_set = 0u64;
    if smep {
        cr4_set |= CR4_SMEP;
    }
    if smap {
        cr4_set |= CR4_SMAP;
    }
    if cr4_set != 0 {
        // SAFETY: architecture-specific privileged or MMIO access at kernel privilege, performed in the init order established by kmain.
        unsafe {
            let cr4 = read_cr4();
            write_cr4(cr4 | cr4_set);
        }
    }
    // SAFETY: architecture-specific privileged or MMIO access at kernel privilege, performed in the init order established by kmain.
    let cr4 = unsafe { read_cr4() };
    let smep_on = smep && cr4 & CR4_SMEP != 0;
    let smap_on = smap && cr4 & CR4_SMAP != 0;
    SMEP_ON.store(smep_on, Ordering::Relaxed);
    SMAP_ON.store(smap_on, Ordering::Relaxed);

    clear_ac();
    let ac_clear = read_rflags() & RFLAGS_AC == 0;

    crate::kprintln!(
        "ifsy: hardening: x86_64: SMEP {} SMAP {} (CR4={:#x}, RFLAGS.AC {}) {}",
        if smep_on { "enabled" } else if smep { "FAILED to enable" } else { "NOT supported" },
        if smap_on { "enabled" } else if smap { "FAILED to enable" } else { "NOT supported" },
        cr4,
        if ac_clear { "clear" } else { "SET" },
        if smep_on && smap_on && ac_clear { crate::console::color::pass() } else { crate::console::color::warn() },
    );
}
