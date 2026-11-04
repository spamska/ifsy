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

use core::arch::asm;
use core::ptr::addr_of_mut;

use super::traps;
use crate::arch::Hal;
use crate::uctx::Ureg;

unsafe extern "C" {
    fn riscv_fpu_save(state: *mut u64);
    fn riscv_fpu_restore(state: *const u64);
}

pub struct Port;

#[derive(Clone)]
pub struct FpuState([u64; 33]);

const SSTATUS_FS: u64 = 0x6000;

impl Hal for Port {
    type FpuState = FpuState;

    const SYSCALL_ABI: &'static str =
        "riscv64: `ecall`, number in a7, arguments in a0-a5, result in a0 (nothing is read from the user stack)";

    /// # Safety
    /// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
    unsafe fn init_user_mode() -> bool {
        let sstatus: u64;
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("csrr {}, sstatus", out(reg) sstatus, options(nomem, nostack, preserves_flags)) };
        if sstatus & SSTATUS_FS == 0 {
            crate::kprintln!("ifsy: user: riscv64: sstatus.FS is off; user mode needs the FPU {}", crate::console::color::failed());
            return false;
        }
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe {
            asm!("csrw sscratch, zero", "csrw scounteren, {}", in(reg) 7u64, options(nostack));
        }
        crate::kprintln!(
            "ifsy: user: riscv64: U-mode armed (sscratch = 0, scounteren = 7); ABI: {} {}",
            Self::SYSCALL_ABI,
            crate::console::color::pass()
        );
        true
    }

    fn set_kernel_stack(top: u64) {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe { addr_of_mut!(traps::riscv_kernel_sp).write_volatile(top) };
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn enter_user(regs: &Ureg) -> ! {
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { traps::enter_user(regs) }
    }

    fn fpu_initial() -> FpuState {
        FpuState([0; 33])
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn fpu_save(state: &mut FpuState) {
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { riscv_fpu_save(state.0.as_mut_ptr()) };
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn fpu_restore(state: &FpuState) {
        unsafe { riscv_fpu_restore(state.0.as_ptr()) };
    }
}
