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

use core::arch::{asm, global_asm};
use core::mem::{offset_of, size_of};
use core::ptr::addr_of_mut;

use crate::arch::Hal;
use crate::console::color;
use crate::mm::hw::USER_VA_TOP;
use crate::note::Fault;
use crate::sched;
use crate::syscall::{self, Args};
use crate::uctx::{FLAGS_USER_MASK, Ureg};

global_asm!(include_str!("utrap.s"));

unsafe extern "C" {
    fn aarch64_enter_frame(frame: *const TrapFrame) -> !;
    fn aarch64_fpu_save(state: *mut FpuState);
    fn aarch64_fpu_restore(state: *const FpuState);
}

pub struct Port;

#[repr(C, align(16))]
#[derive(Clone)]
pub struct FpuState {
    q: [u128; 32],
    fpsr: u64,
    fpcr: u64,
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct TrapFrame {
    pub x: [u64; 31],
    pub sp: u64,
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
    pub far: u64,
    pub fpsr: u64,
    pub fpcr: u64,
    _pad: [u64; 2],
    pub q: [u128; 32],
}

const _: () = {
    assert!(size_of::<TrapFrame>() == 832);
    assert!(offset_of!(TrapFrame, sp) == 248);
    assert!(offset_of!(TrapFrame, elr) == 256);
    assert!(offset_of!(TrapFrame, spsr) == 264);
    assert!(offset_of!(TrapFrame, esr) == 272);
    assert!(offset_of!(TrapFrame, far) == 280);
    assert!(offset_of!(TrapFrame, fpsr) == 288);
    assert!(offset_of!(TrapFrame, fpcr) == 296);
    assert!(offset_of!(TrapFrame, q) == 320);
    assert!(size_of::<FpuState>() == 528);
};

#[unsafe(no_mangle)]
pub static mut aarch64_kernel_sp: u64 = 0;

static mut STAGE: TrapFrame = TrapFrame::zeroed();

impl TrapFrame {
    pub const fn zeroed() -> Self {
        Self { x: [0; 31], sp: 0, elr: 0, spsr: 0, esr: 0, far: 0, fpsr: 0, fpcr: 0, _pad: [0; 2], q: [0; 32] }
    }

    pub fn to_ureg(&self) -> Ureg {
        let mut u = Ureg::zeroed();
        u.pc = self.elr;
        u.sp = self.sp;
        u.flags = self.spsr & FLAGS_USER_MASK;
        u.gpr[..31].copy_from_slice(&self.x);
        u
    }

    pub fn load_ureg(&mut self, u: &Ureg) {
        self.elr = u.pc;
        self.sp = u.sp;
        self.spsr = u.flags & FLAGS_USER_MASK;
        self.x.copy_from_slice(&u.gpr[..31]);
    }
}

const CNTKCTL_EL0_COUNTERS: u64 = 0b11;

impl Hal for Port {
    type FpuState = FpuState;

    const SYSCALL_ABI: &'static str =
        "aarch64: `svc #0`, number in x8, arguments in x0-x5, result in x0 (nothing is read from the user stack)";

    /// # Safety
    /// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
    unsafe fn init_user_mode() -> bool {
        let cpacr: u64;
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe { asm!("mrs {}, cpacr_el1", out(reg) cpacr, options(nomem, nostack, preserves_flags)) };
        if (cpacr >> 20) & 3 != 3 {
            crate::kprintln!("ifsy: user: aarch64: CPACR_EL1.FPEN is off; user mode needs FP/SIMD {}", color::failed());
            return false;
        }
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe {
            asm!("msr cntkctl_el1, {}", "isb", in(reg) CNTKCTL_EL0_COUNTERS, options(nostack, preserves_flags));
        }
        crate::kprintln!("ifsy: user: aarch64: EL0 armed (FP/SIMD on, counters readable); ABI: {} {}", Self::SYSCALL_ABI, color::pass());
        true
    }

