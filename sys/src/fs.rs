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

use crate::console::color::{failed, pass, warn};
use alloc::vec::Vec;
use lwext4_rust::bindings::O_RDONLY;
use lwext4_rust::{Ext4BlockWrapper, Ext4File, InodeTypes, KernelDevOp};

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::block::{BlockDevice, Dev, Disk, RamDisk, SEEK_SET};
use crate::ext4grow;
use crate::mm;
use crate::store;
use crate::sync::SpinLock;

pub struct DiskOp;

impl KernelDevOp for DiskOp {
    type DevType = Disk<Dev>;

    fn write(dev: &mut Self::DevType, buf: &[u8]) -> Result<usize, i32> {
        dev.write(buf)
    }
    fn read(dev: &mut Self::DevType, buf: &mut [u8]) -> Result<usize, i32> {
        dev.read(buf)
    }
    fn seek(dev: &mut Self::DevType, off: i64, whence: i32) -> Result<i64, i32> {
        dev.seek(off, whence)
    }
    fn flush(dev: &mut Self::DevType) -> Result<usize, i32> {
        dev.flush()
    }
}

pub type Root = Ext4BlockWrapper<DiskOp>;

static ROOT: SpinLock<Option<Root>> = SpinLock::new(None);

const O_WRITE_NEW: u32 = 0x241;

static IMAGE_BYTES: AtomicU64 = AtomicU64::new(0);
static MAX_BYTES: AtomicU64 = AtomicU64::new(0);
static OWNED: SpinLock<Option<(u64, u64)>> = SpinLock::new(None);

pub fn disk_info() -> (u64, u64) {
    (IMAGE_BYTES.load(Ordering::Relaxed), MAX_BYTES.load(Ordering::Relaxed))
}

struct DiskIo<'a>(&'a mut Disk<Dev>);

impl ext4grow::Io for DiskIo<'_> {
    fn read(&mut self, off: u64, buf: &mut [u8]) -> Result<(), ext4grow::GrowError> {
        self.0.seek(off as i64, SEEK_SET).map_err(|_| ext4grow::GrowError::Io)?;
        let mut done = 0;
        while done < buf.len() {
            match self.0.read(&mut buf[done..]) {
                Ok(n) if n > 0 => done += n,
                _ => return Err(ext4grow::GrowError::Io),
            }
        }
        Ok(())
    }

    fn write(&mut self, off: u64, buf: &[u8]) -> Result<(), ext4grow::GrowError> {
        self.0.seek(off as i64, SEEK_SET).map_err(|_| ext4grow::GrowError::Io)?;
        let mut done = 0;
        while done < buf.len() {
            match self.0.write(&buf[done..]) {
                Ok(n) if n > 0 => done += n,
                _ => return Err(ext4grow::GrowError::Io),
            }
        }
        Ok(())
    }
}

fn note_geometry(disk: &mut Disk<Dev>) {
    if matches!(disk.device(), Dev::Part(_)) {
        let cur = disk.size();
        match ext4grow::inspect_io(&mut DiskIo(disk)) {
            Ok(g) => {
                let size = g.blocks * g.block_size;
                IMAGE_BYTES.store(size, Ordering::Relaxed);
                MAX_BYTES.store((g.max_blocks * g.block_size).min(store::max_bytes()).max(size), Ordering::Relaxed);
            }
            Err(_) => {
                IMAGE_BYTES.store(cur, Ordering::Relaxed);
                MAX_BYTES.store(cur, Ordering::Relaxed);
            }
        }
        return;
    }
    let Dev::Ram(ram) = disk.device() else { return };
    let img = ram.bytes();
    IMAGE_BYTES.store(img.len() as u64, Ordering::Relaxed);
    let max = ext4grow::inspect(img).map(|g| g.max_blocks * g.block_size).unwrap_or(0);
    MAX_BYTES.store(max, Ordering::Relaxed);
}

pub fn mount_root(disk: Disk<RamDisk>) -> Result<(), i32> {
    mount_dev(Disk::new(Dev::Ram(disk.into_device())))
}

fn mount_dev(mut disk: Disk<Dev>) -> Result<(), i32> {
    note_geometry(&mut disk);
    let fs = Root::new(disk)?;
    ROOT.with(|r| *r = Some(fs));
    Ok(())
}

