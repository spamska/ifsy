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

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::sched;

pub const NAMES: [(&str, u32); 10] = [
    ("sysname", 0o444),
    ("osinfo", 0o444),
    ("time", 0o444),
    ("swap", 0o444),
    ("fsinfo", 0o444),
    ("ps", 0o444),
    ("consinfo", 0o444),
    ("display", 0o444),
    ("diskinfo", 0o444),
    ("user", 0o444),
];

pub fn is_text(name: &str) -> bool {
    NAMES.iter().any(|(n, _)| *n == name)
}

#[cfg(target_arch = "x86_64")]
const MACHINE: &str = "x86_64";
#[cfg(target_arch = "aarch64")]
const MACHINE: &str = "aarch64";
#[cfg(target_arch = "riscv64")]
const MACHINE: &str = "riscv64";

pub fn content(name: &str) -> Option<Vec<u8>> {
    let s = match name {
        "sysname" => String::from("ifsy\n"),
        "user" => format!("{}\n", sched::current_user()),
        "osinfo" => format!("ifsy {} #1 {}\n", env!("CARGO_PKG_VERSION"), MACHINE),
        "time" => {
            let ns = crate::timer::now_ns();
            format!("{:>11} {:>19} {:>19} {:>19}\n", crate::rtc::wall_secs(), ns, ns, 1_000_000_000u64)
        }
        "swap" => {
            let (total, free) = crate::mm::frame_stats();
            format!("{} memory\n4096 pagesize\n{}/{} user\n", total * 4096, free, total)
        }
        "fsinfo" => fsinfo(),
        "ps" => ps(),
        "consinfo" => {
            let (w, h) = crate::console::size();
            format!("{} {}\n", w, h)
        }
        "display" => match crate::console::display_info() {
            Some(d) => format!(
                "{} {} {} {} {} {} {} {} {} {}\n",
                d.cols, d.rows, d.width, d.height, d.cell_w, d.cell_h, d.default_cols, d.default_rows, d.max_cols, d.max_rows
            ),
            None => {
                let (w, h) = crate::console::size();
                format!("{} {} 0 0 0 0 {} {} 0 0\n", w, h, w, h)
            }
        },
        "diskinfo" => {
            let (size, max) = crate::fs::disk_info();
            let (_, free) = crate::mm::frame_stats();
            format!("{} {} {} {}\n", size, max, free * 4096, crate::store::active() as u8)
        }
        _ => return None,
    };
    Some(s.into_bytes())
}

fn fsinfo() -> String {
    use lwext4_rust::bindings::{ext4_mount_point_stats, ext4_mount_stats};
    let got = crate::fs::with_root(|_| {
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        let mut st: ext4_mount_stats = unsafe { core::mem::zeroed() };
        // SAFETY: the 4BSD C state is only touched while the scheduler lock is held (all callers go through SCHED) and after sched_init.
        let r = unsafe { ext4_mount_point_stats(b"/\0".as_ptr() as *const core::ffi::c_char, &mut st) };
        (r, st)
    });
    match got {
        Some((0, st)) => {
            let label: Vec<u8> =
                st.volume_name.iter().map(|&c| c as u8).take_while(|&b| b != 0).collect();
            let label = String::from_utf8_lossy(&label).into_owned();
            format!(
                "/ ext4 {} {} {} {} {} {}\n",
                st.block_size,
                st.blocks_count,
                st.free_blocks_count,
                st.inodes_count,
                st.free_inodes_count,
                if label.is_empty() { "-" } else { label.as_str() }
            )
        }
        _ => String::new(),
    }
}

fn ps() -> String {
    let mut out = String::new();
    for p in sched::proc_list() {
        out.push_str(&format!("{} {} {} {} {}\n", p.tid, p.parent, p.state, p.ticks, p.name));
    }
    out
}