    fn set_kernel_stack(top: u64) {
        // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
        unsafe { addr_of_mut!(aarch64_kernel_sp).write_volatile(top) };
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn enter_user(regs: &Ureg) -> ! {
        super::mask_interrupts();
        let live = crate::sched::current_fpu().unwrap_or_else(Self::fpu_initial);
        // SAFETY: the pointer is valid for the lifetime of its owner and access is exclusive (&mut self or the owning lock is held).
        unsafe {
            let frame = &mut *addr_of_mut!(STAGE);
            *frame = TrapFrame::zeroed();
            frame.load_ureg(regs);
            frame.q = live.q;
            frame.fpsr = live.fpsr;
            frame.fpcr = live.fpcr;
            aarch64_enter_frame(frame)
        }
    }

    fn fpu_initial() -> FpuState {
        FpuState { q: [0; 32], fpsr: 0, fpcr: 0 }
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn fpu_save(state: &mut FpuState) {
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { aarch64_fpu_save(state) };
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn fpu_restore(state: &FpuState) {
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { aarch64_fpu_restore(state) };
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn fpu_snapshot_current(state: &mut FpuState) {
        match unsafe { current_frame() } {
            Some(f) => {
                state.q = f.q;
                state.fpsr = f.fpsr;
                state.fpcr = f.fpcr;
            }
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            None => unsafe { aarch64_fpu_save(state) },
        }
    }

    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn fpu_reset_current(state: &FpuState) {
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { aarch64_fpu_restore(state) };
        if let Some(f) = unsafe { current_frame() } {
            f.q = state.q;
            f.fpsr = state.fpsr;
            f.fpcr = state.fpcr;
        }
    }
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn current_frame() -> Option<&'static mut TrapFrame> {
    let flag: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { asm!("mrs {}, tpidrro_el0", out(reg) flag, options(nomem, nostack, preserves_flags)) };
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    let top = unsafe { addr_of_mut!(aarch64_kernel_sp).read_volatile() };
    if flag != 1 || top == 0 {
        return None;
    }
    // SAFETY: the pointer is valid for the lifetime of its owner and access is exclusive (&mut self or the owning lock is held).
    Some(unsafe { &mut *((top - size_of::<TrapFrame>() as u64) as *mut TrapFrame) })
}

const EC_UNKNOWN: u64 = 0x00;
const EC_FP_TRAP: u64 = 0x07;
const EC_SVC64: u64 = 0x15;
const EC_IABT_LOWER: u64 = 0x20;
const EC_PC_ALIGN: u64 = 0x22;
const EC_DABT_LOWER: u64 = 0x24;
const EC_SP_ALIGN: u64 = 0x26;
const EC_FP_EXC: u64 = 0x2c;
const EC_BRK: u64 = 0x3c;
const ESR_WNR: u64 = 1 << 6;

fn fault_for(f: &TrapFrame) -> Fault {
    let ec = (f.esr >> 26) & 0x3f;
    match ec {
        EC_IABT_LOWER => Fault::Exec(f.far),
        EC_DABT_LOWER if f.esr & ESR_WNR != 0 => Fault::Write(f.far),
        EC_DABT_LOWER => Fault::Read(f.far),
        EC_PC_ALIGN => Fault::Misaligned(f.elr),
        EC_SP_ALIGN => Fault::Misaligned(f.sp),
        EC_UNKNOWN => Fault::InvalidOpcode,
        EC_BRK => Fault::Other("breakpoint"),
        EC_FP_TRAP | EC_FP_EXC => Fault::Other("floating point exception"),
        _ => Fault::Other("unexpected exception"),
    }
}

#[unsafe(no_mangle)]
extern "C" fn aarch64_user_trap(f: &mut TrapFrame, kind: u64) {
    if kind == 1 {
        // SAFETY: architecture-specific privileged or MMIO access at kernel privilege, performed in the init order established by kmain.
        unsafe { super::irq::user_tick() };
        let mut regs = f.to_ureg();
        sched::irq_return(&mut regs);
        f.load_ureg(&regs);
        finish_user(f);
        return;
    }
    let ec = (f.esr >> 26) & 0x3f;
    if ec == EC_SVC64 {
        user_syscall(f);
    } else {
        user_fault(f);
    }
    finish_user(f);
}

fn user_syscall(f: &mut TrapFrame) {
    let mut regs = f.to_ureg();
    let number = if f.x[8] <= u64::from(u32::MAX) { f.x[8] as u32 } else { syscall::UNKNOWN };
    let args: Args = [f.x[0], f.x[1], f.x[2], f.x[3], f.x[4], f.x[5]];
    syscall::dispatch_user(number, &args, &mut regs);
    f.load_ureg(&regs);
}

fn user_fault(f: &mut TrapFrame) {
    let fault = fault_for(f);
    crate::kprintln!(
        "ifsy: user: tid {} {} (esr {:#x}) far={:#x} elr={:#x} sp={:#x} {}",
        sched::current().unwrap_or(0),
        super::traps::exception_class_name((f.esr >> 26) & 0x3f),
        f.esr,
        f.far,
        f.elr,
        f.sp,
        color::warn()
    );
    let mut regs = f.to_ureg();
    sched::fault_return(&mut regs, fault);
    f.load_ureg(&regs);
}

fn finish_user(f: &mut TrapFrame) {
    if f.elr >= USER_VA_TOP {
        let mut regs = f.to_ureg();
        sched::fault_return(&mut regs, Fault::Other("bad program counter"));
        f.load_ureg(&regs);
    }
    crate::context::activate_current();
}
