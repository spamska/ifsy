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

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use sched_4bsd::ffi;

use crate::arch::{CAPS, Hal, Port};
use crate::context::{self, FpuState, SwitchPlan};
use crate::mm::aspace::{AddressSpace, USER_MIN};
use crate::mm::hw::USER_VA_TOP;
use crate::note::{Fault, NCONT, NNOTE, NRSTR, NSAVE, NoteKind, NoteState};
pub use crate::note::Note;
use crate::sync::SpinLock;
use crate::uctx::{self, Ureg};

pub type SharedAs = Arc<SpinLock<AddressSpace>>;

pub type Tid = u32;

pub const HZ: u64 = 100;
pub const BOOT_TID: Tid = 1;

pub const RFNAMEG: u64 = 1 << 0;
pub const RFFDG: u64 = 1 << 2;
pub const RFCNAMEG: u64 = 1 << 10;
pub const RFPROC: u64 = 1 << 4;
pub const RFNOWAIT: u64 = 1 << 6;
pub const RFMEM: u64 = 1 << 5;
pub const RFENVG: u64 = 1 << 1;
pub const RFNOTEG: u64 = 1 << 3;
pub const RFCENVG: u64 = 1 << 11;
pub const RFCFDG: u64 = 1 << 12;
pub const RFREND: u64 = 1 << 13;
pub const RFNOMNT: u64 = 1 << 14;
/// Every rfork flag the kernel understands; any other bit is rejected.
pub const RF_ALL: u64 = RFNAMEG | RFENVG | RFFDG | RFNOTEG | RFPROC | RFMEM | RFNOWAIT | RFCNAMEG | RFCENVG | RFCFDG | RFREND | RFNOMNT;

