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

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::{arch, sched};
static FREQ: AtomicU64 = AtomicU64::new(0);
static EPOCH: AtomicU64 = AtomicU64::new(0);
static TICK_LEN: AtomicU64 = AtomicU64::new(0);
static LAST_TICK: AtomicU64 = AtomicU64::new(0);
static FREQ_TRUSTED: AtomicBool = AtomicBool::new(false);
static IRQ_ACTIVE: AtomicBool = AtomicBool::new(false);
static IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
static RESCHED: AtomicBool = AtomicBool::new(false);
static IN_POLL: AtomicBool = AtomicBool::new(false);
const MAX_CATCHUP: u64 = 1000;
const FAST_POLL_US: u64 = 500;
use crate::arch::port::timer as imp;

#[inline]
pub fn counter() -> u64 {
    imp::counter()
}

pub fn freq() -> u64 {
    FREQ.load(Ordering::Relaxed)
}

pub fn freq_trusted() -> bool {
    FREQ_TRUSTED.load(Ordering::Relaxed)
}

pub fn init() -> u64 {
    let (f, trusted) = imp::frequency();
    let now = imp::counter();
    FREQ.store(f, Ordering::Relaxed);
    FREQ_TRUSTED.store(trusted, Ordering::Relaxed);
    EPOCH.store(now, Ordering::Relaxed);
    TICK_LEN.store((f / sched::HZ).max(1), Ordering::Relaxed);
    LAST_TICK.store(now, Ordering::Relaxed);
    f
}

fn to_ns(delta: u64) -> u64 {
    let f = freq().max(1);
    ((delta as u128 * 1_000_000_000) / f as u128) as u64
}

fn from_us(us: u64) -> u64 {
    ((us as u128 * freq() as u128) / 1_000_000) as u64
}

pub fn now_ns() -> u64 {
    to_ns(counter().wrapping_sub(EPOCH.load(Ordering::Relaxed)))
}

pub fn uptime_ms() -> u64 {
    now_ns() / 1_000_000
}

pub fn delay_us(us: u64) {
    let start = counter();
    let want = from_us(us);
    while counter().wrapping_sub(start) < want {
        core::hint::spin_loop();
    }
}

pub fn delay_ms(ms: u64) {
    delay_us(ms.saturating_mul(1000));
}

#[derive(Clone, Copy, Debug)]
pub struct Deadline {
    start: u64,
    span: u64,
}

impl Deadline {
    pub fn after_us(us: u64) -> Self {
        Self { start: counter(), span: from_us(us) }
    }

    pub fn after_ms(ms: u64) -> Self {
        Self::after_us(ms.saturating_mul(1000))
    }

    pub fn expired(&self) -> bool {
        counter().wrapping_sub(self.start) >= self.span
    }

