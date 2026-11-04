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

pub struct Syscall {
    pub number: u32,
    pub name: &'static str,
}

macro_rules! syscalls {
    ($($name:ident = $num:expr),+ $(,)?) => {
        $(pub const $name: u32 = $num;)+

        pub static TABLE: &[Syscall] = &[
            $(Syscall { number: $num, name: stringify!($name) }),+
        ];
    };
}

syscalls! {
    SYSR1 = 0,
    _ERRSTR = 1,
    BIND = 2,
    CHDIR = 3,
    CLOSE = 4,
    DUP = 5,
    ALARM = 6,
    EXEC = 7,
    EXITS = 8,
    _FSESSION = 9,
    FAUTH = 10,
    _FSTAT = 11,
    SEGBRK = 12,
    _MOUNT = 13,
    OPEN = 14,
    _READ = 15,
    OSEEK = 16,
    SLEEP = 17,
    _STAT = 18,
    RFORK = 19,
    _WRITE = 20,
    PIPE = 21,
    CREATE = 22,
    FD2PATH = 23,
    BRK_ = 24,
    REMOVE = 25,
    _WSTAT = 26,
    _FWSTAT = 27,
    NOTIFY = 28,
    NOTED = 29,
    SEGATTACH = 30,
    SEGDETACH = 31,
    SEGFREE = 32,
    SEGFLUSH = 33,
    RENDEZVOUS = 34,
    UNMOUNT = 35,
    _WAIT = 36,
    SEMACQUIRE = 37,
    SEMRELEASE = 38,
    SEEK = 39,
    FVERSION = 40,
    ERRSTR = 41,
    STAT = 42,
    FSTAT = 43,
    WSTAT = 44,
    FWSTAT = 45,
    MOUNT = 46,
    AWAIT = 47,
    PREAD = 50,
    PWRITE = 51,
    TSEMACQUIRE = 52,
    NSEC = 53,
}

pub const UNKNOWN: u32 = u32::MAX;

pub static QUIET: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support {
    Full,
    Partial(&'static str),
    Missing,
}

pub fn support(number: u32) -> Support {
    match number {
        SYSR1 | SLEEP | NSEC | EXITS | _WAIT | AWAIT | RENDEZVOUS | ERRSTR | _ERRSTR | SEMACQUIRE | TSEMACQUIRE
        | SEMRELEASE | ALARM | NOTIFY | SEGFLUSH | NOTED | _FSESSION | BIND | UNMOUNT | BRK_ | SEGBRK | _READ | PREAD | _WRITE | PWRITE | OPEN | CREATE
        | EXEC | CLOSE | DUP | PIPE | CHDIR | SEEK | STAT | FSTAT | FD2PATH | REMOVE | OSEEK | _STAT | _FSTAT => Support::Full,
        MOUNT | _MOUNT => Support::Partial("9P2000 over a bidirectional pipe, ported from devmnt.c; no cache or read-ahead; clunks of closed files are sent before the next RPC"),
        FAUTH | FVERSION => Support::Partial("9P2000 over a bidirectional pipe, ported from devmnt.c; no authentication protocol is run"),
        SEGATTACH => Support::Partial("classes memory, shared and named shared segments; no physical, image or device classes"),
        SEGDETACH | SEGFREE => Support::Partial("attached segments only; the image, heap and stack cannot be detached or freed"),
        RFORK => Support::Partial("all Plan 9 flags are validated; RFPROC, RFMEM, RFNOWAIT, RFFDG, RFCFDG, RFNAMEG, RFCNAMEG, RFREND act, RFNOTEG only records the note group; RFENVG/RFCENVG/RFNOMNT are accepted (no per-process env or mount restriction yet)"),
        WSTAT | FWSTAT | _WSTAT | _FWSTAT => Support::Partial(
            "sets mode (incl. DMEXCL), mtime, name and length on local files and forwards to 9P servers; uid and gid cannot be changed",
        ),
        _ => Support::Missing,
    }
}

fn name_of(number: u32) -> Option<&'static str> {
    TABLE.iter().find(|s| s.number == number).map(|s| s.name)
}

pub type Args = [u64; 6];

pub fn dispatch(number: u32) -> i64 {
    dispatch_args(number, &[0; 6])
}

