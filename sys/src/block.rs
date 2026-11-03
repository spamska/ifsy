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

#![allow(dead_code)]

use core::cmp::min;

pub const SECTOR_SIZE: usize = 512;
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

const EIO: i32 = 5;
const EINVAL: i32 = 22;
const ENOSPC: i32 = 28;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockError {
    OutOfRange,
    BadBufferLength,
    Io,
}

impl BlockError {
    pub const fn errno(self) -> i32 {
        match self {
            BlockError::OutOfRange | BlockError::BadBufferLength => EINVAL,
            BlockError::Io => EIO,
        }
    }
}

pub trait BlockDevice {
    fn sector_count(&self) -> u64;
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
    fn flush(&mut self) -> Result<(), BlockError>;
    fn size_bytes(&self) -> u64 {
        self.sector_count().saturating_mul(SECTOR_SIZE as u64)
    }
}

fn byte_range(sector_count: u64, lba: u64, len: usize) -> Result<(usize, usize), BlockError> {
    if len % SECTOR_SIZE != 0 {
        return Err(BlockError::BadBufferLength);
    }
    let sectors = (len / SECTOR_SIZE) as u64;
    let end_lba = lba.checked_add(sectors).ok_or(BlockError::OutOfRange)?;
    if end_lba > sector_count {
        return Err(BlockError::OutOfRange);
    }
    let start = usize::try_from(lba)
        .ok()
        .and_then(|l| l.checked_mul(SECTOR_SIZE))
        .ok_or(BlockError::OutOfRange)?;
    let end = start.checked_add(len).ok_or(BlockError::OutOfRange)?;
    Ok((start, end))
}

pub struct RamDisk {
    data: &'static mut [u8],
}

impl RamDisk {
    pub unsafe fn from_raw(ptr: *mut u8, len: usize) -> Self {
        let usable = len - len % SECTOR_SIZE;
        Self { data: unsafe { core::slice::from_raw_parts_mut(ptr, usable) } }
    }
}

impl RamDisk {
    pub fn raw_parts(&self) -> (*mut u8, usize) {
        (self.data.as_ptr() as *mut u8, self.data.len())
    }

    pub fn bytes(&self) -> &[u8] {
        &*self.data
    }
}

impl BlockDevice for RamDisk {
    fn sector_count(&self) -> u64 {
        (self.data.len() / SECTOR_SIZE) as u64
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let (start, end) = byte_range(self.sector_count(), lba, buf.len())?;
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let (start, end) = byte_range(self.sector_count(), lba, buf.len())?;
        self.data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

pub struct PartDev;

impl BlockDevice for PartDev {
    fn sector_count(&self) -> u64 {
        crate::store::sectors()
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() % SECTOR_SIZE != 0 {
            return Err(BlockError::BadBufferLength);
        }
        crate::store::read_at(lba, buf).map_err(|_| BlockError::Io)
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() % SECTOR_SIZE != 0 {
            return Err(BlockError::BadBufferLength);
        }
        crate::store::write_at(lba, buf).map_err(|_| BlockError::Io)
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        crate::store::flush();
        Ok(())
    }
}

pub enum Dev {
    Ram(RamDisk),
    Part(PartDev),
}

impl BlockDevice for Dev {
    fn sector_count(&self) -> u64 {
        match self {
            Dev::Ram(d) => d.sector_count(),
            Dev::Part(d) => d.sector_count(),
        }
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        match self {
            Dev::Ram(d) => d.read_sectors(lba, buf),
            Dev::Part(d) => d.read_sectors(lba, buf),
        }
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        match self {
            Dev::Ram(d) => d.write_sectors(lba, buf),
            Dev::Part(d) => d.write_sectors(lba, buf),
        }
    }

    fn flush(&mut self) -> Result<(), BlockError> {
        match self {
            Dev::Ram(d) => d.flush(),
            Dev::Part(d) => d.flush(),
        }
    }
}

pub struct Disk<D: BlockDevice> {
    dev: D,
    pos: u64,
}

impl<D: BlockDevice> Disk<D> {
    pub fn new(dev: D) -> Self {
        Self { dev, pos: 0 }
    }

    pub fn size(&self) -> u64 {
        self.dev.size_bytes()
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    pub fn device(&self) -> &D {
        &self.dev
    }

    pub fn into_device(self) -> D {
        self.dev
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        let size = self.size();
        if self.pos >= size {
            return Ok(0);
        }
        let want = min(buf.len() as u64, size - self.pos) as usize;
        let buf = &mut buf[..want];

        let mut done = 0usize;
        while done < want {
            let lba = self.pos / SECTOR_SIZE as u64;
            let off = (self.pos % SECTOR_SIZE as u64) as usize;
            let left = want - done;

            if off == 0 && left >= SECTOR_SIZE {
                let whole = left - left % SECTOR_SIZE;
                self.dev.read_sectors(lba, &mut buf[done..done + whole]).map_err(BlockError::errno)?;
                done += whole;
                self.pos += whole as u64;
            } else {
                let mut bounce = [0u8; SECTOR_SIZE];
                self.dev.read_sectors(lba, &mut bounce).map_err(BlockError::errno)?;
                let n = min(SECTOR_SIZE - off, left);
                buf[done..done + n].copy_from_slice(&bounce[off..off + n]);
                done += n;
                self.pos += n as u64;
            }
        }
        Ok(done)
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        if buf.is_empty() {
            return Ok(0);
        }
        let size = self.size();
        if self.pos >= size {
            return Err(ENOSPC);
        }
        let want = min(buf.len() as u64, size - self.pos) as usize;
        let buf = &buf[..want];

        let mut done = 0usize;
        while done < want {
            let lba = self.pos / SECTOR_SIZE as u64;
            let off = (self.pos % SECTOR_SIZE as u64) as usize;
            let left = want - done;

            if off == 0 && left >= SECTOR_SIZE {
                let whole = left - left % SECTOR_SIZE;
                self.dev.write_sectors(lba, &buf[done..done + whole]).map_err(BlockError::errno)?;
                done += whole;
                self.pos += whole as u64;
            } else {
                let mut bounce = [0u8; SECTOR_SIZE];
                self.dev.read_sectors(lba, &mut bounce).map_err(BlockError::errno)?;
                let n = min(SECTOR_SIZE - off, left);
                bounce[off..off + n].copy_from_slice(&buf[done..done + n]);
                self.dev.write_sectors(lba, &bounce).map_err(BlockError::errno)?;
                done += n;
                self.pos += n as u64;
            }
        }
        Ok(done)
    }

    pub fn seek(&mut self, off: i64, whence: i32) -> Result<i64, i32> {
        let base: i64 = match whence {
            SEEK_SET => 0,
            SEEK_CUR => i64::try_from(self.pos).map_err(|_| EINVAL)?,
            SEEK_END => i64::try_from(self.size()).map_err(|_| EINVAL)?,
            _ => return Err(EINVAL),
        };
        let new = base.checked_add(off).ok_or(EINVAL)?;
        if new < 0 {
            return Err(EINVAL);
        }
        self.pos = new as u64;
        Ok(new)
    }

    pub fn flush(&mut self) -> Result<usize, i32> {
        self.dev.flush().map_err(BlockError::errno)?;
        Ok(0)
    }
}