    pub fn remaining_ms(&self) -> u64 {
        to_ns(self.span.saturating_sub(counter().wrapping_sub(self.start))) / 1_000_000
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Polled {
    pub ticks: u64,
    pub need_resched: bool,
}

pub fn resync() {
    LAST_TICK.store(counter(), Ordering::Relaxed);
}

pub fn poll() -> Polled {
    let len = TICK_LEN.load(Ordering::Relaxed);
    if len == 0 || IN_POLL.swap(true, Ordering::Acquire) {
        return Polled { ticks: 0, need_resched: false };
    }
    let now = counter();
    let mut last = LAST_TICK.load(Ordering::Relaxed);
    let (mut n, mut resched) = (0u64, false);
    while now.wrapping_sub(last) >= len {
        last = last.wrapping_add(len);
        n += 1;
        resched |= sched::tick();
        crate::console::tick();
        if n >= MAX_CATCHUP {
            last = now;
            break;
        }
    }
    LAST_TICK.store(last, Ordering::Relaxed);
    IN_POLL.store(false, Ordering::Release);
    Polled { ticks: n, need_resched: resched }
}

pub fn next_deadline() -> u64 {
    LAST_TICK.load(Ordering::Relaxed).wrapping_add(TICK_LEN.load(Ordering::Relaxed))
}

pub fn start_irq() -> bool {
    if IRQ_ACTIVE.load(Ordering::Relaxed) {
        return true;
    }
    if TICK_LEN.load(Ordering::Relaxed) == 0 {
        return false;
    }
    resync();
    IRQ_COUNT.store(0, Ordering::Relaxed);
    RESCHED.store(false, Ordering::Relaxed);
    let ok = unsafe { arch::timer_irq_start(next_deadline(), sched::HZ) };
    IRQ_ACTIVE.store(ok, Ordering::Relaxed);
    ok
}

pub fn stop_irq() {
    if IRQ_ACTIVE.swap(false, Ordering::Relaxed) {
        unsafe { arch::timer_irq_stop() };
    }
    resync();
}

pub fn irq_active() -> bool {
    IRQ_ACTIVE.load(Ordering::Relaxed)
}

pub fn irq_count() -> u64 {
    IRQ_COUNT.load(Ordering::Relaxed)
}

pub fn irq() -> u64 {
    IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut resched = poll().need_resched;
    let tick = next_deadline();
    let fast = sched::net_waiters() > 0;
    arch::timer_fast(fast);
    let next = if fast {
        resched |= sched::wake_net();
        let soon = counter().wrapping_add(from_us(FAST_POLL_US).max(1));
        if soon.wrapping_sub(counter()) < tick.wrapping_sub(counter()) { soon } else { tick }
    } else {
        tick
    };
    if resched {
        RESCHED.store(true, Ordering::Relaxed);
    }
    next
}

pub fn take_resched() -> bool {
    RESCHED.swap(false, Ordering::Relaxed)
}

pub fn idle() {
    arch::idle_wait();
}

pub fn selftest() {
    use crate::console::color::{failed, green, pass, red, warn};
    use crate::syscall::{self, dispatch_args};

    let f = freq();
    kprintln!(
        "ifsy: timer: counter {} Hz ({}) {}",
        f,
        if freq_trusted() { "hardware/calibrated" } else { "assumed" },
        if freq_trusted() { pass() } else { warn() }
    );

    let mut ok = true;
    let mut check = |name: &str, cond: bool| {
        if !cond {
            kprintln!("ifsy: timer: check failed: {} {}", name, failed());
            ok = false;
        }
    };

    check("frequency plausible", (1_000_000..=10_000_000_000).contains(&f));
    let (c1, c2) = (counter(), counter());
    check("counter monotonic", c2 >= c1);

    let t0 = now_ns();
    delay_us(10_000);
    let waited = now_ns() - t0;
    check("delay_us(10000) waits at least 10 ms", waited >= 10_000_000);
    check("delay_us(10000) is not wildly long", waited < 1_000_000_000);
    let pending = (0..3).any(|_| {
        let d = Deadline::after_ms(5);
        !d.expired() && d.remaining_ms() <= 5
    });
    check("deadline pending", pending);
    let d = Deadline::after_ms(5);
    delay_ms(6);
    check("deadline expires", d.expired() && d.remaining_ms() == 0);
    sched::init();
    let s0 = sched::ticks();
    resync();
    delay_ms(35);
    let p = poll();
    check("poll delivers >= 3 ticks after 35 ms", p.ticks >= 3 && sched::ticks() == s0 + p.ticks);
    check("second poll right away delivers none", (0..3).any(|_| poll().ticks <= 1));
    sched::init();
    dispatch_args(syscall::SLEEP, &[20, 0, 0, 0, 0, 0]);
    check("sleep parks the only thread", sched::current().is_none());
    resync();
    let start = now_ns();
    let limit = Deadline::after_ms(1000);
    let mut woke = false;
    while !limit.expired() {
        if poll().need_resched {
            woke = true;
            break;
        }
        core::hint::spin_loop();
    }
    let slept = now_ns() - start;
    check("sleeper woken by poll()", woke && sched::schedule() == Some(sched::BOOT_TID));
    check("slept at least 20 ms", slept >= 20_000_000);

    kprintln!(
        "ifsy: timer: self-test {} (uptime {} ms, sleep(20) took {} us)",
        if ok { green("PASS") } else { red("FAIL") },
        uptime_ms(),
        slept / 1000
    );
    sched::init();
}

pub fn irq_selftest() {
    use crate::console::color::{failed, green, red, warn};
    use crate::syscall::{self, dispatch_args};

    sched::init();
    if !start_irq() {
        kprintln!("ifsy: timer: no timer interrupt source here; staying on the polled clock {}", warn());
        return;
    }

    let (mut got, mut took);
    let mut attempt = 0;
    loop {
        let s0 = sched::ticks();
        let t0 = now_ns();
        let limit = Deadline::after_ms(500);
        arch::irq_enable();
        while sched::ticks() < s0 + 5 && !limit.expired() {
            core::hint::spin_loop();
        }
        arch::mask_interrupts();
        got = sched::ticks() - s0;
        took = now_ns() - t0;
        attempt += 1;
        if (got >= 5 && took >= 40_000_000 && took < 400_000_000) || attempt >= 3 {
            break;
        }
        resync();
    }
    if got < 5 || irq_count() == 0 {
        arch::timer_irq_diag();
        stop_irq();
        sched::init();
        kprintln!(
            "ifsy: timer: armed the tick interrupt but none arrived in 500 ms ({} ticks); using the polled clock {}",
            got,
            failed()
        );
        return;
    }

    let mut ok = true;
    let mut check = |name: &str, cond: bool| {
        if !cond {
            kprintln!("ifsy: timer: interrupt check failed: {} {}", name, failed());
            ok = false;
        }
    };

    check("5 ticks take about 5/HZ seconds", took >= 40_000_000 && took < 400_000_000);
    check("the interrupt handler ran", irq_count() >= 1);

    sched::init();
    dispatch_args(syscall::SLEEP, &[20, 0, 0, 0, 0, 0]);
    check("sleep parks the only thread", sched::current().is_none());
    resync();
    take_resched();
    let start = now_ns();
    let limit = Deadline::after_ms(1000);
    arch::irq_enable();
    while !take_resched() && !limit.expired() {
        core::hint::spin_loop();
    }
    arch::mask_interrupts();
    let slept = now_ns() - start;
    check("sleeper woken by the interrupt", slept < 1_000_000_000 && sched::schedule() == Some(sched::BOOT_TID));
    check("slept at least 20 ms", slept >= 20_000_000);
    let before = irq_count();
    idle();
    check("idle() returns after an interrupt", irq_count() > before);

    kprintln!(
        "ifsy: timer: interrupt self-test {} ({} IRQs, 5 ticks in {} us, sleep(20) took {} us)",
        if ok { green("PASS") } else { red("FAIL") },
        irq_count(),
        took / 1000,
        slept / 1000
    );
    sched::init();
    take_resched();
}
