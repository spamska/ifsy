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

//! SMP bring-up.
//!
//! Asks Limine for the processor list, starts every application processor (AP) and counts how many
//! reach the kernel. Each AP records itself and then parks with interrupts masked; the boot
//! processor remains the only CPU that runs threads. Scheduling on several CPUs additionally needs
//! per-CPU current-thread and kernel-stack state in `sched`, per-CPU descriptor tables and timers
//! and TLB shootdowns; the C scheduler already takes `ncpus`.

use core::sync::atomic::{AtomicUsize, Ordering};

use limine::mp::Cpu;
use limine::request::MpRequest;

#[used]
#[unsafe(link_section = ".requests")]
static MP_REQUEST: MpRequest = MpRequest::new();

static CPUS: AtomicUsize = AtomicUsize::new(1);
/// APs that have reached `ap_entry` (the boot processor is not counted).
static APS_ONLINE: AtomicUsize = AtomicUsize::new(0);

/// Number of CPUs reported by the bootloader (1 until `enumerate` has run).
pub fn cpu_count() -> usize {
    CPUS.load(Ordering::Relaxed)
}

/// Number of application processors that came up and are parked.
pub fn aps_online() -> usize {
    APS_ONLINE.load(Ordering::Acquire)
}

/// Entry point of an application processor, jumped to by Limine on its own stack.
///
/// # Safety
/// Only Limine may call this, once per AP, after `goto_address` was written.
unsafe extern "C" fn ap_entry(cpu: &Cpu) -> ! {
    cpu.extra.store(1, Ordering::Release);
    APS_ONLINE.fetch_add(1, Ordering::AcqRel);
    // Interrupts stay masked and nothing else is touched: the AP only parks until the scheduler
    // learns to use more than one CPU.
    crate::arch::halt()
}

/// Record the CPU count and start the application processors; returns the CPU count.
pub fn start() -> usize {
    let Some(resp) = MP_REQUEST.get_response() else { return 1 };
    let cpus = resp.cpus();
    let n = cpus.len().max(1);
    CPUS.store(n, Ordering::Relaxed);

    #[cfg(target_arch = "x86_64")]
    let is_bsp = |c: &Cpu| c.lapic_id == resp.bsp_lapic_id();
    #[cfg(target_arch = "aarch64")]
    let is_bsp = |c: &Cpu| c.mpidr == resp.bsp_mpidr();
    #[cfg(target_arch = "riscv64")]
    let is_bsp = |c: &Cpu| c.hartid == resp.bsp_hartid();

    let mut started = 0usize;
    for c in cpus.iter().copied().filter(|c| !is_bsp(c)) {
        c.goto_address.write(ap_entry);
        started += 1;
    }
    // Bounded wait so a CPU that never comes up cannot hang the boot.
    let mut spins = 0u64;
    while APS_ONLINE.load(Ordering::Acquire) < started && spins < 200_000_000 {
        core::hint::spin_loop();
        spins += 1;
    }
    n
}
