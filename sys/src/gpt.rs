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

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

pub const SECTOR: usize = 512;

pub trait Sectors {
    fn sectors(&self) -> u64;
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()>;
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()>;
    fn flush(&mut self) -> Result<(), ()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptError {
    Io,
    NoTable,
    Corrupt(&'static str),
    NoPartition,
    NoRoom,
}

impl GptError {
    pub fn message(self) -> &'static str {
        match self {
            GptError::Io => "disk read or write failed",
            GptError::NoTable => "no GPT on the disk",
            GptError::Corrupt(w) => w,
            GptError::NoPartition => "partition not found",
            GptError::NoRoom => "no free space after the partition",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub index: usize,
    pub first: u64,
    pub last: u64,
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

struct Table {
    header: [u8; SECTOR],
    entries: Vec<u8>,
    num: usize,
    size: usize,
    entries_lba: u64,
}

fn load(d: &mut dyn Sectors) -> Result<Table, GptError> {
    let mut header = [0u8; SECTOR];
    d.read(1, &mut header).map_err(|_| GptError::Io)?;
    if &header[..8] != b"EFI PART" {
        return Err(GptError::NoTable);
    }
    let hsize = u32_at(&header, 12) as usize;
    if !(92..=SECTOR).contains(&hsize) {
        return Err(GptError::Corrupt("bad GPT header size"));
    }
    let num = u32_at(&header, 80) as usize;
    let size = u32_at(&header, 84) as usize;
    if num == 0 || num > 4096 || size < 128 || size > SECTOR || size % 8 != 0 {
        return Err(GptError::Corrupt("bad GPT entry array"));
    }
    let entries_lba = u64_at(&header, 72);
    let sectors = (num * size).div_ceil(SECTOR);
    let mut raw = alloc::vec![0u8; sectors * SECTOR];
    d.read(entries_lba, &mut raw).map_err(|_| GptError::Io)?;
    raw.truncate(num * size);
    Ok(Table { header, entries: raw, num, size, entries_lba })
}

fn name_of(e: &[u8]) -> String {
    let mut s = String::new();
    for c in e[56..128].chunks(2) {
        let u = u16::from_le_bytes([c[0], c[1]]);
        if u == 0 {
            break;
        }
        s.push(char::from_u32(u as u32).unwrap_or('?'));
    }
    s
}

fn used(e: &[u8]) -> bool {
    e[..16].iter().any(|&b| b != 0)
}

pub fn find(d: &mut dyn Sectors, name: &str) -> Result<Found, GptError> {
    let t = load(d)?;
    for i in 0..t.num {
        let e = &t.entries[i * t.size..(i + 1) * t.size];
        if used(e) && name_of(e) == name {
            let (first, last) = (u64_at(e, 32), u64_at(e, 40));
            if last < first {
                return Err(GptError::Corrupt("partition ends before it starts"));
            }
            return Ok(Found { index: i, first, last });
        }
    }
    Err(GptError::NoPartition)
}

pub fn room(d: &mut dyn Sectors, name: &str) -> Result<u64, GptError> {
    let t = load(d)?;
    let f = find_in(&t, name)?;
    Ok(limit(&t, &f, d.sectors()))
}

fn find_in(t: &Table, name: &str) -> Result<Found, GptError> {
    for i in 0..t.num {
        let e = &t.entries[i * t.size..(i + 1) * t.size];
        if used(e) && name_of(e) == name {
            return Ok(Found { index: i, first: u64_at(e, 32), last: u64_at(e, 40) });
        }
    }
    Err(GptError::NoPartition)
}

fn limit(t: &Table, f: &Found, disk_sectors: u64) -> u64 {
    let entry_sectors = (t.num * t.size).div_ceil(SECTOR) as u64;
    let last_usable = disk_sectors.saturating_sub(2 + entry_sectors);
    let mut lim = last_usable;
    for i in 0..t.num {
        if i == f.index {
            continue;
        }
        let e = &t.entries[i * t.size..(i + 1) * t.size];
        if used(e) && u64_at(e, 32) > f.first {
            lim = lim.min(u64_at(e, 32) - 1);
        }
    }
    lim
}

pub fn grow(d: &mut dyn Sectors, name: &str, want_last: Option<u64>) -> Result<u64, GptError> {
    let mut t = load(d)?;
    let disk = d.sectors();
    let f = find_in(&t, name)?;
    let lim = limit(&t, &f, disk);
    let new_last = want_last.map_or(lim, |w| w.min(lim));
    if new_last <= f.last {
        return Err(GptError::NoRoom);
    }
    if lim <= f.last {
        return Err(GptError::NoRoom);
    }
    let hsize = u32_at(&t.header, 12) as usize;
    let entry_sectors = (t.num * t.size).div_ceil(SECTOR) as u64;
    let disk_last = disk - 1;
    let old_alt = u64_at(&t.header, 32);
    if old_alt > disk_last {
        return Err(GptError::Corrupt("backup GPT is beyond the end of the disk"));
    }

    put64(&mut t.entries[f.index * t.size..], 40, new_last);
    let array_crc = crc32(&t.entries);
    let backup_entries_lba = disk_last - entry_sectors;
    let mut primary = t.header;
    put64(&mut primary, 32, disk_last);
    put64(&mut primary, 48, backup_entries_lba - 1);
    put32(&mut primary, 88, array_crc);
    put32(&mut primary, 16, 0);
    let c = crc32(&primary[..hsize]);
    put32(&mut primary, 16, c);
    let mut backup = primary;
    put64(&mut backup, 24, disk_last);
    put64(&mut backup, 32, 1);
    put64(&mut backup, 72, backup_entries_lba);
    put32(&mut backup, 16, 0);
    let c = crc32(&backup[..hsize]);
    put32(&mut backup, 16, c);

    let mut padded = t.entries.clone();
    padded.resize(entry_sectors as usize * SECTOR, 0);
    d.write(backup_entries_lba, &padded).map_err(|_| GptError::Io)?;
    d.write(disk_last, &backup).map_err(|_| GptError::Io)?;
    d.write(t.entries_lba, &padded).map_err(|_| GptError::Io)?;
    d.write(1, &primary).map_err(|_| GptError::Io)?;

    if old_alt != disk_last {
        let from = old_alt - entry_sectors;
        let end = old_alt.min(backup_entries_lba - 1);
        if from <= end {
            let zeros = alloc::vec![0u8; (end - from + 1) as usize * SECTOR];
            d.write(from, &zeros).map_err(|_| GptError::Io)?;
        }
    }
    d.flush().map_err(|_| GptError::Io)?;
    Ok(new_last)
}
