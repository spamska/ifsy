//! Rust bindings for the 4BSD scheduler in `c/4bsd.c`.
//!
//! A port of FreeBSD's 4BSD scheduler. Like FreeBSD's global `sched_lock`, it
//! keeps its state in globals and relies on the kernel serialising every call
//! behind one lock (see `sys/src/sched.rs`).
//!
//! A thread is an opaque handle (`*mut thread_t`); the NULL handle is the
//! CPU's idle context.
#![no_std]

pub mod ffi {
    #![allow(non_snake_case)]
    #![allow(non_camel_case_types)]
    #![allow(dead_code)]

    use core::ffi::{c_int, c_void};
    pub type thread_t = c_void;

    /// `struct sched_switch` in `4bsd.h`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct sched_switch {
        pub old: *mut c_void,
        pub new: *mut c_void,
        pub kind: c_int,
    }

    impl sched_switch {
        pub const fn empty() -> Self {
            Self { old: core::ptr::null_mut(), new: core::ptr::null_mut(), kind: SW_NONE }
        }
    }

    /// Nothing to switch (same context, or switching is off).
    pub const SW_NONE: c_int = 0;
    /// Save into `old`, resume `new`.
    pub const SW_SWITCH: c_int = 1;
    /// `old` has exited: resume `new`, save nothing.
    pub const SW_THROW: c_int = 2;
    /// `new` has no context and cannot run; must not be switched to.
    pub const SW_NOCTX: c_int = -1;

    unsafe extern "C" {
        // ---- set-up ----
        pub fn sched_init(hz: c_int, stathz: c_int);
        pub fn sched_set_ncpus(n: c_int);
        pub fn sched_ncpus() -> c_int;

        // ---- thread life ----
        pub fn sched_fork(parent: *mut thread_t, child: *mut thread_t) -> bool;
        pub fn sched_exit(td: *mut thread_t);

        // ---- the clock ----
        /// One tick of `td`; true if this CPU should reschedule.
        pub fn sched_tick(td: *mut thread_t) -> bool;
        /// `schedcpu()` (once a second) and every fifth call the load average.
        pub fn sched_background_work();

        // ---- state changes ----
        /// Make `td` runnable; true if this CPU should reschedule.
        pub fn sched_wakeup(td: *mut thread_t) -> bool;
        pub fn sched_sleep(td: *mut thread_t);
        pub fn sched_nice(td: *mut thread_t, nice: c_int);
        pub fn sched_set_class(td: *mut thread_t, class: c_int);
        pub fn sched_prio(td: *mut thread_t, prio: c_int);
        pub fn sched_user_prio(td: *mut thread_t, prio: c_int);
        pub fn sched_lend_prio(td: *mut thread_t, prio: c_int);
        pub fn sched_unlend_prio(td: *mut thread_t, prio: c_int);
        pub fn sched_lend_user_prio(td: *mut thread_t, prio: c_int);
        pub fn sched_lend_user_prio_cond(td: *mut thread_t, prio: c_int);
        pub fn sched_userret_slowpath(td: *mut thread_t);
        pub fn sched_pin(td: *mut thread_t);
        pub fn sched_unpin(td: *mut thread_t);
        pub fn sched_bind(td: *mut thread_t, cpu: c_int) -> bool;
        pub fn sched_unbind(td: *mut thread_t);
        pub fn sched_is_bound(td: *mut thread_t) -> bool;
        pub fn sched_set_cpuset(td: *mut thread_t, mask: u64);

        // ---- queries ----
        pub fn sched_get_priority(td: *mut thread_t) -> c_int;
        pub fn sched_get_user_priority(td: *mut thread_t) -> c_int;
        pub fn sched_pctcpu(td: *mut thread_t) -> u32;
        pub fn sched_estcpu(td: *mut thread_t) -> u32;
        pub fn sched_load() -> c_int;
        pub fn sched_loadavg(i: c_int) -> u32;
        pub fn sched_rr_interval() -> c_int;
        pub fn sched_runnable() -> bool;
        /// This CPU owes a context switch (preemption, slice end, ...).
        pub fn sched_resched_pending() -> bool;

        // ---- context switching ----
        pub fn sched_set_context_switching(on: bool);
        pub fn sched_context_switching() -> bool;
        pub fn sched_set_thread_context(
            td: *mut thread_t,
            stack_top: *mut c_void,
            entry: unsafe extern "C" fn(*mut c_void),
            arg: *mut c_void,
        ) -> bool;
        pub fn sched_switch_prepare(old: *mut thread_t, sw: *mut sched_switch) -> *mut thread_t;
        pub fn sched_switch_run(sw: *const sched_switch);
    }
}