pub fn dispatch_args(number: u32, args: &Args) -> i64 {
    use crate::sched;
    match number {
        SYSR1 => 0,
        BIND => crate::sysmnt::sys_bind(args[0], args[1], args[2]),
        UNMOUNT => crate::sysmnt::sys_unmount(args[0], args[1]),
        MOUNT => crate::sysmnt::sys_mount(args[0], args[1], args[2], args[3], args[4]),
        _MOUNT => crate::sysmnt::sys_old_mount(args[0], args[1], args[2], args[3]),
        FAUTH => crate::sysmnt::sys_fauth(args[0], args[1]),
        FVERSION => crate::sysmnt::sys_fversion(args[0], args[1], args[2], args[3]),
        _FSESSION => {
            sched::set_errstr_current("fsession: obsolete, use fversion");
            -1
        }
        OSEEK => crate::io::sys_oseek(args[0], args[1], args[2]),
        _STAT => crate::io::sys_old_stat(args[0], args[1]),
        _FSTAT => crate::io::sys_old_fstat(args[0], args[1]),
        _WSTAT => crate::io::sys_old_wstat(args[0], args[1]),
        _FWSTAT => crate::io::sys_old_fwstat(args[0], args[1]),
        SEGATTACH => crate::seg::sys_segattach(args[0], args[1], args[2], args[3]),
        SEGDETACH => crate::seg::sys_segdetach(args[0]),
        SEGFREE => crate::seg::sys_segfree(args[0], args[1]),
        SEGFLUSH => crate::seg::sys_segflush(args[0], args[1]),
        RFORK => sched::sys_rfork(args[0]),
        EXITS => sched::sys_exits(args[0]),
        SLEEP => sched::sys_sleep(args[0] as i64),
        _WAIT | AWAIT => sched::sys_wait(args[0], args[1]),
        RENDEZVOUS => sched::sys_rendezvous(args[0], args[1]),
        ALARM => sched::sys_alarm(args[0] as i64),
        EXEC => sched::sys_exec(args[0], args[1]),
        NOTIFY => sched::sys_notify(args[0]),
        NOTED => {
            sched::set_errstr_current("noted: no user register block");
            -1
        }
        _READ => crate::io::sys_read(args[0], args[1], args[2], crate::io::CURRENT_OFFSET),
        PREAD => crate::io::sys_read(args[0], args[1], args[2], args[3]),
        _WRITE => crate::io::sys_write(args[0], args[1], args[2], crate::io::CURRENT_OFFSET),
        PWRITE => crate::io::sys_write(args[0], args[1], args[2], args[3]),
        OPEN => crate::io::sys_open(args[0], args[1]),
        CREATE => crate::io::sys_create(args[0], args[1], args[2]),
        CLOSE => crate::io::sys_close(args[0]),
        DUP => crate::io::sys_dup(args[0], args[1]),
        PIPE => crate::io::sys_pipe(args[0]),
        CHDIR => crate::io::sys_chdir(args[0]),
        SEEK => crate::io::sys_seek(args[0], args[1], args[2], args[3]),
        STAT => crate::io::sys_stat(args[0], args[1], args[2]),
        FSTAT => crate::io::sys_fstat(args[0], args[1], args[2]),
        FD2PATH => crate::io::sys_fd2path(args[0], args[1], args[2]),
        REMOVE => crate::io::sys_remove(args[0]),
        WSTAT => crate::io::sys_wstat(args[0], args[1], args[2]),
        FWSTAT => crate::io::sys_fwstat(args[0], args[1], args[2]),
        BRK_ => sched::sys_brk(args[0], false),
        SEGBRK => sched::sys_segbrk(args[0], args[1]),
        _ERRSTR => sched::sys_errstr(args[0], 64),
        ERRSTR => sched::sys_errstr(args[0], args[1]),
        SEMACQUIRE => sched::sys_semacquire(
            args[0],
            if args[1] != 0 { sched::SemMode::Block } else { sched::SemMode::Try },
        ),
        TSEMACQUIRE => sched::sys_semacquire(args[0], sched::SemMode::Timed(args[1])),
        SEMRELEASE => sched::sys_semrelease(args[0], args[1] as i64),
        NSEC => crate::timer::now_ns() as i64,
        _ if QUIET.load(core::sync::atomic::Ordering::Relaxed) => -1,
        _ => {
            match name_of(number) {
                Some(name) => crate::kprintln!("ifsy: syscall: {} ({}) -- not implemented", name, number),
                None if number == UNKNOWN => crate::kprintln!("ifsy: syscall: <number unavailable> -- not implemented"),
                None => crate::kprintln!("ifsy: syscall: unknown number {} -- not implemented", number),
            }
            -1
        }
    }
}

pub fn dispatch_user(number: u32, args: &Args, regs: &mut crate::uctx::Ureg) -> i64 {
    use crate::sched::{self, Outcome, State};
    let Some(me) = sched::current() else { return -1 };
    let mut keep_regs = false;
    let mut result = match number {
        NOTED => match sched::sys_noted(args[0], regs) {
            Outcome::Ret(v) => v,
            Outcome::Replaced => {
                keep_regs = true;
                0
            }
            Outcome::Dead => return 0,
        },
        EXEC => {
            let r = dispatch_args(number, args);
            if r == 0 {
                if let Some(start) = sched::take_start(me) {
                    *regs = start;
                    keep_regs = true;
                    sched::reset_fpu_current();
                }
            }
            r
        }
        RFORK => {
            let r = dispatch_args(number, args);
            if r > 0 && crate::arch::CAPS.user_mode {
                let child = r as sched::Tid;
                let mut child_regs = *regs;
                child_regs.set_ret(0);
                sched::set_start(child, child_regs);
                sched::fork_fpu(me, child);
            }
            r
        }
        _ => dispatch_args(number, args),
    };
    if sched::state_of(me) != Some(State::Running) {
        return result;
    }
    if let Some(real) = sched::finish_blocked_syscall(me) {
        result = real;
    }
    if !keep_regs {
        regs.set_ret(result);
    }
    sched::syscall_return(regs);
    result
}
