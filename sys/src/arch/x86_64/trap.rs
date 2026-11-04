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

use core::arch::global_asm;
use core::mem::{offset_of, size_of};

use super::gdt::{USER_CS, USER_DS};
use super::idt::InterruptStackFrame;
use super::pic;
use crate::console::color;
use crate::mm::hw::USER_VA_TOP;
use crate::note::Fault;
use crate::sched;
use crate::syscall::{self, Args};
use crate::uctx::Ureg;

global_asm!(include_str!("trap.s"), options(att_syntax));

unsafe extern "C" {
    static trap_vector_table: [u64; 0];
    static trap_vector_table_end: [u64; 0];
    fn x86_enter_frame(frame: *const TrapFrame) -> !;
}

pub const VECTOR_SYSCALL: u8 = 64;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

const _: () = {
    assert!(size_of::<TrapFrame>() == 176);
    assert!(offset_of!(TrapFrame, r15) == 0);
    assert!(offset_of!(TrapFrame, rax) == 112);
    assert!(offset_of!(TrapFrame, vector) == 120);
    assert!(offset_of!(TrapFrame, error) == 128);
    assert!(offset_of!(TrapFrame, rip) == 136);
    assert!(offset_of!(TrapFrame, ss) == 168);
};

const RFLAGS_USER_MASK: u64 = 0xCD5;
const RFLAGS_FIXED: u64 = 0x202;

pub fn sanitize_flags(flags: u64) -> u64 {
    (flags & RFLAGS_USER_MASK) | RFLAGS_FIXED
}

impl TrapFrame {
    pub const fn zeroed() -> Self {
        Self {
            r15: 0, r14: 0, r13: 0, r12: 0, r11: 0, r10: 0, r9: 0, r8: 0, rbp: 0, rdi: 0, rsi: 0, rdx: 0,
            rcx: 0, rbx: 0, rax: 0, vector: 0, error: 0, rip: 0, cs: 0, rflags: 0, rsp: 0, ss: 0,
        }
    }

    pub fn to_ureg(&self) -> Ureg {
        let mut u = Ureg::zeroed();
        u.pc = self.rip;
        u.sp = self.rsp;
        u.flags = self.rflags;
        u.gpr[0] = self.rax;
        u.gpr[1] = self.rcx;
        u.gpr[2] = self.rdx;
        u.gpr[3] = self.rbx;
        u.gpr[5] = self.rbp;
        u.gpr[6] = self.rsi;
        u.gpr[7] = self.rdi;
        u.gpr[8] = self.r8;
        u.gpr[9] = self.r9;
        u.gpr[10] = self.r10;
        u.gpr[11] = self.r11;
        u.gpr[12] = self.r12;
        u.gpr[13] = self.r13;
        u.gpr[14] = self.r14;
        u.gpr[15] = self.r15;
        u
    }

    pub fn load_ureg(&mut self, u: &Ureg) {
        self.rip = u.pc;
        self.rsp = u.sp;
        self.rflags = sanitize_flags(u.flags);
        self.cs = u64::from(USER_CS);
        self.ss = u64::from(USER_DS);
        self.rax = u.gpr[0];
        self.rcx = u.gpr[1];
        self.rdx = u.gpr[2];
        self.rbx = u.gpr[3];
        self.rbp = u.gpr[5];
        self.rsi = u.gpr[6];
        self.rdi = u.gpr[7];
        self.r8 = u.gpr[8];
        self.r9 = u.gpr[9];
        self.r10 = u.gpr[10];
        self.r11 = u.gpr[11];
        self.r12 = u.gpr[12];
        self.r13 = u.gpr[13];
        self.r14 = u.gpr[14];
        self.r15 = u.gpr[15];
    }

    fn hw_frame(&self) -> InterruptStackFrame {
        InterruptStackFrame {
            instruction_pointer: self.rip,
            code_segment: self.cs,
            cpu_flags: self.rflags,
            stack_pointer: self.rsp,
            stack_segment: self.ss,
        }
    }
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
pub unsafe fn enter_user(regs: &Ureg) -> ! {
    super::mask_interrupts();
    let mut frame = TrapFrame::zeroed();
    frame.load_ureg(regs);
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe { x86_enter_frame(&frame) }
}

pub fn vectors() -> impl Iterator<Item = (u8, u64)> {
    let start = core::ptr::addr_of!(trap_vector_table) as *const [u64; 2];
    let end = core::ptr::addr_of!(trap_vector_table_end) as *const [u64; 2];
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    let n = unsafe { end.offset_from(start) } as usize;
    (0..n).map(move |i| {
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        let [v, a] = unsafe { *start.add(i) };
        (v as u8, a)
    })
}

fn vector_name(v: u64) -> &'static str {
    match v {
        0 => "#DE Divide Error",
        1 => "#DB Debug",
        2 => "NMI",
        3 => "#BP Breakpoint",
        4 => "#OF Overflow",
        5 => "#BR Bound Range Exceeded",
        6 => "#UD Invalid Opcode",
        7 => "#NM Device Not Available",
        8 => "#DF Double Fault",
        10 => "#TS Invalid TSS",
        11 => "#NP Segment Not Present",
        12 => "#SS Stack-Segment Fault",
        13 => "#GP General Protection Fault",
        14 => "#PF Page Fault",
        16 => "#MF x87 Floating-Point Error",
        17 => "#AC Alignment Check",
        18 => "#MC Machine Check",
        19 => "#XM SIMD Floating-Point Exception",
        20 => "#VE Virtualization Exception",
        21 => "#CP Control Protection Exception",
        _ => "unexpected vector",
    }
}