fn grown_copy(old: (*mut u8, usize), want: u64) -> Result<(Disk<Dev>, (u64, u64)), String> {
    let old_img = unsafe { core::slice::from_raw_parts(old.0, old.1) };
    let geo = ext4grow::inspect(old_img).map_err(|e| String::from(e.message()))?;
    let new_blocks = want / geo.block_size;
    if new_blocks <= geo.blocks {
        return Err(format!("{} is not larger than the current size ({})", want, geo.blocks * geo.block_size));
    }
    if new_blocks > geo.max_blocks {
        return Err(format!("{} is more than this file system can grow to ({})", want, geo.max_blocks * geo.block_size));
    }
    let bytes = new_blocks * geo.block_size;
    let pages = bytes.div_ceil(4096);
    let (_, free) = mm::frame_stats();
    if free < pages + 4096 {
        return Err(format!("not enough memory: need {} KiB, {} KiB free", pages * 4, free * 4));
    }
    let Some(phys) = mm::with_frames(|f| f.alloc_contiguous(pages, 1)) else {
        return Err(String::from("no contiguous block of memory that large"));
    };
    let base = (phys.0 + mm::hhdm()) as *mut u8;
    let img = unsafe {
        core::ptr::copy_nonoverlapping(old.0, base, old.1);
        core::ptr::write_bytes(base.add(old.1), 0, (pages * 4096) as usize - old.1);
        core::slice::from_raw_parts_mut(base, (pages * 4096) as usize)
    };
    match ext4grow::grow(img, new_blocks) {
        Ok(n) => {
            let disk = unsafe { RamDisk::from_raw(base, (n * geo.block_size) as usize) };
            Ok((Disk::new(Dev::Ram(disk)), (phys.0, pages)))
        }
        Err(e) => {
            mm::with_frames(|f| f.free_contiguous(phys, pages));
            Err(match e {
                ext4grow::GrowError::TooBig { max_blocks } => {
                    format!("more than this file system can grow to ({})", max_blocks * geo.block_size)
                }
                e => String::from(e.message()),
            })
        }
    }
}

pub fn grow_root(want: u64) -> Result<u64, String> {
    ROOT.with(|slot| {
        let Some(root) = slot.take() else { return Err(String::from("no root file system")) };
        let old_disk = root.into_device();
        if matches!(old_disk.device(), Dev::Part(_)) {
            return grow_on_disk(slot, old_disk, want);
        }
        let old = match old_disk.device() {
            Dev::Ram(r) => r.raw_parts(),
            Dev::Part(_) => unreachable!(),
        };
        drop(old_disk);
        let result = grown_copy(old, want);
        match result {
            Ok((mut disk, frames)) => {
                let size = disk.size();
                note_geometry(&mut disk);
                match Root::new(disk) {
                    Ok(fs) => {
                        *slot = Some(fs);
                        if let Some((p, n)) = OWNED.with(|o| o.replace(frames)) {
                            mm::with_frames(|f| f.free_contiguous(mm::PhysAddr(p), n));
                        }
                        Ok(size)
                    }
                    Err(e) => {
                        mm::with_frames(|f| f.free_contiguous(mm::PhysAddr(frames.0), frames.1));
                        remount_old(slot, old);
                        Err(format!("mounting the grown disk failed (errno {}); old disk restored", e))
                    }
                }
            }
            Err(msg) => {
                remount_old(slot, old);
                Err(msg)
            }
        }
    })
}

fn grow_on_disk(slot: &mut Option<Root>, mut disk: Disk<Dev>, want: u64) -> Result<u64, String> {
    let result = (|| -> Result<u64, String> {
        let g = ext4grow::inspect_io(&mut DiskIo(&mut disk)).map_err(|e| String::from(e.message()))?;
        let new_blocks = want / g.block_size;
        if new_blocks <= g.blocks {
            return Err(format!("{} is not larger than the current size ({})", want, g.blocks * g.block_size));
        }
        if new_blocks > g.max_blocks {
            return Err(format!("{} is more than this file system can grow to ({})", want, g.max_blocks * g.block_size));
        }
        store::ensure_bytes(new_blocks * g.block_size)?;
        let n = ext4grow::grow_io(&mut DiskIo(&mut disk), new_blocks).map_err(|e| String::from(e.message()))?;
        store::flush();
        Ok(n * g.block_size)
    })();
    drop(disk);
    let mut fresh = Disk::new(Dev::Part(crate::block::PartDev));
    note_geometry(&mut fresh);
    match Root::new(fresh) {
        Ok(fs) => {
            *slot = Some(fs);
            result
        }
        Err(e) => Err(format!("mounting the file system again failed (errno {}){}", e, result.err().map(|m| format!("; {}", m)).unwrap_or_default())),
    }
}

