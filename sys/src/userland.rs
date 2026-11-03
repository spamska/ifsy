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

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::port::userland::{BINARIES, SDK};
use crate::console::color::{failed, pass, warn};
use crate::fs;

const RCMAIN: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../user/rc/rcmain.plan9"));
const USERS: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../user/etc/users"));

const DIRS: &[&str] = &[
    "/bin", "/bin/service", "/rc", "/rc/lib", "/tmp", "/etc", "/adm", "/var", "/var/run", "/sys", "/sys/log",
    "/usr", "/usr/root", "/usr/lain",
];

const MODE_PROGRAM: u32 = 0o755;
const MODE_DATA: u32 = 0o644;

static RC_READY: AtomicBool = AtomicBool::new(false);

pub fn rc_ready() -> bool {
    RC_READY.load(Ordering::Relaxed)
}

struct Item {
    path: String,
    data: &'static [u8],
    mode: u32,
    /// Configuration: written only if the file does not exist yet, so edits (and passwords)
    /// survive a reboot of a persistent root. Everything else is rewritten when it differs.
    keep: bool,
}

const TAR_BLOCK: usize = 512;

fn tar_field(field: &[u8]) -> usize {
    field
        .iter()
        .take_while(|&&b| b != 0 && b != b' ')
        .fold(0usize, |acc, &b| acc * 8 + (b - b'0') as usize)
}

fn tar_name(h: &[u8]) -> Option<String> {
    let end = |f: &[u8]| f.iter().position(|&b| b == 0).unwrap_or(f.len());
    let name = core::str::from_utf8(&h[..end(&h[..100])]).ok()?;
    let prefix = core::str::from_utf8(&h[345..345 + end(&h[345..500])]).ok()?;
    let rel = if prefix.is_empty() { String::from(name) } else { alloc::format!("{}/{}", prefix, name) };
    Some(alloc::format!("/{}", rel.trim_matches('/')))
}

fn sdk_entries(dirs: &mut Vec<String>, items: &mut Vec<Item>) {
    let mut off = 0usize;
    while off + TAR_BLOCK <= SDK.len() {
        let h = &SDK[off..off + TAR_BLOCK];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size = tar_field(&h[124..136]);
        let mode = tar_field(&h[100..108]) as u32 & 0o777;
        let kind = h[156];
        let Some(path) = tar_name(h) else { break };
        off += TAR_BLOCK;
        if off + size > SDK.len() {
            break;
        }
        match kind {
            b'5' => dirs.push(path),
            b'0' | 0 => items.push(Item { path, data: &SDK[off..off + size], mode, keep: false }),
            _ => {}
        }
        off += size.div_ceil(TAR_BLOCK) * TAR_BLOCK;
    }
}

fn items() -> Vec<Item> {
    let mut v = Vec::new();
    for &(name, data) in BINARIES {
        v.push(Item { path: alloc::format!("/bin/{}", name), data, mode: MODE_PROGRAM, keep: false });
    }
    v.push(Item { path: String::from("/rc/lib/rcmain"), data: RCMAIN, mode: MODE_DATA, keep: false });
    // Configuration: written only if absent, so edits survive a reboot of a persistent root.
    v.push(Item { path: String::from("/adm/users"), data: USERS, mode: MODE_DATA, keep: true });
    v
}

enum Outcome {
    Ok,
    Failed(&'static str, i32),
}

fn install_item(item: &Item) -> Outcome {
    if item.keep && fs::read_file(&item.path).is_ok() {
        return Outcome::Ok;
    }
    if matches!(fs::read_file(&item.path), Ok(back) if back == item.data) {
        return Outcome::Ok;
    }
    if let Err(e) = fs::write_file_mode(&item.path, item.data, item.mode) {
        return Outcome::Failed("write", e);
    }
    match fs::read_file(&item.path) {
        Ok(back) if back == item.data => Outcome::Ok,
        Ok(_) => Outcome::Failed("read-back mismatch", 0),
        Err(e) => Outcome::Failed("read-back", e),
    }
}

pub fn install() {
    if BINARIES.is_empty() {
        kprintln!("ifsy: userland: no user programs are built for this architecture; /bin left empty {}", warn());
        return;
    }

    let mut items = items();
    let mut sdk_dirs = Vec::new();
    sdk_entries(&mut sdk_dirs, &mut items);
    let result = fs::with_root(|_| {
        for d in DIRS.iter().copied().chain(sdk_dirs.iter().map(|d| d.as_str())) {
            if let Err(e) = fs::make_dir(d) {
                return Err((d.to_string(), e));
            }
        }
        Ok(items.iter().map(install_item).collect::<Vec<_>>())
    });

    let outcomes = match result {
        None => return kprintln!("ifsy: userland: install skipped (nothing mounted) {}", warn()),
        Some(Err((dir, e))) => {
            return kprintln!("ifsy: userland: mkdir {} failed (errno {}) {}", dir, e, failed());
        }
        Some(Ok(o)) => o,
    };

    let (mut good, mut bytes) = (0usize, 0usize);
    let mut bad = false;
    for (item, outcome) in items.iter().zip(&outcomes) {
        match outcome {
            Outcome::Ok => {
                good += 1;
                bytes += item.data.len();
                kprintln!("ifsy: userland:   {} ({} bytes, {:04o})", item.path, item.data.len(), item.mode);
            }
            Outcome::Failed(what, e) => {
                bad = true;
                kprintln!("ifsy: userland:   {}: {} failed (errno {}) {}", item.path, what, e, failed());
            }
        }
    }

    if bad {
        kprintln!("ifsy: userland: installed {} of {} files {}", good, items.len(), failed());
        return;
    }
    RC_READY.store(true, Ordering::Relaxed);
    kprintln!("ifsy: userland: installed {} files ({} KiB) into /bin, /rc, /lib and /sys {}", good, bytes / 1024, pass());
}