pub const ERRMAX: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resume {
    Continue,
    Delivered,
    Killed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Ret(i64),
    Replaced,
    Dead,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SemMode {
    Try,
    Block,
    Timed(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wait {
    Timer(u64),
    Child,
    Rendezvous(u64),
    Sem(usize, u64),
    Net,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Ready,
    Running,
    Sleeping(Wait),
    Zombie,
}

struct Thread {
    parent: Tid,
    state: State,
    waitable: bool,
    #[allow(dead_code)]
    exit_failed: bool,
    pending_ret: i64,
    aspace: Option<SharedAs>,
    errstr: String,
    exit_msg: String,
    cpu_ticks: u64,
    alarm_at: Option<u64>,
    notes: NoteState,
    start: Option<Ureg>,
    syscall_blocked: bool,
    wait_buf: Option<(u64, u64)>,
    deadline: Option<u64>,
    stack: Option<Stack>,
    body: Option<(fn(usize), usize)>,
    fpu: Option<Box<FpuState>>,
    fds: crate::io::SharedFds,
    ns: crate::ns::SharedNs,
    /// Who the thread runs as ; inherited by rfork.
    user: String,
    /// Rendezvous group; threads only meet within one group (see RFREND).
    rgrp: u32,
    /// Note group; notes may be posted to every member (see RFNOTEG).
    #[allow(dead_code)]
    ngrp: u32,
}

/// The host owner: the only user that may change its user name, shut the machine down,
/// grow the disk or write outside /tmp and its own home directory.
pub const HOSTOWNER: &str = "root";

const STACK_PAGES: u64 = 16;

enum Stack {
    Kvm { base: u64, top: u64 },
    Heap(alloc::boxed::Box<[u128]>),
}

impl Stack {
    fn alloc() -> Option<Stack> {
        if let Some(top) = crate::mm::aspace::kvm::alloc_stack(STACK_PAGES) {
            return Some(Stack::Kvm { base: top - STACK_PAGES * 4096, top });
        }
        let words = (STACK_PAGES as usize * 4096) / 16;
        Some(Stack::Heap(alloc::vec![0u128; words].into_boxed_slice()))
    }

    fn top(&self) -> u64 {
        match self {
            Stack::Kvm { top, .. } => *top,
            Stack::Heap(b) => b.as_ptr() as u64 + (b.len() * 16) as u64,
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        if let Stack::Kvm { base, .. } = self {
            crate::mm::aspace::kvm::free(*base);
        }
    }
}

impl Thread {
    fn new(parent: Tid, state: State, waitable: bool, aspace: Option<SharedAs>) -> Self {
        Self {
            parent,
            state,
            waitable,
            exit_failed: false,
            pending_ret: 0,
            aspace,
            errstr: String::new(),
            exit_msg: String::new(),
            cpu_ticks: 0,
            alarm_at: None,
            notes: NoteState::new(),
            start: None,
            syscall_blocked: false,
            wait_buf: None,
            deadline: None,
            stack: None,
            body: None,
            fpu: None,
            fds: crate::io::new_table(),
            ns: crate::ns::new_ns(),
            user: String::from(HOSTOWNER),
            rgrp: 0,
            ngrp: 0,
        }
    }
}

struct WaitInfo {
    pid: Tid,
    cpu_ticks: u64,
    msg: String,
}

pub struct Sched {
    threads: BTreeMap<Tid, Thread>,
    current: Option<Tid>,
    next_tid: Tid,
    ticks: u64,
    need_resched: bool,
    rendezvous: BTreeMap<(u32, u64), (Tid, u64)>,
    pending: Option<PendingSwitch>,
    graveyard: Vec<Stack>,
    idle_stack: Option<Stack>,
}

struct PendingSwitch {
    sw: ffi::sched_switch,
    plan: SwitchPlan,
}
// SAFETY: access to the inner value is serialized by the lock (or the data is only moved once), so sharing across threads cannot race.
unsafe impl Send for PendingSwitch {}

static SWITCHING: AtomicBool = AtomicBool::new(false);

fn handle(t: Tid) -> *mut c_void {
    t as usize as *mut c_void
}

fn handle_of(t: Option<Tid>) -> *mut c_void {
    t.map_or(null_mut(), handle)
}

fn tid_of(p: *mut c_void) -> Option<Tid> {
    if p.is_null() { None } else { Some(p as usize as Tid) }
}

impl Sched {
    const fn new() -> Self {
        Self {
            threads: BTreeMap::new(),
            current: None,
            next_tid: BOOT_TID + 1,
            ticks: 0,
            need_resched: false,
            rendezvous: BTreeMap::new(),
            pending: None,
            graveyard: Vec::new(),
            idle_stack: None,
        }
    }

    fn thread(&mut self, t: Tid) -> Option<&mut Thread> {
        self.threads.get_mut(&t)
    }

    fn schedule(&mut self) -> Option<Tid> {
        if let Some(cur) = self.current {
            if self.threads.get(&cur).is_some_and(|t| t.state == State::Running) {
                // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
                if !unsafe { ffi::sched_resched_pending() } {
                    self.need_resched = false;
                    return Some(cur);
                }
                if let Some(t) = self.thread(cur) {
                    t.state = State::Ready;
                }
            }
        }
        let old = self.current;
        let mut sw = ffi::sched_switch::empty();
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        let next = tid_of(unsafe { ffi::sched_switch_prepare(handle_of(old), &mut sw) });
        match sw.kind {
            ffi::SW_SWITCH | ffi::SW_THROW => {
                let plan = self.switch_plan(old, next, sw.kind == ffi::SW_SWITCH);
                self.pending = Some(PendingSwitch { sw, plan });
            }
            ffi::SW_NOCTX => panic!("sched: chosen thread has no context"),
            _ => {}
        }
        if let Some(n) = next {
            if let Some(t) = self.thread(n) {
                t.state = State::Running;
            }
        }
        self.current = next;
        self.need_resched = false;
        next
    }

    fn switch_plan(&mut self, old: Option<Tid>, new: Option<Tid>, save_old: bool) -> SwitchPlan {
        let mut plan = SwitchPlan::none();
        if !CAPS.user_mode {
            return plan;
        }
        if save_old {
            if let Some(b) = old.and_then(|o| self.thread(o)).and_then(|t| t.fpu.as_mut()) {
                plan.out_fpu = &mut **b as *mut FpuState;
            }
        }
        if let Some(n) = new {
            plan.in_aspace = self.aspace_of(n);
            if let Some(t) = self.thread(n) {
                if let Some(st) = &t.stack {
                    plan.in_kstack = st.top();
                }
                if let Some(b) = t.fpu.as_ref() {
                    plan.in_fpu = &**b as *const FpuState;
                }
            }
        }
        plan
    }

    fn wake(&mut self, tid: Tid, ret: i64) {
        let Some(t) = self.thread(tid) else { return };
        if !matches!(t.state, State::Sleeping(_)) {
            return;
        }
        t.state = State::Ready;
        t.pending_ret = ret;
        t.deadline = None;
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        if unsafe { ffi::sched_wakeup(handle(tid)) } {
            self.need_resched = true;
        }
    }

    fn block_current(&mut self, why: Wait) {
        let Some(me) = self.current else { return };
        if let Some(t) = self.thread(me) {
            t.state = State::Sleeping(why);
            t.syscall_blocked = true;
        }
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        unsafe { ffi::sched_sleep(handle(me)) };
        self.need_resched = true;
    }

    fn fork(&mut self, parent: Tid, waitable: bool, aspace: Option<SharedAs>,
            body: Option<(fn(usize), usize)>) -> Option<Tid> {
        let child = self.next_tid;
        let after = child.checked_add(1)?;

        let stack = if SWITCHING.load(Ordering::Relaxed) { Some(Stack::alloc()?) } else { None };

        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        if !unsafe { ffi::sched_fork(handle(parent), handle(child)) } {
            return None;
        }
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        if unsafe { ffi::sched_resched_pending() } {
            self.need_resched = true;
        }
        if let Some(st) = &stack {
            // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
            let ok = unsafe {
                ffi::sched_set_thread_context(handle(child), st.top() as *mut c_void, thread_entry, handle(child))
            };
            if !ok {
                // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
                unsafe { ffi::sched_exit(handle(child)) };
                return None;
            }
        }

        let mut thread = Thread::new(parent, State::Ready, waitable, aspace);
        if let Some(p) = self.threads.get(&parent) {
            thread.user = p.user.clone();
        }
        thread.stack = stack;
        thread.body = body;
        self.next_tid = after;
        self.threads.insert(child, thread);
        Some(child)
    }

    fn aspace_of(&self, tid: Tid) -> Option<SharedAs> {
        self.threads.get(&tid).and_then(|t| t.aspace.clone())
    }

    fn user_read(&self, tid: Tid, va: u64, out: &mut [u8]) -> bool {
        self.aspace_of(tid).is_some_and(|a| a.with(|s| s.user_read(va, out).is_ok()))
    }

    fn user_write(&self, tid: Tid, va: u64, data: &[u8]) -> bool {
        self.aspace_of(tid).is_some_and(|a| a.with(|s| s.user_write(va, data).is_ok()))
    }

    fn user_cstr(&self, tid: Tid, va: u64, max: usize) -> Option<String> {
        let a = self.aspace_of(tid)?;
        let bytes = a.with(|s| s.user_read_cstr(va, max)).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn user_u32(&self, tid: Tid, va: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        self.user_read(tid, va, &mut b).then(|| u32::from_le_bytes(b))
    }

    fn space_id(&self, tid: Tid) -> usize {
        self.threads.get(&tid).and_then(|t| t.aspace.as_ref()).map_or(0, |a| Arc::as_ptr(a) as usize)
    }

    fn reap(&mut self, child: Tid) {
        self.threads.remove(&child);
    }

    fn zombie_child(&self, parent: Tid) -> Option<Tid> {
        self.threads
            .iter()
            .find(|(_, t)| t.parent == parent && t.state == State::Zombie)
            .map(|(&k, _)| k)
    }

    fn has_live_children(&self, parent: Tid) -> bool {
        self.threads.values().any(|t| t.parent == parent && t.waitable && t.state != State::Zombie)
    }

    fn sys_rfork(&mut self, flags: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        if flags & !RF_ALL != 0
            || (flags & RFNAMEG != 0 && flags & RFCNAMEG != 0)
            || (flags & RFENVG != 0 && flags & RFCENVG != 0)
            || (flags & RFFDG != 0 && flags & RFCFDG != 0)
        {
            self.set_errstr(me, "bad arg in system call");
            return -1;
        }
        if flags & RFPROC == 0 {
            if let Some(t) = self.thread(me) {
                if flags & RFFDG != 0 {
                    let copy = t.fds.with(|f| f.clone());
                    t.fds = Arc::new(SpinLock::new(copy));
                } else if flags & RFCFDG != 0 {
                    t.fds = crate::io::new_table();
                }
                if flags & RFREND != 0 {
                    t.rgrp = me;
                }
                if flags & RFNOTEG != 0 {
                    t.ngrp = me;
                }
                if flags & RFCNAMEG != 0 {
                    t.ns = crate::ns::new_ns();
                } else if flags & RFNAMEG != 0 {
                    t.ns = crate::ns::copy_ns(&t.ns);
                }
            }
            return 0;
        }
        let aspace = match self.aspace_of(me) {
            None => None,
            Some(a) if flags & RFMEM != 0 => Some(a),
            Some(a) => match a.with(|s| s.duplicate()) {
                Ok(copy) => Some(Arc::new(SpinLock::new(copy))),
                Err(_) => return -1,
            },
        };
        match self.fork(me, flags & RFNOWAIT == 0, aspace, None) {
            Some(child) => {
                let handler = self.threads.get(&me).map_or(0, |t| t.notes.handler);
                let parent_fds = self.threads.get(&me).map(|t| t.fds.clone());
                let parent_ns = self.threads.get(&me).map(|t| t.ns.clone());
                let (prgrp, pngrp) = self.threads.get(&me).map_or((0, 0), |t| (t.rgrp, t.ngrp));
                if let Some(t) = self.thread(child) {
                    t.notes.handler = handler;
                    t.rgrp = if flags & RFREND != 0 { child } else { prgrp };
                    t.ngrp = if flags & RFNOTEG != 0 { child } else { pngrp };
                    if let Some(ns) = parent_ns {
                        t.ns = if flags & RFCNAMEG != 0 {
                            crate::ns::new_ns()
                        } else if flags & RFNAMEG != 0 {
                            crate::ns::copy_ns(&ns)
                        } else {
                            ns
                        };
                    }
                    if let Some(fds) = parent_fds {
                        t.fds = if flags & RFFDG != 0 {
                            Arc::new(SpinLock::new(fds.with(|f| f.clone())))
                        } else if flags & RFCFDG != 0 {
                            crate::io::new_table()
                        } else {
                            fds
                        };
                    }
                }
                i64::from(child)
            }
            None => -1,
        }
    }

    fn sys_exits(&mut self, msg_ptr: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        let msg = if msg_ptr == 0 {
            String::new()
        } else {
            self.user_cstr(me, msg_ptr, ERRMAX).unwrap_or_else(|| String::from("bad exit string"))
        };
        self.exit_current(msg)
    }

    fn exit_current(&mut self, msg: String) -> i64 {
        let Some(me) = self.current else { return -1 };
        let failed = !msg.is_empty();
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        unsafe { ffi::sched_exit(handle(me)) };
        if let Some(st) = self.thread(me).and_then(|t| t.stack.take()) {
            self.graveyard.push(st);
        }

        let kids: Vec<Tid> = self.threads.iter().filter(|(_, t)| t.parent == me).map(|(&k, _)| k).collect();
        for k in kids {
            if self.threads.get(&k).is_some_and(|t| t.state == State::Zombie) {
                self.reap(k);
            } else if let Some(t) = self.thread(k) {
                t.parent = 0;
            }
        }

        let (parent, waitable, cpu_ticks) = match self.threads.get(&me) {
            Some(t) => (t.parent, t.waitable, t.cpu_ticks),
            None => (0, false, 0),
        };
        let parent_waiting = waitable
            && self.threads.get(&parent).is_some_and(|p| p.state == State::Sleeping(Wait::Child));

        if parent_waiting {
            let info = WaitInfo { pid: me, cpu_ticks, msg };
            let buf = self.thread(parent).and_then(|p| p.wait_buf.take());
            let ret = match buf {
                Some((va, n)) => self.write_wait(parent, va, n, &info),
                None => i64::from(me),
            };
            self.reap(me);
            self.wake(parent, ret);
        } else if waitable && self.threads.contains_key(&parent) {
            if let Some(t) = self.thread(me) {
                t.state = State::Zombie;
                t.exit_failed = failed;
                t.exit_msg = msg;
                t.aspace = None;
                t.fds = crate::io::closed_table();
                t.alarm_at = None;
            }
        } else {
            self.reap(me);
        }

        self.rendezvous.retain(|_, (t, _)| *t != me);
        self.schedule();
        0
    }

    fn write_wait(&self, tid: Tid, buf: u64, n: u64, info: &WaitInfo) -> i64 {
        if buf == 0 {
            return i64::from(info.pid);
        }
        if n == 0 {
            return -1;
        }
        let ms = info.cpu_ticks.saturating_mul(1000) / HZ;
        let text = format!("{} {} {} {} {}", info.pid, ms, 0, ms, info.msg);
        let take = text.len().min((n as usize).saturating_sub(1));
        let mut out = Vec::with_capacity(take + 1);
        out.extend_from_slice(&text.as_bytes()[..take]);
        out.push(0);
        if self.user_write(tid, buf, &out) { take as i64 } else { -1 }
    }

    fn wait_info(&self, tid: Tid) -> Option<WaitInfo> {
        self.threads.get(&tid).map(|t| WaitInfo { pid: tid, cpu_ticks: t.cpu_ticks, msg: t.exit_msg.clone() })
    }

    fn sys_sleep(&mut self, ms: i64) -> i64 {
        let Some(me) = self.current else { return -1 };
        if ms <= 0 {
            if let Some(t) = self.thread(me) {
                t.state = State::Ready;
            }
            self.schedule();
            return 0;
        }
        let delta = (ms as u64).saturating_mul(HZ).saturating_add(999) / 1000;
        let at = self.ticks.saturating_add(delta.max(1));
        self.block_current(Wait::Timer(at));
        self.schedule();
        0
    }

    fn sys_netwait(&mut self) -> i64 {
        self.block_current(Wait::Net);
        self.schedule();
        0
    }

    fn wake_net(&mut self) {
        let due: Vec<Tid> =
            self.threads.iter().filter(|(_, t)| t.state == State::Sleeping(Wait::Net)).map(|(&k, _)| k).collect();
        for tid in due {
            self.wake(tid, 0);
        }
    }

    fn sys_wait(&mut self, buf: u64, n: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        if let Some(z) = self.zombie_child(me) {
            let info = self.wait_info(z);
            self.reap(z);
            return match info {
                Some(i) => self.write_wait(me, buf, n, &i),
                None => i64::from(z),
            };
        }
        if !self.has_live_children(me) {
            return -1;
        }
        if let Some(t) = self.thread(me) {
            t.wait_buf = if buf != 0 { Some((buf, n)) } else { None };
        }
        self.block_current(Wait::Child);
        self.schedule();
        0
    }

    fn sys_rendezvous(&mut self, tag: u64, value: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        let key = (self.threads.get(&me).map_or(0, |t| t.rgrp), tag);
        if let Some((partner, theirs)) = self.rendezvous.remove(&key) {
            self.wake(partner, value as i64);
            return theirs as i64;
        }
        self.rendezvous.insert(key, (me, value));
        self.block_current(Wait::Rendezvous(tag));
        self.schedule();
        0
    }

    fn sys_alarm(&mut self, ms: i64) -> i64 {
        let Some(me) = self.current else { return -1 };
        let now = self.ticks;
        let prev = self
            .thread(me)
            .and_then(|t| t.alarm_at.take())
            .map_or(0, |at| (at.saturating_sub(now).saturating_mul(1000) / HZ) as i64);
        if ms > 0 {
            let delta = ((ms as u64).saturating_mul(HZ).saturating_add(999) / 1000).max(1);
            if let Some(t) = self.thread(me) {
                t.alarm_at = Some(now.saturating_add(delta));
            }
        }
        prev
    }

    fn sys_brk(&mut self, addr: u64, ret_end: bool) -> i64 {
        let Some(me) = self.current else { return -1 };
        let Some(a) = self.aspace_of(me) else { return -1 };
        if !ret_end && addr == 0 {
            return -1;
        }
        match a.with(|s| s.brk(addr)) {
            Ok(end) => if ret_end { end as i64 } else { 0 },
            Err(_) => -1,
        }
    }

    /// segbrk(saddr, addr): grow the segment containing `saddr`; only the heap may be resized.
    fn sys_segbrk(&mut self, saddr: u64, addr: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        let Some(a) = self.aspace_of(me) else { return -1 };
        let heap = saddr == 0
            || a.with(|s| {
                matches!(s.region_at(saddr), Some((_, _, crate::mm::aspace::Kind::Heap))) || saddr == s.brk_end()
            });
        if !heap {
            self.set_errstr(me, "segbrk: not the heap segment");
            return -1;
        }
        self.sys_brk(addr, true)
    }

    fn sys_errstr(&mut self, buf: u64, n: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        if buf == 0 || n == 0 {
            return -1;
        }
        let n = n.min(ERRMAX as u64) as usize;
        let Some(new) = self.user_cstr(me, buf, n) else { return -1 };
        let old = self.threads.get(&me).map(|t| t.errstr.clone()).unwrap_or_default();
        let mut out = Vec::with_capacity(n);
        out.extend_from_slice(&old.as_bytes()[..old.len().min(n - 1)]);
        out.push(0);
        if !self.user_write(me, buf, &out) {
            return -1;
        }
        if let Some(t) = self.thread(me) {
            t.errstr = new;
        }
        0
    }

    fn sys_semacquire(&mut self, addr: u64, mode: SemMode) -> i64 {
        let Some(me) = self.current else { return -1 };
        if addr % 4 != 0 {
            return -1;
        }
        let Some(v) = self.user_u32(me, addr) else { return -1 };
        if v > 0 {
            return if self.user_write(me, addr, &(v - 1).to_le_bytes()) { 1 } else { -1 };
        }
        let deadline = match mode {
            SemMode::Try | SemMode::Timed(0) => return 0,
            SemMode::Block => None,
            SemMode::Timed(ms) => Some(self.ticks.saturating_add((ms.saturating_mul(HZ).saturating_add(999) / 1000).max(1))),
        };
        let key = self.space_id(me);
        if let Some(t) = self.thread(me) {
            t.deadline = deadline;
        }
        self.block_current(Wait::Sem(key, addr));
        self.schedule();
        0
    }

    fn sys_semrelease(&mut self, addr: u64, count: i64) -> i64 {
        let Some(me) = self.current else { return -1 };
        if addr % 4 != 0 || count < 0 {
            return -1;
        }
        let Some(v) = self.user_u32(me, addr) else { return -1 };
        let key = self.space_id(me);
        let sleepers: Vec<Tid> = self
            .threads
            .iter()
            .filter(|(_, t)| t.state == State::Sleeping(Wait::Sem(key, addr)))
            .map(|(&k, _)| k)
            .collect();
        let mut left = count as u64;
        for tid in sleepers {
            if left == 0 {
                break;
            }
            self.wake(tid, 1);
            left -= 1;
        }
        if left > 0 {
            let nv = v.saturating_add(left.min(u64::from(u32::MAX)) as u32);
            if !self.user_write(me, addr, &nv.to_le_bytes()) {
                return -1;
            }
        }
        0
    }

    fn set_errstr(&mut self, tid: Tid, msg: &str) {
        if let Some(t) = self.thread(tid) {
            t.errstr = String::from(msg);
        }
    }

    fn post_note(&mut self, tid: Tid, note: Note) -> bool {
        let Some(t) = self.thread(tid) else { return false };
        if t.state == State::Zombie {
            return false;
        }
        if t.notes.queue.len() >= NNOTE {
            if note.kind == NoteKind::User {
                return false;
            }
            t.notes.queue.pop_back();
        }
        t.notes.queue.push_back(note);
        let asleep = matches!(t.state, State::Sleeping(_));
        if asleep {
            self.interrupt(tid);
        }
        true
    }

    fn interrupt(&mut self, tid: Tid) {
        self.rendezvous.retain(|_, (t, _)| *t != tid);
        if let Some(t) = self.thread(tid) {
            t.wait_buf = None;
            t.errstr = String::from("interrupted");
        }
        self.wake(tid, -1);
    }

    fn user_return(&mut self, tid: Tid, regs: &mut Ureg) -> Resume {
        if self.current != Some(tid) {
            return Resume::Continue;
        }
        let Some(t) = self.threads.get(&tid) else { return Resume::Continue };
        let (handler, notified) = (t.notes.handler, t.notes.notified);
        let Some(front) = t.notes.queue.front() else { return Resume::Continue };

        if front.kind == NoteKind::User && notified {
            return Resume::Continue;
        }
        let Some(mut note) = self.thread(tid).and_then(|t| t.notes.queue.pop_front()) else { return Resume::Continue };
        if note.msg.starts_with("sys:") {
            note.msg = format!("{} pc={:#x}", note.msg, regs.pc);
            note.msg.truncate(ERRMAX - 1);
        }

        if handler == 0 || (note.kind != NoteKind::User && notified) {
            return self.kill(note.msg);
        }
        if !self.aspace_of(tid).is_some_and(|a| a.with(|s| code_ok(s, handler))) {
            return self.kill(String::from("Suicide"));
        }

        let saved = *regs;
        let text = note.msg.as_bytes();
        let take = text.len().min(ERRMAX - 1);
        let padded = ((take + 1 + 15) & !15) as u64;
        let frame = (|| {
            let top = regs.sp.checked_sub(uctx::RED_ZONE)? & !15;
            let note_va = top.checked_sub(padded)?;
            let ureg_va = note_va.checked_sub(uctx::UREG_SIZE as u64)? & !15;
            let sp = if uctx::CALL_PUSHES_RETURN { ureg_va.checked_sub(8)? } else { ureg_va };
            Some((note_va, ureg_va, sp))
        })();
        let Some((note_va, ureg_va, sp)) = frame else { return self.kill(String::from("Suicide")) };

        let mut cstr = Vec::with_capacity(take + 1);
        cstr.extend_from_slice(&text[..take]);
        cstr.push(0);
        let wrote = self.user_write(tid, ureg_va, &saved.to_bytes())
            && self.user_write(tid, note_va, &cstr)
            && (!uctx::CALL_PUSHES_RETURN || self.user_write(tid, sp, &0u64.to_le_bytes()));
        if !wrote {
            return self.kill(String::from("Suicide"));
        }

        if let Some(t) = self.thread(tid) {
            t.notes.notified = true;
            t.notes.ureg_va = ureg_va;
            t.notes.saved_flags = saved.flags;
            t.notes.last = Some(note);
        }
        regs.pc = handler;
        regs.sp = sp;
        regs.set_args(ureg_va, note_va);
        regs.clear_link();
        Resume::Delivered
    }

    fn kill(&mut self, msg: String) -> Resume {
        self.exit_current(msg);
        Resume::Killed
    }

    fn sys_notify(&mut self, f: u64) -> i64 {
        let Some(me) = self.current else { return -1 };
        if f != 0 && !self.aspace_of(me).is_some_and(|a| a.with(|s| code_ok(s, f))) {
            self.set_errstr(me, "bad arg in system call");
            return -1;
        }
        if let Some(t) = self.thread(me) {
            t.notes.handler = f;
        }
        0
    }

    fn sys_noted(&mut self, v: u64, regs: &mut Ureg) -> Outcome {
        let Some(me) = self.current else { return Outcome::Ret(-1) };
        let Some(t) = self.threads.get(&me) else { return Outcome::Ret(-1) };
        let (notified, ureg_va, flags, last) = (t.notes.notified, t.notes.ureg_va, t.notes.saved_flags, t.notes.last.clone());
        let last_msg = || last.as_ref().map_or_else(|| String::from("Suicide"), |n| n.msg.clone());

        if !notified && v != NRSTR {
            self.exit_current(String::from("Suicide"));
            return Outcome::Dead;
        }
        match v {
            NCONT | NRSTR => {
                let (va, fl) = if v == NRSTR {
                    let Some(t) = self.threads.get(&me) else { return Outcome::Ret(-1) };
                    (t.notes.sv_ureg_va, t.notes.sv_flags)
                } else {
                    (ureg_va, flags)
                };
                if v == NRSTR && va == 0 {
                    self.set_errstr(me, "noted: no saved note frame");
                    return Outcome::Ret(-1);
                }
                let mut raw = [0u8; uctx::UREG_SIZE];
                let restored = if self.user_read(me, va, &mut raw) { Ureg::from_bytes(&raw) } else { None };
                let restored = restored.map(|u| u.sanitized(fl)).filter(|u| {
                    self.aspace_of(me).is_some_and(|a| a.with(|s| code_ok(s, u.pc) && stack_ok(s, u.sp)))
                });
                match restored {
                    Some(u) => {
                        if let Some(t) = self.thread(me) {
                            t.notes.notified = false;
                            if v == NRSTR {
                                t.notes.sv_ureg_va = 0;
                            }
                        }
                        *regs = u;
                        Outcome::Replaced
                    }
                    None => {
                        self.exit_current(String::from("Suicide"));
                        Outcome::Dead
                    }
                }
            }
            NSAVE => {
                // Keep the frame for noted(NRSTR) and leave the handler's note state so that
                // further notes can be delivered meanwhile.
                if let Some(t) = self.thread(me) {
                    t.notes.sv_ureg_va = ureg_va;
                    t.notes.sv_flags = flags;
                    t.notes.notified = false;
                }
                Outcome::Ret(0)
            }
            _ => {
                if let Some(t) = self.thread(me) {
                    t.notes.notified = false;
                }
                self.exit_current(last_msg());
                Outcome::Dead
            }
        }
    }

    fn tick(&mut self) -> bool {
        self.ticks = self.ticks.wrapping_add(1);
        let now = self.ticks;

        let due: Vec<Tid> = self
            .threads
            .iter()
            .filter(|(_, t)| matches!(t.state, State::Sleeping(Wait::Timer(at)) if at <= now))
            .map(|(&k, _)| k)
            .collect();
        for tid in due {
            self.wake(tid, 0);
        }
        self.wake_net();

        let alarmed: Vec<Tid> =
            self.threads.iter().filter(|(_, t)| t.alarm_at.is_some_and(|at| at <= now)).map(|(&k, _)| k).collect();
        for tid in alarmed {
            if let Some(t) = self.thread(tid) {
                t.alarm_at = None;
            }
            self.post_note(tid, Note::alarm());
        }

        let timed_out: Vec<Tid> = self
            .threads
            .iter()
            .filter(|(_, t)| matches!(t.state, State::Sleeping(Wait::Sem(..))) && t.deadline.is_some_and(|d| d <= now))
            .map(|(&k, _)| k)
            .collect();
        for tid in timed_out {
            self.wake(tid, 0);
        }

        if let Some(cur) = self.current {
            if let Some(t) = self.thread(cur) {
                t.cpu_ticks = t.cpu_ticks.saturating_add(1);
            }
            // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
            if unsafe { ffi::sched_tick(handle(cur)) } {
                if let Some(t) = self.thread(cur) {
                    t.state = State::Ready;
                }
                self.need_resched = true;
            }
        }
        if now % HZ == 0 {
            // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
            unsafe { ffi::sched_background_work() };
        }
        self.need_resched
    }
}

fn code_ok(a: &AddressSpace, va: u64) -> bool {
    (USER_MIN..USER_VA_TOP).contains(&va) && a.translate(va).is_some_and(|(_, f)| f.user_accessible() && f.executable())
}

fn stack_ok(a: &AddressSpace, sp: u64) -> bool {
    sp > USER_MIN && sp <= USER_VA_TOP && a.translate(sp - 1).is_some_and(|(_, f)| f.user_accessible() && f.writable())
}

static SCHED: SpinLock<Sched> = SpinLock::new(Sched::new());

fn locked<R>(f: impl FnOnce(&mut Sched) -> R) -> R {
    let irq = crate::arch::irq_save();
    let (r, sw) = SCHED.with(|s| {
        s.graveyard.clear();
        let r = f(s);
        (r, s.pending.take())
    });
    if let Some(PendingSwitch { sw, plan }) = sw {
        context::perform(plan);
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        unsafe { ffi::sched_switch_run(&sw) };
    }
    crate::arch::irq_restore(irq);
    r
}

pub fn init() {
    assert!(!SWITCHING.load(Ordering::Relaxed), "sched::init() with context switching on");
    SCHED.with(|s| {
        let idle = s.idle_stack.take();
        *s = Sched::new();
        s.idle_stack = idle;
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        unsafe { ffi::sched_init(HZ as core::ffi::c_int, 0) };
        s.threads.insert(BOOT_TID, Thread::new(0, State::Ready, false, None));
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        if unsafe { ffi::sched_wakeup(handle(BOOT_TID)) } {
            s.schedule();
        }
    });
}

pub fn tick() -> bool {
    SCHED.with(|s| s.tick())
}

pub fn schedule() -> Option<Tid> {
    locked(|s| s.schedule())
}

pub fn preempt() {
    if SWITCHING.load(Ordering::Relaxed) && crate::timer::take_resched() {
        locked(|s| {
            s.schedule();
        });
    }
}

pub fn yield_now() {
    locked(|s| {
        s.sys_sleep(0);
    });
}

pub fn spawn(f: fn(usize), arg: usize) -> Option<Tid> {
    if !SWITCHING.load(Ordering::Relaxed) {
        return None;
    }
    locked(|s| {
        let parent = s.current.unwrap_or(BOOT_TID);
        s.fork(parent, false, None, Some((f, arg)))
    })
}

#[allow(dead_code)]
pub fn switching_active() -> bool {
    SWITCHING.load(Ordering::Relaxed)
}

unsafe extern "C" fn thread_entry(arg: *mut c_void) {
    let tid = arg as usize as Tid;
    crate::arch::irq_enable();
    let body = SCHED.with(|s| s.thread(tid).and_then(|t| t.body.take()));
    match body {
        Some((f, a)) => {
            f(a);
            locked(|s| s.exit_current(String::new()));
        }
        None => match (CAPS.user_mode, take_start(tid)) {
            (true, Some(regs)) => context::enter_user(regs),
            _ => {
                locked(|s| s.exit_current(String::from("rfork: no user mode")));
            }
        },
    }
    crate::arch::halt();
}

unsafe extern "C" fn idle_entry(_: *mut c_void) {
    loop {
        schedule();
        crate::timer::idle();
    }
}

pub fn start_switching() -> bool {
    SCHED.with(|s| {
        if SWITCHING.load(Ordering::Relaxed) {
            return true;
        }
        if s.threads.len() != 1 || s.current != Some(BOOT_TID) {
            return false;
        }
        let Some(stack) = Stack::alloc() else { return false };
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        let ok = unsafe {
            ffi::sched_set_thread_context(null_mut(), stack.top() as *mut c_void, idle_entry, null_mut())
        };
        if !ok {
            return false;
        }
        s.idle_stack = Some(stack);
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        unsafe { ffi::sched_set_context_switching(true) };
        SWITCHING.store(true, Ordering::Relaxed);
        true
    })
}

pub fn current() -> Option<Tid> {
    SCHED.with(|s| s.current)
}

pub struct ProcInfo {
    pub tid: Tid,
    pub parent: Tid,
    pub state: &'static str,
    pub ticks: u64,
    pub name: String,
}

static PNAMES: SpinLock<BTreeMap<Tid, String>> = SpinLock::new(BTreeMap::new());

fn base_of(path: &str) -> String {
    String::from(path.rsplit('/').next().unwrap_or(path))
}

pub fn proc_list() -> Vec<ProcInfo> {
    let mut v: Vec<ProcInfo> = SCHED.with(|s| {
        s.threads
            .iter()
            .map(|(&tid, t)| ProcInfo {
                tid,
                parent: t.parent,
                state: match t.state {
                    State::Ready => "Ready",
                    State::Running => "Running",
                    State::Sleeping(_) => "Sleeping",
                    State::Zombie => "Moribund",
                },
                ticks: t.cpu_ticks,
                name: String::new(),
            })
            .collect()
    });
    PNAMES.with(|n| {
        n.retain(|tid, _| v.iter().any(|p| p.tid == *tid));
        for p in v.iter_mut() {
            p.name = match n.get(&p.tid) {
                Some(s) => s.clone(),
                None if p.tid == BOOT_TID => String::from("kernel"),
                None => String::from("-"),
            };
        }
    });
    v
}

pub fn state_of(tid: Tid) -> Option<State> {
    SCHED.with(|s| s.threads.get(&tid).map(|t| t.state))
}

pub fn pending_ret(tid: Tid) -> Option<i64> {
    SCHED.with(|s| s.threads.get(&tid).map(|t| t.pending_ret))
}

pub fn thread_count() -> usize {
    SCHED.with(|s| s.threads.len())
}

pub fn ticks() -> u64 {
    SCHED.with(|s| s.ticks)
}

pub fn sys_rfork(flags: u64) -> i64 {
    locked(|s| s.sys_rfork(flags))
}

pub fn sys_exits(msg_ptr: u64) -> i64 {
    locked(|s| s.sys_exits(msg_ptr))
}

static NET_WAITERS: AtomicU32 = AtomicU32::new(0);

pub fn net_waiters() -> u32 {
    NET_WAITERS.load(Ordering::Relaxed)
}

pub fn sys_netwait() -> i64 {
    NET_WAITERS.fetch_add(1, Ordering::Relaxed);
    let r = locked(|s| s.sys_netwait());
    NET_WAITERS.fetch_sub(1, Ordering::Relaxed);
    r
}

pub fn wake_net() -> bool {
    SCHED.with(|s| {
        s.wake_net();
        s.need_resched
    })
}

pub fn sys_sleep(ms: i64) -> i64 {
    locked(|s| s.sys_sleep(ms))
}

pub fn sys_wait(buf: u64, n: u64) -> i64 {
    locked(|s| s.sys_wait(buf, n))
}

pub fn sys_alarm(ms: i64) -> i64 {
    SCHED.with(|s| s.sys_alarm(ms))
}

pub fn sys_segbrk(saddr: u64, addr: u64) -> i64 {
    SCHED.with(|s| s.sys_segbrk(saddr, addr))
}

pub fn sys_brk(addr: u64, ret_end: bool) -> i64 {
    SCHED.with(|s| s.sys_brk(addr, ret_end))
}

pub fn sys_errstr(buf: u64, n: u64) -> i64 {
    SCHED.with(|s| s.sys_errstr(buf, n))
}

pub fn sys_semacquire(addr: u64, mode: SemMode) -> i64 {
    locked(|s| s.sys_semacquire(addr, mode))
}

pub fn sys_semrelease(addr: u64, count: i64) -> i64 {
    SCHED.with(|s| s.sys_semrelease(addr, count))
}

pub fn attach_aspace(tid: Tid, aspace: AddressSpace) -> bool {
    SCHED.with(|s| match s.thread(tid) {
        Some(t) => {
            t.aspace = Some(Arc::new(SpinLock::new(aspace)));
            true
        }
        None => false,
    })
}

pub fn with_aspace<R>(tid: Tid, f: impl FnOnce(&mut AddressSpace) -> R) -> Option<R> {
    let a = SCHED.with(|s| s.aspace_of(tid))?;
    Some(a.with(f))
}

pub fn shares_aspace(a: Tid, b: Tid) -> bool {
    SCHED.with(|s| {
        let (x, y) = (s.space_id(a), s.space_id(b));
        x != 0 && x == y
    })
}

pub fn take_note(tid: Tid) -> Option<Note> {
    SCHED.with(|s| s.thread(tid).and_then(|t| t.notes.queue.pop_front()))
}

pub fn notes_pending(tid: Tid) -> usize {
    SCHED.with(|s| s.threads.get(&tid).map_or(0, |t| t.notes.queue.len()))
}

pub fn note_state(tid: Tid) -> Option<(bool, u64)> {
    SCHED.with(|s| s.threads.get(&tid).map(|t| (t.notes.notified, t.notes.handler)))
}

pub fn postnote(tid: Tid, msg: &str, kind: NoteKind) -> bool {
    locked(|s| s.post_note(tid, Note { msg: String::from(msg), kind }))
}

pub fn detach_aspace(tid: Tid) {
    let old = SCHED.with(|s| s.thread(tid).and_then(|t| t.aspace.take()));
    drop(old);
}

pub fn take_start(tid: Tid) -> Option<Ureg> {
    SCHED.with(|s| s.thread(tid).and_then(|t| t.start.take()))
}

pub fn finish_blocked_syscall(tid: Tid) -> Option<i64> {
    SCHED.with(|s| {
        let t = s.thread(tid)?;
        if t.syscall_blocked && t.state == State::Running {
            t.syscall_blocked = false;
            Some(t.pending_ret)
        } else {
            None
        }
    })
}

fn checkpoint(regs: &mut Ureg) -> Resume {
    locked(|s| match s.current {
        Some(me) => s.user_return(me, regs),
        None => Resume::Continue,
    })
}

pub fn syscall_return(regs: &mut Ureg) -> Resume {
    checkpoint(regs)
}

pub fn irq_return(regs: &mut Ureg) -> Resume {
    checkpoint(regs)
}

pub fn fault_return(regs: &mut Ureg, fault: Fault) -> Resume {
    locked(|s| {
        let Some(me) = s.current else { return Resume::Continue };
        s.post_note(me, Note { msg: fault.text(), kind: NoteKind::Debug });
        s.user_return(me, regs)
    })
}

pub fn resume_user(regs: &mut Ureg) -> Resume {
    locked(|s| {
        let Some(me) = s.current else { return Resume::Continue };
        if let Some(t) = s.thread(me) {
            if t.syscall_blocked {
                t.syscall_blocked = false;
                regs.set_ret(t.pending_ret);
            }
        }
        s.user_return(me, regs)
    })
}

pub fn set_errstr_current(msg: &str) {
    SCHED.with(|s| {
        if let Some(me) = s.current {
            s.set_errstr(me, msg);
        }
    });
}

pub fn sys_notify(f: u64) -> i64 {
    SCHED.with(|s| s.sys_notify(f))
}

pub fn sys_noted(v: u64, regs: &mut Ureg) -> Outcome {
    locked(|s| s.sys_noted(v, regs))
}

fn bytes_from_user(va: u64, max: usize) -> Option<Vec<u8>> {
    SCHED.with(|s| {
        let me = s.current?;
        let a = s.aspace_of(me)?;
        a.with(|sp| sp.user_read_cstr(va, max)).ok()
    })
}

fn read_argv(argv: u64) -> Result<Vec<Vec<u8>>, &'static str> {
    use crate::elf::{MAX_ARGS, MAX_ARG_BYTES};
    let mut out = Vec::new();
    if argv == 0 {
        return Ok(out);
    }
    let mut total = 0usize;
    for i in 0..=MAX_ARGS as u64 {
        let mut w = [0u8; 8];
        if !copy_from_user(argv.wrapping_add(8 * i), &mut w) {
            return Err("bad address in exec arguments");
        }
        let p = u64::from_le_bytes(w);
        if p == 0 {
            return Ok(out);
        }
        if out.len() == MAX_ARGS {
            break;
        }
        let Some(a) = bytes_from_user(p, 4096) else { return Err("bad address in exec arguments") };
        if a.len() >= 4096 {
            return Err("exec argument too long");
        }
        total += a.len() + 1;
        if total > MAX_ARG_BYTES {
            return Err("exec arguments too long");
        }
        out.push(a);
    }
    Err("too many exec arguments")
}

fn exec_prepare(path_ptr: u64, argv_ptr: u64) -> Result<(AddressSpace, Ureg), String> {
    let raw = cstr_from_user(path_ptr, 512).ok_or_else(|| String::from("bad address in exec path"))?;
    let path = crate::io::resolve_follow(&raw);
    let args = read_argv(argv_ptr).map_err(String::from)?;
    exec_load(&path, args)
}

fn exec_load(path: &str, mut args: Vec<Vec<u8>>) -> Result<(AddressSpace, Ureg), String> {
    if !path.starts_with('/') && !path.starts_with("#M") {
        return Err(String::from("exec: path must be absolute"));
    }
    if args.is_empty() {
        args.push(path.as_bytes().to_vec());
    }

    let mut path = String::from(path);
    let mut hops = 0;
    let image = loop {
        let image = crate::io::read_image(&path)?;
        if !image.starts_with(b"#!") {
            break image;
        }
        // "#!interp [args...]": run interp with its arguments, then the script path, then the
        // script's own arguments (argv[0] is replaced).
        hops += 1;
        if hops > 4 {
            return Err(String::from("exec: too many #! levels"));
        }
        let end = match image.iter().take(256).position(|&b| b == b'\n') {
            Some(i) => i,
            None if image.len() <= 256 => image.len(),
            None => return Err(String::from("exec: #! line too long")),
        };
        let words: Vec<Vec<u8>> = image[2..end]
            .split(|&b| b == b' ' || b == b'\t' || b == b'\r')
            .filter(|w| !w.is_empty())
            .map(|w| w.to_vec())
            .collect();
        let interp = words.first().ok_or_else(|| String::from("exec: empty #! line"))?;
        let interp = String::from_utf8(interp.clone()).map_err(|_| String::from("exec: bad #! interpreter"))?;
        if !interp.starts_with('/') {
            return Err(String::from("exec: #! interpreter must be an absolute path"));
        }
        let mut next = words;
        next.push(path.as_bytes().to_vec());
        next.extend(args.into_iter().skip(1));
        args = next;
        path = interp;
    };
    let mut space = crate::mm::new_address_space().map_err(|_| String::from("exec: no address space"))?;
    let loaded = crate::elf::load(&mut space, &image).map_err(|e| String::from(e.msg()))?;
    drop(image);
    let top = space.map_stack(crate::mm::aspace::STACK_DEFAULT).map_err(|_| String::from("exec: out of memory"))?;
    let (sp, argv) = crate::elf::build_stack(&space, top, &args).map_err(|_| String::from("exec: out of memory"))?;
    Ok((space, Ureg::entry(loaded.entry, sp, args.len() as u64, argv)))
}

pub fn sys_exec(path_ptr: u64, argv_ptr: u64) -> i64 {
    let Some(me) = current() else { return -1 };
    let prog = cstr_from_user(path_ptr, 1024);
    match exec_prepare(path_ptr, argv_ptr) {
        Err(why) => {
            SCHED.with(|s| s.set_errstr(me, &why));
            -1
        }
        Ok((space, start)) => {
            let old = SCHED.with(|s| {
                let t = s.thread(me)?;
                t.start = Some(start);
                t.notes.reset_for_exec();
                Some(t.aspace.replace(Arc::new(SpinLock::new(space))))
            });
            match old {
                Some(old) => {
                    drop(old);
                    crate::io::close_on_exec();
                    if let Some(p) = prog {
                        PNAMES.with(|n| {
                            n.insert(me, base_of(&p));
                        });
                    }
                    0
                }
                None => -1,
            }
        }
    }
}

pub fn exec_kernel(path: &str, argv: &[&str]) -> Result<Ureg, String> {
    let me = current().ok_or_else(|| String::from("exec: no current thread"))?;
    let args: Vec<Vec<u8>> = argv.iter().map(|a| a.as_bytes().to_vec()).collect();
    let (space, start) = exec_load(path, args)?;
    let old = SCHED.with(|s| {
        let t = s.thread(me)?;
        t.notes.reset_for_exec();
        Some(t.aspace.replace(Arc::new(SpinLock::new(space))))
    });
    match old {
        Some(old) => {
            drop(old);
            PNAMES.with(|n| {
                n.insert(me, base_of(path));
            });
            reset_fpu_current();
            Ok(start)
        }
        None => Err(String::from("exec: no such thread")),
    }
}

struct Launch {
    path: String,
    args: Vec<String>,
}

fn user_launcher(key: usize) {
    let result = {
        // SAFETY: the pointer came from Box::into_raw on the matching spawn path and is consumed exactly once.
        let launch = unsafe { Box::from_raw(key as *mut Launch) };
        let argv: Vec<&str> = launch.args.iter().map(String::as_str).collect();
        exec_kernel(&launch.path, &argv)
    };
    match result {
        Ok(regs) => context::enter_user(regs),
        Err(why) => {
            kprintln!("ifsy: user: cannot start user program: {}", why);
            locked(|s| s.exit_current(why));
        }
    }
}

pub fn spawn_user(path: &str, args: &[&str]) -> Option<Tid> {
    if !CAPS.user_mode || !SWITCHING.load(Ordering::Relaxed) {
        return None;
    }
    let launch = Box::new(Launch { path: String::from(path), args: args.iter().map(|a| String::from(*a)).collect() });
    let key = Box::into_raw(launch) as usize;
    let tid = locked(|s| {
        let parent = s.current.unwrap_or(BOOT_TID);
        s.fork(parent, true, None, Some((user_launcher, key)))
    });
    if tid.is_none() {
        // SAFETY: the pointer came from Box::into_raw on the matching spawn path and is consumed exactly once.
        drop(unsafe { Box::from_raw(key as *mut Launch) });
    }
    tid
}

pub fn current_aspace() -> Option<SharedAs> {
    SCHED.with(|s| s.current.and_then(|c| s.aspace_of(c)))
}

/// The user name of the running thread ("root" when there is no thread yet).
pub fn current_user() -> String {
    SCHED
        .with(|s| s.current.and_then(|c| s.threads.get(&c)).map(|t| t.user.clone()))
        .unwrap_or_else(|| String::from(HOSTOWNER))
}

pub fn is_hostowner() -> bool {
    SCHED.with(|s| s.current.and_then(|c| s.threads.get(&c)).is_none_or(|t| t.user == HOSTOWNER))
}

/// The user name of thread `tid`, if it exists.
pub fn user_of(tid: Tid) -> Option<String> {
    SCHED.with(|s| s.threads.get(&tid).map(|t| t.user.clone()))
}

/// Change the running thread's user name. Only the host owner may; and a thread that has
/// become somebody else cannot become the host owner again.
pub fn set_user(name: &str) -> Result<(), &'static str> {
    let ok = !name.is_empty()
        && name.len() < 28
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.');
    if !ok {
        return Err("bad user name");
    }
    SCHED.with(|s| {
        let me = s.current.ok_or("no current process")?;
        let t = s.threads.get_mut(&me).ok_or("no current process")?;
        if t.user != HOSTOWNER {
            return Err("permission denied");
        }
        t.user = String::from(name);
        Ok(())
    })
}

pub fn current_ns() -> Option<crate::ns::SharedNs> {
    SCHED.with(|s| s.current.and_then(|c| s.threads.get(&c)).map(|t| t.ns.clone()))
}

pub fn current_fds() -> Option<crate::io::SharedFds> {
    SCHED.with(|s| s.current.and_then(|c| s.threads.get(&c)).map(|t| t.fds.clone()))
}

pub fn set_start(tid: Tid, regs: Ureg) {
    SCHED.with(|s| {
        if let Some(t) = s.thread(tid) {
            t.start = Some(regs);
        }
    });
}

pub fn fork_fpu(parent: Tid, child: Tid) {
    if !CAPS.user_mode {
        return;
    }
    SCHED.with(|s| {
        let live = s.current == Some(parent);
        let Some(p) = s.thread(parent) else { return };
        let buf = p.fpu.get_or_insert_with(|| Box::new(Port::fpu_initial()));
        if live {
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            unsafe { Port::fpu_snapshot_current(buf) };
        }
        let copy = (**buf).clone();
        if let Some(c) = s.thread(child) {
            c.fpu = Some(Box::new(copy));
        }
    });
}

pub fn reset_fpu_current() {
    if !CAPS.user_mode {
        return;
    }
    SCHED.with(|s| {
        let Some(me) = s.current else { return };
        let Some(t) = s.thread(me) else { return };
        let buf = t.fpu.get_or_insert_with(|| Box::new(Port::fpu_initial()));
        **buf = Port::fpu_initial();
        // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
        unsafe { Port::fpu_reset_current(buf) };
    });
}

pub fn current_fpu() -> Option<FpuState> {
    SCHED.with(|s| {
        let me = s.current?;
        s.thread(me)?.fpu.as_ref().map(|b| (**b).clone())
    })
}

pub fn errstr_of(tid: Tid) -> Option<String> {
    SCHED.with(|s| s.threads.get(&tid).map(|t| t.errstr.clone()))
}

pub fn copy_from_user(va: u64, out: &mut [u8]) -> bool {
    SCHED.with(|s| s.current.is_some_and(|me| s.user_read(me, va, out)))
}

pub fn copy_to_user(va: u64, data: &[u8]) -> bool {
    SCHED.with(|s| s.current.is_some_and(|me| s.user_write(me, va, data)))
}

pub fn cstr_from_user(va: u64, max: usize) -> Option<String> {
    SCHED.with(|s| s.current.and_then(|me| s.user_cstr(me, va, max)))
}

pub fn sys_rendezvous(tag: u64, value: u64) -> i64 {
    locked(|s| s.sys_rendezvous(tag, value))
}

pub fn selftest() {
    use crate::console::color::{green, red};
    use crate::syscall::{self, dispatch_args};

    init();
    let mut ok = true;
    let mut check = |name: &str, cond: bool| {
        if !cond {
            kprintln!("ifsy: sched: check failed: {} {}", name, crate::console::color::failed());
            ok = false;
        }
    };
    let sys = |n: u32, a0: u64, a1: u64| dispatch_args(n, &[a0, a1, 0, 0, 0, 0]);

    check("boot thread running", current() == Some(1) && state_of(1) == Some(State::Running));

    let child = sys(syscall::RFORK, RFPROC, 0);
    check("rfork returns child tid", child == 2);
    check("child ready, parent still running", state_of(2) == Some(State::Ready) && current() == Some(1));
    check("child's rfork result is 0", pending_ret(2) == Some(0));
    check("rfork without RFPROC makes no thread", sys(syscall::RFORK, 0, 0) == 0 && thread_count() == 2);

    check("wait blocks parent", sys(syscall::_WAIT, 0, 0) == 0);
    check("parent sleeping, child scheduled", state_of(1) == Some(State::Sleeping(Wait::Child)) && current() == Some(2));

    sys(syscall::EXITS, 0, 0);
    check("parent woken with child tid", pending_ret(1) == Some(2) && current() == Some(1));
    check("child reaped", state_of(2).is_none() && thread_count() == 1);
    check("wait with no children fails", sys(syscall::AWAIT, 0, 0) == -1);

    sys(syscall::SLEEP, 30, 0);
    check("sleeping, CPU idle", current().is_none() && matches!(state_of(1), Some(State::Sleeping(Wait::Timer(_)))));
    let t0 = ticks();
    check("not woken early", !tick() && !tick() && current().is_none());
    check("woken on deadline", tick() && ticks() == t0 + 3);
    check("pull runs sleeper", schedule() == Some(1) && state_of(1) == Some(State::Running));

    sys(syscall::RFORK, RFPROC, 0);
    check("rendezvous parks first caller", sys(syscall::RENDEZVOUS, 7, 111) == 0 && current() == Some(3));
    check("rendezvous returns partner's value", sys(syscall::RENDEZVOUS, 7, 222) == 111);
    check("first caller woken with partner's value", pending_ret(1) == Some(222) && state_of(1) == Some(State::Ready));

    let before = current();
    let mut switched = false;
    for _ in 0..(HZ / 10 + 1) {
        if tick() {
            switched = schedule() != before;
            break;
        }
    }
    check("quantum expiry switches thread", switched && current().is_some());

    let me = current();
    sys(syscall::SLEEP, 0, 0);
    check("sleep(0) yields", current() != me && current().is_some());

    check("child runs", current() == Some(3));
    sys(syscall::EXITS, 0, 0);
    check("zombie left, parent scheduled", state_of(3) == Some(State::Zombie) && current() == Some(1));
    check("wait reaps zombie without blocking", sys(syscall::_WAIT, 0, 0) == 3 && thread_count() == 1 && current() == Some(1));

    let free_ok = {
        init();
        current() == Some(1) && thread_count() == 1
    };
    check("re-init leaves only the boot thread", free_ok);

    kprintln!("ifsy: sched: self-test {}", if ok { green("PASS") } else { red("FAIL") });
    init();
}

mod swtest {
    use super::*;
    use core::sync::atomic::{AtomicU32, AtomicUsize};

    pub static LOG: [AtomicU32; 64] = [const { AtomicU32::new(0) }; 64];
    pub static LOG_LEN: AtomicUsize = AtomicUsize::new(0);
    pub static SUMS_OK: AtomicU32 = AtomicU32::new(0);
    pub static EXITED: AtomicU32 = AtomicU32::new(0);
    pub static S2_RAN: AtomicBool = AtomicBool::new(false);
    pub static S1_SAW_S2: AtomicBool = AtomicBool::new(false);

    pub const ROUNDS: u32 = 5;

    pub fn note(id: u32) {
        let i = LOG_LEN.fetch_add(1, Ordering::SeqCst);
        if i < LOG.len() {
            LOG[i].store(id, Ordering::SeqCst);
        }
    }

    pub fn worker(id: usize) {
        let id32 = id as u32;
        let (mut a, mut b, mut c, mut d) = (id as u64, 0x1111_u64 * id as u64, 3u64, 0u64);
        let mut x = 1.5f64 + id as f64;
        for r in 0..ROUNDS {
            note(id32);
            a = core::hint::black_box(a.wrapping_mul(31).wrapping_add(r as u64));
            b = core::hint::black_box(b ^ (a << 3));
            c = core::hint::black_box(c.rotate_left(7) ^ b);
            d = core::hint::black_box(d.wrapping_add(c));
            x = core::hint::black_box(x * 1.0625 + 0.5);
            yield_now();
        }
        let (mut ea, mut eb, mut ec, mut ed) = (id as u64, 0x1111_u64 * id as u64, 3u64, 0u64);
        let mut ex = 1.5f64 + id as f64;
        for r in 0..ROUNDS {
            ea = ea.wrapping_mul(31).wrapping_add(r as u64);
            eb ^= ea << 3;
            ec = ec.rotate_left(7) ^ eb;
            ed = ed.wrapping_add(ec);
            ex = ex * 1.0625 + 0.5;
        }
        if (a, b, c, d) == (ea, eb, ec, ed) && x == ex {
            SUMS_OK.fetch_add(1, Ordering::SeqCst);
        }
        EXITED.fetch_add(1, Ordering::SeqCst);
    }

    pub fn spinner1(_: usize) {
        let limit = crate::timer::Deadline::after_ms(600);
        while !S2_RAN.load(Ordering::SeqCst) && !limit.expired() {
            core::hint::spin_loop();
        }
        S1_SAW_S2.store(S2_RAN.load(Ordering::SeqCst), Ordering::SeqCst);
        EXITED.fetch_add(1, Ordering::SeqCst);
    }

    pub fn spinner2(_: usize) {
        S2_RAN.store(true, Ordering::SeqCst);
        EXITED.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn switch_selftest() {
    use crate::console::color::{green, red, warn};
    use swtest::*;

    init();
    if !start_switching() {
        kprintln!("ifsy: sched: context switching could not be started {}", warn());
        return;
    }

    crate::arch::irq_enable();
    let mut ok = true;
    let mut check = |name: &str, cond: bool| {
        if !cond {
            kprintln!("ifsy: sched: switch check failed: {} {}", name, crate::console::color::failed());
            ok = false;
        }
    };
    let limit = || crate::timer::Deadline::after_ms(3000);

    let irq = crate::arch::irq_save();
    let ids = [spawn(worker, 10), spawn(worker, 11), spawn(worker, 12)];
    let spawned = ids.iter().all(Option::is_some) && thread_count() == 4;
    crate::arch::irq_restore(irq);
    check("spawn kernel threads", spawned);
    let t = limit();
    while EXITED.load(Ordering::SeqCst) < 3 && !t.expired() {
        yield_now();
    }
    let n = LOG_LEN.load(Ordering::SeqCst);
    let log: [u32; 64] = core::array::from_fn(|i| LOG[i].load(Ordering::SeqCst));
    let count = |id: u32| log[..n.min(64)].iter().filter(|&&v| v == id).count();
    check("every thread ran every round", n == 15 && count(10) == 5 && count(11) == 5 && count(12) == 5);

    check("threads take turns", n >= 6 && log[..6].windows(2).all(|w| w[0] != w[1]));
    check("registers survive context switches", SUMS_OK.load(Ordering::SeqCst) == 3);

    let t = limit();
    while thread_count() > 1 && !t.expired() {
        yield_now();
    }
    yield_now();
    check("exited threads are gone", thread_count() == 1);
    check("exited stacks released", SCHED.with(|s| s.graveyard.is_empty()));
    check("boot thread still current and running", current() == Some(BOOT_TID) && state_of(BOOT_TID) == Some(State::Running));

    EXITED.store(0, Ordering::SeqCst);
    let sp = [spawn(spinner1, 0), spawn(spinner2, 0)];
    check("spawn spinners", sp.iter().all(Option::is_some));
    let t = limit();
    while EXITED.load(Ordering::SeqCst) < 2 && !t.expired() {
        yield_now();
    }
    check("tick preempts a thread that never yields", S2_RAN.load(Ordering::SeqCst) && S1_SAW_S2.load(Ordering::SeqCst));
    while thread_count() > 1 && !t.expired() {
        yield_now();
    }
    let t0 = ticks();
    sys_sleep(30);
    let dt = ticks() - t0;
    check("sleep through the idle context", (3..=3 * HZ).contains(&dt) && current() == Some(BOOT_TID));

    if !ok {
        switch_diag();
    }
    crate::arch::mask_interrupts();
    kprintln!("ifsy: sched: context-switch self-test {}", if ok { green("PASS") } else { red("FAIL") });
}

pub fn dump_threads() {
    SCHED.with(|s| {
        kprintln!("ifsy: sched: threads: current={:?} count={} switching={}", s.current, s.threads.len(), SWITCHING.load(Ordering::Relaxed));
        for (tid, t) in s.threads.iter() {
            kprintln!(
                "ifsy: sched:   tid {} parent {} state {:?} blocked_in_syscall {} notes {} cpu_ticks {} exit_msg {:?}",
                tid, t.parent, t.state, t.syscall_blocked, t.notes.queue.len(), t.cpu_ticks, t.exit_msg
            );
        }
    });
}

fn switch_diag() {
    use swtest::*;
    kprintln!(
        "ifsy: sched: diag: log_len={} exited={} sums_ok={} s2_ran={} ticks={} irqs={} switching={}",
        LOG_LEN.load(Ordering::SeqCst),
        EXITED.load(Ordering::SeqCst),
        SUMS_OK.load(Ordering::SeqCst),
        S2_RAN.load(Ordering::SeqCst),
        ticks(),
        crate::timer::irq_count(),
        SWITCHING.load(Ordering::Relaxed)
    );
    let first: alloc::vec::Vec<u32> = LOG.iter().take(LOG_LEN.load(Ordering::SeqCst).min(16)).map(|v| v.load(Ordering::SeqCst)).collect();
    kprintln!("ifsy: sched: diag: first log entries {:?}", first);
    SCHED.with(|s| {
        kprintln!("ifsy: sched: diag: current={:?} threads={} graveyard={}", s.current, s.threads.len(), s.graveyard.len());
        for (tid, t) in s.threads.iter() {
            kprintln!("ifsy: sched: diag:   tid {} state {:?} parent {} cpu_ticks {}", tid, t.state, t.parent, t.cpu_ticks);
        }
    });
}
