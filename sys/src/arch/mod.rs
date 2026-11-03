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

pub mod hal;
pub use hal::Hal;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64 as port;
#[cfg(target_arch = "x86_64")]
pub use x86_64::{
    Port, halt, idle_wait, init_protection, init_traps, irq_enable, irq_restore, irq_save, mask_interrupts, power_off, reset,
    serial,
    timer_fast, timer_irq_diag, timer_irq_start, timer_irq_stop,
};

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64 as port;
#[cfg(target_arch = "aarch64")]
pub use aarch64::{
    Port, halt, idle_wait, init_protection, init_traps, irq_enable, irq_restore, irq_save, mask_interrupts, power_off, reset,
    serial,
    timer_fast, timer_irq_diag, timer_irq_start, timer_irq_stop,
};

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64 as port;
#[cfg(target_arch = "riscv64")]
pub use riscv64::{
    Port, halt, idle_wait, init_protection, init_traps, irq_enable, irq_restore, irq_save, mask_interrupts, power_off, reset,
    serial,
    timer_fast, timer_irq_diag, timer_irq_start, timer_irq_stop,
};

#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub context_switch: bool,
    pub user_mode: bool,
    pub timer_irq: bool,
}

pub const CAPS: Caps = Caps { context_switch: true, user_mode: cfg!(any(target_arch = "x86_64", target_arch = "riscv64", target_arch = "aarch64")), timer_irq: true };

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
compile_error!(
    "ifsy only supports x86_64 (AMD64), aarch64 (ARM64) and riscv64 (RISC-V) targets; \
     LoongArch and all other architectures are intentionally unsupported"
);