pub fn switch_root(mut disk: Disk<Dev>) -> Result<(), i32> {
    ROOT.with(|slot| {
        let Some(root) = slot.take() else { return Err(5) };
        let old_disk = root.into_device();
        let old = match old_disk.device() {
            Dev::Ram(r) => Some(r.raw_parts()),
            Dev::Part(_) => None,
        };
        drop(old_disk);
        note_geometry(&mut disk);
        match Root::new(disk) {
            Ok(fs) => {
                *slot = Some(fs);
                Ok(())
            }
            Err(e) => {
                if let Some(o) = old {
                    remount_old(slot, o);
                }
                Err(e)
            }
        }
    })
}

fn remount_old(slot: &mut Option<Root>, old: (*mut u8, usize)) {
    let mut disk = Disk::new(Dev::Ram(unsafe { RamDisk::from_raw(old.0, old.1) }));
    note_geometry(&mut disk);
    *slot = Root::new(disk).ok();
}

pub fn with_root<R>(f: impl FnOnce(&mut Root) -> R) -> Option<R> {
    ROOT.with(|r| r.as_mut().map(f))
}

const MAX_LINK_HOPS: usize = 16;

pub fn is_symlink(path: &str) -> bool {
    with_root(|_| Ext4File::new(path, InodeTypes::EXT4_DE_SYMLINK).check_inode_exist(path, InodeTypes::EXT4_DE_SYMLINK))
        .unwrap_or(false)
}

pub fn read_link(path: &str) -> Result<String, i32> {
    let c = alloc::ffi::CString::new(path).map_err(|_| 22)?;
    let r = with_root(|_| {
        let mut buf = alloc::vec![0u8; 1024];
        let mut n = 0usize;
        let e = unsafe {
            lwext4_rust::bindings::ext4_readlink(c.as_ptr(), buf.as_mut_ptr() as *mut core::ffi::c_char, buf.len(), &mut n)
        };
        if e != 0 {
            return Err(e);
        }
        buf.truncate(n);
        String::from_utf8(buf).map_err(|_| 22)
    });
    r.unwrap_or(Err(5))
}

pub fn make_symlink(target: &str, path: &str) -> Result<(), i32> {
    let (t, p) = (
        alloc::ffi::CString::new(target).map_err(|_| 22)?,
        alloc::ffi::CString::new(path).map_err(|_| 22)?,
    );
    let e = with_root(|_| unsafe { lwext4_rust::bindings::ext4_fsymlink(t.as_ptr(), p.as_ptr()) }).unwrap_or(5);
    if e == 0 { Ok(()) } else { Err(e) }
}

pub enum Followed {
    Done(String),
    Abs(String),
}

pub fn follow_ex(path: &str, follow_last: bool, hops: &mut usize) -> Followed {
    let mut todo: Vec<String> = path.split('/').filter(|c| !c.is_empty()).rev().map(String::from).collect();
    let mut cur = String::new();
    while let Some(c) = todo.pop() {
        if c == "." {
            continue;
        }
        if c == ".." {
            if let Some(i) = cur.rfind('/') {
                cur.truncate(i);
            }
            continue;
        }
        let next = format!("{}/{}", cur, c);
        if (follow_last || !todo.is_empty()) && is_symlink(&next) {
            *hops += 1;
            let Some(target) = (*hops <= MAX_LINK_HOPS).then(|| read_link(&next).ok()).flatten() else {
                cur = next;
                while let Some(rest) = todo.pop() {
                    cur.push('/');
                    cur.push_str(&rest);
                }
                break;
            };
            if target.starts_with('/') {
                let mut abs = target;
                while let Some(rest) = todo.pop() {
                    abs.push('/');
                    abs.push_str(&rest);
                }
                return Followed::Abs(abs);
            }
            todo.extend(target.split('/').filter(|c| !c.is_empty()).rev().map(String::from));
            continue;
        }
        cur = next;
    }
    Followed::Done(if cur.is_empty() { String::from("/") } else { cur })
}

pub fn follow(path: &str, follow_last: bool) -> String {
    let mut hops = 0usize;
    let mut p = String::from(path);
    loop {
        match follow_ex(&p, follow_last, &mut hops) {
            Followed::Done(s) => return s,
            Followed::Abs(s) => p = s,
        }
    }
}

pub fn list_dir(path: &str) -> Result<Vec<(Vec<u8>, InodeTypes)>, i32> {
    let d = Ext4File::new(path, InodeTypes::EXT4_DE_DIR);
    let (names, types) = d.lwext4_dir_entries()?;
    Ok(names
        .into_iter()
        .map(|mut n| {
            n.pop();
            n
        })
        .zip(types)
        .collect())
}

