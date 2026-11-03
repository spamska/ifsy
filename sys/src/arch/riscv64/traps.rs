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

use crate::console::color;
use crate::mm::hw::USER_VA_TOP;
use crate::note::Fault;
use crate::sched;
use crate::syscall::{self, Args};
use crate::uctx::Ureg;

global_asm!(include_str!("trap.s"));

unsafe extern "C" {
    fn riscv_trap_entry();
    fn riscv_enter_frame(frame: *const TrapFrame) -> !;
    fn riscv_fpu_save(state: *mut u64);
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrapFrame {
    pub x: [u64; 32],
    pub sepc: u64,
    pub sstatus: u64,
    pub scause: u64,
    pub stval: u64,
    pub fcsr: u64,
    _pad: u64,
    pub f: [u64; 32],
}

const _: () = {
    assert!(size_of::<TrapFrame>() == 560);
    assert!(offset_of!(TrapFrame, x) == 0);
    assert!(offset_of!(TrapFrame, sepc) == 256);
    assert!(offset_of!(TrapFrame, sstatus) == 264);
    assert!(offset_of!(TrapFrame, scause) == 272);
    assert!(offset_of!(TrapFrame, stval) == 280);
    assert!(offset_of!(TrapFrame, fcsr) == 288);
    assert!(offset_of!(TrapFrame, f) == 304);
};

#[unsafe(no_mangle)]
pub static mut riscv_kernel_sp: u64 = 0;

impl TrapFrame {
    pub const fn zeroed() -> Self {
        Self { x: [0; 32], sepc: 0, sstatus: 0, scause: 0, stval: 0, fcsr: 0, _pad: 0, f: [0; 32] }
    }

    pub fn to_ureg(&self) -> Ureg {
        let mut u = Ureg::zeroed();
        u.pc = self.sepc;
        u.sp = self.x[2];
        for i in 1..32 {
            if i != 2 {
                u.gpr[i] = self.x[i];
            }
        }
        u
    }

    pub fn load_ureg(&mut self, u: &Ureg) {
        self.sepc = u.pc;
        self.x[2] = u.sp;
        for i in 1..32 {
            if i != 2 {
                self.x[i] = u.gpr[i];
            }
        }
    }
}

pub unsafe fn enter_user(regs: &Ureg) -> ! {
    super::mask_interrupts();
    let mut frame = TrapFrame::zeroed();
    frame.load_ureg(regs);
    let mut fp = [0u64; 33];
    unsafe { riscv_fpu_save(fp.as_mut_ptr()) };
    frame.f.copy_from_slice(&fp[..32]);
    frame.fcsr = fp[32];
    unsafe { riscv_enter_frame(&frame) }
}

const CAUSE_INTERRUPT: u64 = 1 << 63;
const IRQ_S_TIMER: u64 = 5;

fn fault_for(f: &TrapFrame) -> Fault {
    let tval = f.stval;
    match f.scause {
        0 | 4 | 6 => Fault::Misaligned(tval),
        1 | 12 => Fault::Exec(tval),
        2 => Fault::InvalidOpcode,
        3 => Fault::Other("breakpoint"),
        5 | 13 => Fault::Read(tval),
        7 | 15 => Fault::Write(tval),
        _ => Fault::Other("unexpected exception"),
    }
}

#[unsafe(no_mangle)]
extern "C" fn riscv_user_trap(f: &mut TrapFrame) {
    if f.scause & CAUSE_INTERRUPT != 0 {
        match f.scause & !CAUSE_INTERRUPT {
            IRQ_S_TIMER => {
                super::irq::timer_tick();
                sched::preempt();
                let mut regs = f.to_ureg();
                sched::irq_return(&mut regs);
                f.load_ureg(&regs);
                finish_user(f);
            }
            code => super::irq::stray(code),
        }
        return;
    }
    match f.scause {
        8 => {
            user_syscall(f);
            finish_user(f);
        }
        0..=7 | 12 | 13 | 15 => {
            user_fault(f);
            finish_user(f);
        }
        _ => fatal(f),
    }
}

fn user_syscall(f: &mut TrapFrame) {
    let mut regs = f.to_ureg();
    regs.pc = regs.pc.wrapping_add(4);
    let number = if f.x[17] <= u64::from(u32::MAX) { f.x[17] as u32 } else { syscall::UNKNOWN };
    let args: Args = [f.x[10], f.x[11], f.x[12], f.x[13], f.x[14], f.x[15]];
    syscall::dispatch_user(number, &args, &mut regs);
    f.load_ureg(&regs);
}

fn user_fault(f: &mut TrapFrame) {
    let fault = fault_for(f);
    crate::kprintln!(
        "ifsy: user: tid {} {} (scause {}) stval={:#x} sepc={:#x} sp={:#x} {}",
        sched::current().unwrap_or(0),
        exception_name(f.scause),
        f.scause,
        f.stval,
        f.sepc,
        f.x[2],
        color::warn()
    );
    let mut regs = f.to_ureg();
    sched::fault_return(&mut regs, fault);
    f.load_ureg(&regs);
}

fn finish_user(f: &mut TrapFrame) {
    if f.sepc >= USER_VA_TOP {
        let mut regs = f.to_ureg();
        sched::fault_return(&mut regs, Fault::Other("bad program counter"));
        f.load_ureg(&regs);
    }
    crate::context::activate_current();
}

fn fatal(f: &TrapFrame) -> ! {
    crate::kprintln!(
        "ifsy: {}: riscv64 user trap scause={} ({}) stval={:#x} sepc={:#x}",
        color::red("TRAP"),
        f.scause,
        exception_name(f.scause),
        f.stval,
        f.sepc
    );
    crate::arch::halt();
}

fn exception_name(code: u64) -> &'static str {
    match code {
        0 => "Instruction address misaligned",
        1 => "Instruction access fault",
        2 => "Illegal instruction",
        3 => "Breakpoint",
        4 => "Load address misaligned",
        5 => "Load access fault",
        6 => "Store/AMO address misaligned",
        7 => "Store/AMO access fault",
        8 => "Environment call from U-mode",
        9 => "Environment call from S-mode",
        12 => "Instruction page fault",
        13 => "Load page fault",
        15 => "Store/AMO page fault",
        _ => "(see scause)",
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn riscv_trap_common() -> ! {
    let (cause, tval, epc): (u64, u64, u64);
    unsafe {
        asm!("csrr {}, scause", out(reg) cause, options(nomem, nostack, preserves_flags));
        asm!("csrr {}, stval", out(reg) tval, options(nomem, nostack, preserves_flags));
        asm!("csrr {}, sepc", out(reg) epc, options(nomem, nostack, preserves_flags));
    }
    let is_interrupt = cause >> 63 != 0;
    let code = cause & !(1u64 << 63);
    crate::kprintln!(
        "ifsy: {}: riscv64 {} cause={} ({}) stval={:#x} sepc={:#x}",
        crate::console::color::red("TRAP"),
        if is_interrupt { "interrupt" } else { "exception" },
        code,
        exception_name(code),
        tval,
        epc,
    );
    crate::arch::halt();
}

pub unsafe fn init() {
    let entry = riscv_trap_entry as usize as u64;
    unsafe {
        asm!("csrw stvec, {}", in(reg) entry, options(nostack, preserves_flags));
        asm!("csrw sscratch, zero", options(nostack, preserves_flags));
    }
    crate::kprintln!("ifsy: hardening: riscv64: trap vector installed (stvec, direct mode) {}", crate::console::color::pass());
}
