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

use crate::arch::{CAPS, Hal, Port};
use crate::sched::{self, SharedAs};
use crate::uctx::Ureg;
pub type FpuState = <Port as Hal>::FpuState;

pub struct SwitchPlan {
    pub out_fpu: *mut FpuState,
    pub in_kstack: u64,
    pub in_aspace: Option<SharedAs>,
    pub in_fpu: *const FpuState,
}

impl SwitchPlan {
    pub const fn none() -> Self {
        Self { out_fpu: core::ptr::null_mut(), in_kstack: 0, in_aspace: None, in_fpu: core::ptr::null() }
    }
}

pub fn perform(plan: SwitchPlan) {
    if !CAPS.user_mode {
        return;
    }

    // SAFETY: the pointer is valid for the lifetime of its owner and access is exclusive (&mut self or the owning lock is held).
    unsafe {
        if !plan.out_fpu.is_null() {
            Port::fpu_save(&mut *plan.out_fpu);
        }
        if plan.in_kstack != 0 {
            Port::set_kernel_stack(plan.in_kstack);
        }
        if let Some(a) = &plan.in_aspace {
            activate(a);
        }
        if !plan.in_fpu.is_null() {
            Port::fpu_restore(&*plan.in_fpu);
        }
    }
}

fn activate(a: &SharedAs) {
    a.with(|s| {
        if crate::mm::hw::read_root() != s.root() {
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            unsafe { s.activate() };
        }
    });
}

pub fn activate_current() {
    if let Some(a) = sched::current_aspace() {
        activate(&a);
    }
}

pub fn enter_user(mut regs: Ureg) -> ! {
    if sched::resume_user(&mut regs) == sched::Resume::Killed {
        crate::arch::halt();
    }
    activate_current();
    // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe { Port::enter_user(&regs) }
}