pub fn read_file(path: &str) -> Result<Vec<u8>, i32> {
    let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
    f.file_open(path, O_RDONLY)?;
    let mut out = alloc::vec![0u8; f.file_size() as usize];
    let mut done = 0;
    while done < out.len() {
        let n = f.file_read(&mut out[done..])?;
        if n == 0 {
            break;
        }
        done += n;
    }
    out.truncate(done);
    f.file_close()?;
    Ok(out)
}

pub fn remove_file(path: &str) -> Result<(), i32> {
    let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
    f.file_remove(path).map(|_| ())
}

pub fn write_file(path: &str, data: &[u8]) -> Result<(), i32> {
    let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
    f.file_open(path, O_WRITE_NEW)?;
    let mut done = 0;
    while done < data.len() {
        let n = f.file_write(&data[done..])?;
        if n == 0 {
            f.file_close()?;
            return Err(28);
        }
        done += n;
    }
    f.file_close()?;
    Ok(())
}

pub fn make_dir(path: &str) -> Result<(), i32> {
    if list_dir(path).is_ok() {
        return Ok(());
    }
    let mut d = Ext4File::new(path, InodeTypes::EXT4_DE_DIR);
    d.dir_mk(path).map(|_| ())
}

pub fn write_file_mode(path: &str, data: &[u8], mode: u32) -> Result<(), i32> {
    write_file(path, data)?;
    let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
    f.file_mode_set(mode).map(|_| ())
}

fn print_dir(path: &str) {
    match list_dir(path) {
        Ok(entries) => {
            for (name, ty) in entries {
                let tag = if ty == InodeTypes::EXT4_DE_DIR { "dir" } else { "file" };
                kprintln!("ifsy: fs:   [{}] {}", tag, core::str::from_utf8(&name).unwrap_or("?"));
            }
        }
        Err(e) => kprintln!("ifsy: fs: ls {} failed (errno {}) {}", path, e, failed()),
    }
}

fn pattern(len: usize) -> Vec<u8> {
    let mut x = 0x2545_F491u32;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn run_checks() -> Result<(), &'static str> {
    const MSG: &[u8] = b"Hello from Ifsy, via lwext4!\n";
    write_file("/hello.txt", MSG).map_err(|_| "write /hello.txt")?;
    if read_file("/hello.txt").map_err(|_| "read /hello.txt")? != MSG {
        return Err("small file mismatch");
    }

    let mut d = Ext4File::new("/testdir", InodeTypes::EXT4_DE_DIR);
    d.dir_mk("/testdir").map_err(|_| "mkdir")?;
    write_file("/testdir/a.bin", MSG).map_err(|_| "write in dir")?;
    d.file_rename("/testdir/a.bin", "/testdir/b.bin").map_err(|_| "rename")?;
    if read_file("/testdir/b.bin").map_err(|_| "read renamed")? != MSG {
        return Err("renamed file mismatch");
    }
    if read_file("/testdir/a.bin").is_ok() {
        return Err("old name still exists");
    }

    let big = pattern(300 * 1024 + 123);
    write_file("/big.bin", &big).map_err(|_| "write big")?;
    if read_file("/big.bin").map_err(|_| "read big")? != big {
        return Err("big file mismatch");
    }

    let mut f = Ext4File::new("/hello.txt", InodeTypes::EXT4_DE_REG_FILE);
    f.file_remove("/hello.txt").map_err(|_| "remove")?;
    if read_file("/hello.txt").is_ok() {
        return Err("removed file still readable");
    }
    Ok(())
}

pub fn selftest() {
    kprintln!("ifsy: fs: root listing:");
    print_dir("/");

    let r = with_root(|_| run_checks());
    match r {
        None => return kprintln!("ifsy: fs: self-test skipped (nothing mounted) {}", warn()),
        Some(Ok(())) => kprintln!("ifsy: fs: self-test (create/read/mkdir/rename/remove/300KiB file) {}", pass()),
        Some(Err(why)) => return kprintln!("ifsy: fs: self-test ({}) {}", why, failed()),
    }

    let Some(root) = ROOT.with(|r| r.take()) else { return };
    let disk = root.into_device();
    match mount_dev(disk) {
        Ok(()) => {
            let ok = with_root(|_| {
                read_file("/big.bin").map(|v| v == pattern(300 * 1024 + 123)).unwrap_or(false)
                    && read_file("/testdir/b.bin").map(|v| v.starts_with(b"Hello")).unwrap_or(false)
                    && read_file("/hello.txt").is_err()
            });
            kprintln!(
                "ifsy: fs: remount persistence {}",
                if ok == Some(true) { pass() } else { failed() }
            );
            kprintln!("ifsy: fs: root listing after remount:");
            print_dir("/");
        }
        Err(e) => kprintln!("ifsy: fs: remount failed (errno {}) {}", e, failed()),
    }
}
