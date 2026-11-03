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
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::block::{Dev, Disk, PartDev};
use crate::console::color::{failed, pass, warn};
use crate::gpt::{self, Sectors};
use crate::sync::SpinLock;

const SECTOR: usize = 512;
const PARTITION_NAME: &str = "ifsy";

const LINE_SECTORS: u64 = 8;
const LINE_BYTES: usize = LINE_SECTORS as usize * SECTOR;
const LINES: usize = 1024;

struct Store {
    dev: Box<dyn Sectors + Send>,
    first: u64,
    sectors: u64,
    tags: Vec<u64>,
    data: Vec<u8>,
}

static STORE: SpinLock<Option<Store>> = SpinLock::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SECTORS: AtomicU64 = AtomicU64::new(0);

pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub fn sectors() -> u64 {
    SECTORS.load(Ordering::Acquire)
}

impl Store {
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        let n = (buf.len() / SECTOR) as u64;
        if lba.checked_add(n).ok_or(())? > self.sectors {
            return Err(());
        }
        if n >= 2 * LINE_SECTORS {
            return self.dev.read(self.first + lba, buf);
        }
        let mut done = 0usize;
        while done < buf.len() {
            let cur = lba + (done / SECTOR) as u64;
            let line = cur & !(LINE_SECTORS - 1);
            let idx = (line / LINE_SECTORS) as usize % LINES;
            if self.tags[idx] != line + 1 {
                let have = (self.sectors - line).min(LINE_SECTORS) as usize;
                let slot = &mut self.data[idx * LINE_BYTES..idx * LINE_BYTES + have * SECTOR];
                self.dev.read(self.first + line, slot)?;
                self.tags[idx] = line + 1;
            }
            let in_line = (cur - line) as usize * SECTOR;
            let valid = (self.sectors - line).min(LINE_SECTORS) as usize * SECTOR;
            let take = (buf.len() - done).min(valid - in_line);
            buf[done..done + take].copy_from_slice(&self.data[idx * LINE_BYTES + in_line..idx * LINE_BYTES + in_line + take]);
            done += take;
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        let n = (buf.len() / SECTOR) as u64;
        if n == 0 {
            return Ok(());
        }
        if lba.checked_add(n).ok_or(())? > self.sectors {
            return Err(());
        }
        self.dev.write(self.first + lba, buf)?;
        let mut line = lba & !(LINE_SECTORS - 1);
        while line < lba + n {
            let idx = (line / LINE_SECTORS) as usize % LINES;
            if self.tags[idx] == line + 1 {
                self.tags[idx] = 0;
            }
            line += LINE_SECTORS;
        }
        Ok(())
    }
}

pub fn read_at(lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    STORE.with(|s| s.as_mut().ok_or(())?.read(lba, buf))
}

pub fn write_at(lba: u64, buf: &[u8]) -> Result<(), ()> {
    STORE.with(|s| s.as_mut().ok_or(())?.write(lba, buf))
}

pub fn flush() {
    if !active() {
        return;
    }
    STORE.with(|s| {
        if let Some(s) = s.as_mut() {
            let _ = s.dev.flush();
        }
    });
}

pub fn max_bytes() -> u64 {
    STORE.with(|s| {
        let Some(s) = s.as_mut() else { return 0 };
        let cur = s.sectors * SECTOR as u64;
        match gpt::room(&mut *s.dev, PARTITION_NAME) {
            Ok(last) if last >= s.first => ((last - s.first + 1) * SECTOR as u64).max(cur),
            _ => cur,
        }
    })
}

pub fn ensure_bytes(want: u64) -> Result<(), String> {
    STORE.with(|s| {
        let s = s.as_mut().ok_or_else(|| String::from("no disk"))?;
        let need = want.div_ceil(SECTOR as u64);
        if need <= s.sectors {
            return Ok(());
        }
        let want_last = s.first + need - 1;
        match gpt::room(&mut *s.dev, PARTITION_NAME) {
            Ok(last) if last >= want_last => {}
            Ok(last) => {
                return Err(alloc::format!(
                    "not enough free space on the disk: the partition can reach {} bytes, {} are needed",
                    (last.saturating_sub(s.first) + 1) * SECTOR as u64,
                    need * SECTOR as u64
                ));
            }
            Err(e) => return Err(String::from(e.message())),
        }
        match gpt::grow(&mut *s.dev, PARTITION_NAME, Some(want_last)) {
            Ok(last) => {
                s.sectors = last - s.first + 1;
                SECTORS.store(s.sectors, Ordering::Release);
                s.tags.iter_mut().for_each(|t| *t = 0);
                Ok(())
            }
            Err(e) => Err(String::from(e.message())),
        }
    })
}

#[cfg(target_arch = "x86_64")]
fn open_disk() -> Option<Box<dyn Sectors + Send>> {
    crate::ahci::probe().map(|a| Box::new(a) as Box<dyn Sectors + Send>)
}

#[cfg(not(target_arch = "x86_64"))]
fn open_disk() -> Option<Box<dyn Sectors + Send>> {
    crate::virtio_blk::probe().map(|a| Box::new(a) as Box<dyn Sectors + Send>)
}

pub fn attach() -> Option<Disk<Dev>> {
    let mut dev = open_disk()?;
    let found = match gpt::find(&mut *dev, PARTITION_NAME) {
        Ok(f) => f,
        Err(e) => {
            kprintln!("ifsy: store: no usable partition named '{}' ({}); staying live {}", PARTITION_NAME, e.message(), warn());
            return None;
        }
    };
    let sectors = found.last - found.first + 1;
    let mut sb = [0u8; 2 * SECTOR];
    if sectors < 8 || dev.read(found.first + 2, &mut sb).is_err() || u16::from_le_bytes([sb[56], sb[57]]) != 0xEF53 {
        kprintln!("ifsy: store: partition '{}' holds no ext4 file system; staying live {}", PARTITION_NAME, warn());
        return None;
    }
    let blocks = u32::from_le_bytes([sb[4], sb[5], sb[6], sb[7]]) as u64;
    let log = u32::from_le_bytes([sb[24], sb[25], sb[26], sb[27]]);
    if log > 6 || blocks << (10 + log) > sectors * SECTOR as u64 {
        kprintln!("ifsy: store: the ext4 file system is larger than its partition; staying live {}", failed());
        return None;
    }
    let fs_bytes = blocks << (10 + log);
    STORE.with(|s| {
        *s = Some(Store {
            dev,
            first: found.first,
            sectors,
            tags: alloc::vec![0u64; LINES],
            data: alloc::vec![0u8; LINES * LINE_BYTES],
        })
    });
    SECTORS.store(sectors, Ordering::Release);
    ACTIVE.store(true, Ordering::Release);
    kprintln!(
        "ifsy: store: root file system on the '{}' partition: {} MiB of {} MiB, read from disk, not held in RAM {}",
        PARTITION_NAME,
        fs_bytes >> 20,
        (sectors * SECTOR as u64) >> 20,
        pass()
    );
    Some(Disk::new(Dev::Part(PartDev)))
}

pub fn detach() {
    ACTIVE.store(false, Ordering::Release);
    SECTORS.store(0, Ordering::Release);
    STORE.with(|s| *s = None);
}