fn has_error_code(v: u64) -> bool {
    matches!(v, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21)
}

fn read_cr2() -> u64 {
    let v: u64;
    // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

fn fault_for(f: &TrapFrame, cr2: u64) -> Fault {
    const PF_WRITE: u64 = 1 << 1;
    const PF_INSTR: u64 = 1 << 4;
    match f.vector {
        0 => Fault::DivideError,
        6 => Fault::InvalidOpcode,
        14 if f.error & PF_INSTR != 0 => Fault::Exec(cr2),
        14 if f.error & PF_WRITE != 0 => Fault::Write(cr2),
        14 => Fault::Read(cr2),
        17 => Fault::Misaligned(0),
        3 => Fault::Other("breakpoint"),
        4 => Fault::Other("overflow"),
        5 => Fault::Other("bound range exceeded"),
        7 | 16 | 19 => Fault::Other("floating point exception"),
        12 => Fault::Other("stack fault"),
        13 => Fault::Other("general protection"),
        _ => Fault::Other("unexpected exception"),
    }
}

#[unsafe(no_mangle)]
extern "C" fn trap_dispatch(f: &mut TrapFrame) {
    super::protect::clac();
    let from_user = f.cs & 3 == 3;
    match f.vector as u8 {
        pic::VECTOR_TIMER => {
            crate::timer::irq();
            pic::eoi_master();
            sched::preempt();
            if from_user {
                let mut regs = f.to_ureg();
                sched::irq_return(&mut regs);
                f.load_ureg(&regs);
                finish_user(f);
            }
        }
        pic::VECTOR_SPURIOUS_MASTER => {}
        pic::VECTOR_SPURIOUS_SLAVE => pic::eoi_master(),
        VECTOR_SYSCALL if from_user => {
            user_syscall(f);
            finish_user(f);
        }
        _ if from_user && f.vector < 32 && !matches!(f.vector, 2 | 8 | 18) => {
            user_fault(f);
            finish_user(f);
        }
        _ => fatal(f),
    }
}

fn user_syscall(f: &mut TrapFrame) {
    let mut regs = f.to_ureg();
    let number = if f.rbp <= u64::from(u32::MAX) { f.rbp as u32 } else { syscall::UNKNOWN };
    let mut args: Args = [0; 6];
    for (i, a) in args.iter_mut().enumerate() {
        let mut w = [0u8; 8];
        if sched::copy_from_user(regs.sp.wrapping_add(8 + 8 * i as u64), &mut w) {
            *a = u64::from_le_bytes(w);
        }
    }
    syscall::dispatch_user(number, &args, &mut regs);
    f.load_ureg(&regs);
}

fn user_fault(f: &mut TrapFrame) {
    let cr2 = if f.vector == 14 { read_cr2() } else { 0 };
    let fault = fault_for(f, cr2);
    crate::kprintln!(
        "ifsy: user: tid {} {} (vector {} error {:#x}) rip={:#x} rsp={:#x} {}",
        sched::current().unwrap_or(0),
        vector_name(f.vector),
        f.vector,
        f.error,
        f.rip,
        f.rsp,
        color::warn()
    );
    let mut regs = f.to_ureg();
    sched::fault_return(&mut regs, fault);
    f.load_ureg(&regs);
}

fn finish_user(f: &mut TrapFrame) {
    if f.rip >= USER_VA_TOP {
        let mut regs = f.to_ureg();
        sched::fault_return(&mut regs, Fault::Other("bad program counter"));
        f.load_ureg(&regs);
    }
    crate::context::activate_current();
    f.rflags = sanitize_flags(f.rflags);
    f.cs = u64::from(USER_CS);
    f.ss = u64::from(USER_DS);
}

fn fatal(f: &TrapFrame) -> ! {
    let hw = f.hw_frame();
    let err = has_error_code(f.vector).then_some(f.error);
    if f.vector == 14 {
        let addr = read_cr2();
        crate::kprintln!("ifsy: {}: {}", color::red("FAULT"), super::idt::classify_page_fault(&hw, f.error, addr));
        crate::kprintln!(
            "ifsy: {}: {} (vector {}) error_code={:#x} fault_addr={:#x} {}",
            color::red("TRAP"), vector_name(f.vector), f.vector, f.error, addr, hw
        );
    } else if let Some(code) = err {
        crate::kprintln!(
            "ifsy: {}: {} (vector {}) error_code={:#x} {}",
            color::red("TRAP"), vector_name(f.vector), f.vector, code, hw
        );
    } else {
        crate::kprintln!("ifsy: {}: {} (vector {}) {}", color::red("TRAP"), vector_name(f.vector), f.vector, hw);
    }
    crate::arch::halt();
}
