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

extern crate alloc;
use alloc::vec::Vec;

const SB: usize = 1024;
const MAGIC: u16 = 0xEF53;

const COMPAT_RESIZE_INODE: u32 = 0x10;
const COMPAT_SPARSE_SUPER2: u32 = 0x200;
const COMPAT_FAST_COMMIT: u32 = 0x400;
const INCOMPAT_OK: u32 = 0x2 | 0x40 | 0x200 | 0x400 | 0x4000;
const RO_SPARSE_SUPER: u32 = 0x1;
const RO_OK: u32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x20 | 0x40;
const STATE_ERROR: u16 = 0x2;
const DESC_SIZE: usize = 32;
const RESIZE_INO: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowError {
    BadMagic,
    Unsupported(&'static str),
    NotLarger,
    TooBig { max_blocks: u64 },
    BufferTooSmall,
    Corrupt(&'static str),
    Io,
}

impl GrowError {
    pub fn message(self) -> &'static str {
        match self {
            GrowError::BadMagic => "not an ext4 file system",
            GrowError::Unsupported(w) => w,
            GrowError::NotLarger => "new size is not larger than the current one",
            GrowError::TooBig { .. } => "larger than the group descriptor table allows",
            GrowError::BufferTooSmall => "internal: buffer too small",
            GrowError::Corrupt(w) => w,
            GrowError::Io => "disk read or write failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub block_size: u64,
    pub first_data_block: u64,
    pub blocks: u64,
    pub free_blocks: u64,
    pub blocks_per_group: u64,
    pub inodes_per_group: u64,
    pub inode_size: u64,
    pub groups: u64,
    pub gdt_blocks: u64,
    pub reserved_gdt_blocks: u64,
    pub max_blocks: u64,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn put16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn is_power(mut n: u64, base: u64) -> bool {
    if n == 0 {
        return false;
    }
    while n % base == 0 {
        n /= base;
    }
    n == 1
}

fn has_super(g: u64, sparse: bool) -> bool {
    !sparse || g <= 1 || is_power(g, 3) || is_power(g, 5) || is_power(g, 7)
}

fn div_up(a: u64, b: u64) -> u64 {
    a.div_ceil(b)
}

fn set_bit(buf: &mut [u8], i: u64) {
    buf[(i / 8) as usize] |= 1 << (i % 8);
}
fn clear_bit(buf: &mut [u8], i: u64) {
    buf[(i / 8) as usize] &= !(1 << (i % 8));
}
fn get_bit(buf: &[u8], i: u64) -> bool {
    buf[(i / 8) as usize] & (1 << (i % 8)) != 0
}

pub trait Io {
    fn read(&mut self, off: u64, buf: &mut [u8]) -> Result<(), GrowError>;
    fn write(&mut self, off: u64, buf: &[u8]) -> Result<(), GrowError>;
}

pub struct SliceIo<'a>(pub &'a mut [u8]);

impl Io for SliceIo<'_> {
    fn read(&mut self, off: u64, buf: &mut [u8]) -> Result<(), GrowError> {
        let o = usize::try_from(off).map_err(|_| GrowError::BufferTooSmall)?;
        let e = o.checked_add(buf.len()).filter(|&e| e <= self.0.len()).ok_or(GrowError::BufferTooSmall)?;
        buf.copy_from_slice(&self.0[o..e]);
        Ok(())
    }
    fn write(&mut self, off: u64, buf: &[u8]) -> Result<(), GrowError> {
        let o = usize::try_from(off).map_err(|_| GrowError::BufferTooSmall)?;
        let e = o.checked_add(buf.len()).filter(|&e| e <= self.0.len()).ok_or(GrowError::BufferTooSmall)?;
        self.0[o..e].copy_from_slice(buf);
        Ok(())
    }
}

fn read_vec(io: &mut dyn Io, off: u64, len: usize) -> Result<Vec<u8>, GrowError> {
    let mut v = alloc::vec![0u8; len];
    io.read(off, &mut v)?;
    Ok(v)
}

fn zero_range(io: &mut dyn Io, mut off: u64, mut len: u64) -> Result<(), GrowError> {
    let zeros = alloc::vec![0u8; 65536];
    while len > 0 {
        let n = len.min(zeros.len() as u64) as usize;
        io.write(off, &zeros[..n])?;
        off += n as u64;
        len -= n as u64;
    }
    Ok(())
}

pub fn inspect(img: &[u8]) -> Result<Geometry, GrowError> {
    if img.len() < SB + 1024 {
        return Err(GrowError::BufferTooSmall);
    }
    inspect_sb(&img[SB..SB + 1024])
}

pub fn inspect_io(io: &mut dyn Io) -> Result<Geometry, GrowError> {
    let sb = read_vec(io, SB as u64, 1024)?;
    inspect_sb(&sb)
}

pub fn inspect_sb(sb: &[u8]) -> Result<Geometry, GrowError> {
    if le16(sb, 56) != MAGIC {
        return Err(GrowError::BadMagic);
    }
    let compat = le32(sb, 92);
    let incompat = le32(sb, 96);
    let ro = le32(sb, 100);
    if le32(sb, 76) == 0 {
        return Err(GrowError::Unsupported("ext2 revision 0 is not supported"));
    }
    if compat & (COMPAT_SPARSE_SUPER2 | COMPAT_FAST_COMMIT) != 0 {
        return Err(GrowError::Unsupported("sparse_super2/fast_commit is not supported"));
    }
    if incompat & !INCOMPAT_OK != 0 {
        return Err(GrowError::Unsupported("file system uses an unsupported incompat feature (64bit, meta_bg, needs_recovery, ...)"));
    }
    if ro & !RO_OK != 0 {
        return Err(GrowError::Unsupported("file system uses an unsupported ro_compat feature (metadata_csum, uninit_bg, bigalloc, ...)"));
    }
    if le16(sb, 58) & STATE_ERROR != 0 {
        return Err(GrowError::Unsupported("file system has errors; refusing to resize"));
    }
    let log = le32(sb, 24);
    if log > 6 {
        return Err(GrowError::Corrupt("bad block size"));
    }
    let block_size = 1024u64 << log;
    let first = le32(sb, 20) as u64;
    let blocks = le32(sb, 4) as u64;
    let bpg = le32(sb, 32) as u64;
    let ipg = le32(sb, 40) as u64;
    let isz = le16(sb, 88) as u64;
    if bpg == 0 || ipg == 0 || isz < 128 || bpg > block_size * 8 || ipg > block_size * 8 || blocks <= first {
        return Err(GrowError::Corrupt("bad geometry"));
    }
    let groups = div_up(blocks - first, bpg);
    let dpb = block_size / DESC_SIZE as u64;
    let gdt_blocks = div_up(groups, dpb);
    let rsv = if compat & COMPAT_RESIZE_INODE != 0 { le16(sb, 206) as u64 } else { 0 };
    let max_blocks = (first + (gdt_blocks + rsv) * dpb * bpg).min(u32::MAX as u64);
    Ok(Geometry {
        block_size,
        first_data_block: first,
        blocks,
        free_blocks: le32(sb, 12) as u64,
        blocks_per_group: bpg,
        inodes_per_group: ipg,
        inode_size: isz,
        groups,
        gdt_blocks,
        reserved_gdt_blocks: rsv,
        max_blocks,
    })
}

struct Layout {
    bs: usize,
    first: u64,
    bpg: u64,
    ipg: u64,
    itb: u64,
    gdt: u64,
    rsv: u64,
    sparse: bool,
}

impl Layout {
    fn start(&self, g: u64) -> u64 {
        self.first + g * self.bpg
    }
    fn backup_overhead(&self, g: u64) -> u64 {
        if has_super(g, self.sparse) { 1 + self.gdt + self.rsv } else { 0 }
    }
    fn min_group(&self, g: u64) -> u64 {
        self.backup_overhead(g) + 2 + self.itb + 1
    }
}

pub fn grow(img: &mut [u8], new_blocks: u64) -> Result<u64, GrowError> {
    let g0 = inspect(img)?;
    if new_blocks > g0.blocks && new_blocks <= g0.max_blocks && (img.len() as u64) < new_blocks * g0.block_size {
        return Err(GrowError::BufferTooSmall);
    }
    grow_io(&mut SliceIo(img), new_blocks)
}

pub fn grow_io(io: &mut dyn Io, new_blocks: u64) -> Result<u64, GrowError> {
    let mut sb = read_vec(io, SB as u64, 1024)?;
    let g0 = inspect_sb(&sb)?;
    let bs = g0.block_size as usize;
    if new_blocks <= g0.blocks {
        return Err(GrowError::NotLarger);
    }
    if new_blocks > g0.max_blocks {
        return Err(GrowError::TooBig { max_blocks: g0.max_blocks });
    }
    let sparse = le32(&sb, 100) & RO_SPARSE_SUPER != 0;
    let mut lay = Layout {
        bs,
        first: g0.first_data_block,
        bpg: g0.blocks_per_group,
        ipg: g0.inodes_per_group,
        itb: div_up(g0.inodes_per_group * g0.inode_size, g0.block_size),
        gdt: g0.gdt_blocks,
        rsv: g0.reserved_gdt_blocks,
        sparse,
    };
    let bsu = g0.block_size;
    let dpb = bsu / DESC_SIZE as u64;

    let mut n1 = new_blocks;
    let mut groups1 = div_up(n1 - lay.first, lay.bpg);
    if groups1 > g0.groups {
        let last = groups1 - 1;
        let last_len = n1 - lay.start(last);
        if last_len < lay.min_group(last) {
            n1 -= last_len;
            groups1 -= 1;
        }
    }
    if n1 <= g0.blocks {
        return Err(GrowError::NotLarger);
    }

    let gdt_old = g0.gdt_blocks;
    let gdt_new = div_up(groups1, dpb).max(gdt_old);
    let extra = gdt_new - gdt_old;
    if extra > g0.reserved_gdt_blocks {
        return Err(GrowError::TooBig { max_blocks: g0.max_blocks });
    }

    let mut gdt = read_vec(io, (lay.first + 1) * bsu, gdt_old as usize * bs)?;
    gdt.resize(gdt_new as usize * bs, 0);
    let gd = |g: u64| g as usize * DESC_SIZE;

    let old_groups = g0.groups;

    let old_last = old_groups - 1;
    let old_len = g0.blocks - lay.start(old_last);
    if old_len < lay.bpg {
        let new_len = (n1 - lay.start(old_last)).min(lay.bpg);
        let d = gd(old_last);
        let bb = le32(&gdt, d) as u64;
        let mut bitmap = read_vec(io, bb * bsu, bs)?;
        for i in old_len..new_len {
            if !get_bit(&bitmap, i) {
                return Err(GrowError::Corrupt("last group's bitmap padding is not set"));
            }
        }
        for i in old_len..new_len {
            clear_bit(&mut bitmap, i);
        }
        io.write(bb * bsu, &bitmap)?;
        let free = le16(&gdt, d + 12) as u64 + (new_len - old_len);
        put16(&mut gdt, d + 12, free as u16);
    }

    for g in old_groups..groups1 {
        let start = lay.start(g);
        let len = (n1 - start).min(lay.bpg);
        let pos = start + lay.backup_overhead(g);
        let (bb, ib, it) = (pos, pos + 1, pos + 2);
        let data_start = it + lay.itb;
        if data_start >= start + len {
            return Err(GrowError::Corrupt("new group too small"));
        }
        zero_range(io, start * bsu, (data_start - start) * bsu)?;
        let mut bm = alloc::vec![0u8; bs];
        for i in 0..data_start - start {
            set_bit(&mut bm, i);
        }
        for i in len..bs as u64 * 8 {
            set_bit(&mut bm, i);
        }
        io.write(bb * bsu, &bm)?;
        let mut im = alloc::vec![0u8; bs];
        for i in lay.ipg..bs as u64 * 8 {
            set_bit(&mut im, i);
        }
        io.write(ib * bsu, &im)?;
        let d = gd(g);
        gdt[d..d + DESC_SIZE].fill(0);
        put32(&mut gdt, d, bb as u32);
        put32(&mut gdt, d + 4, ib as u32);
        put32(&mut gdt, d + 8, it as u32);
        put16(&mut gdt, d + 12, (len - (data_start - start)) as u16);
        put16(&mut gdt, d + 14, lay.ipg as u16);
    }

    let (mut free_blocks, mut free_inodes) = (0u64, 0u64);
    for g in 0..groups1 {
        free_blocks += le16(&gdt, gd(g) + 12) as u64;
        free_inodes += le16(&gdt, gd(g) + 14) as u64;
    }
    let old_blocks = g0.blocks;
    let reserved = le32(&sb, 8) as u64;
    put32(&mut sb, 4, n1 as u32);
    put32(&mut sb, 0, (groups1 * lay.ipg) as u32);
    put32(&mut sb, 8, (reserved * n1 / old_blocks) as u32);
    put32(&mut sb, 12, free_blocks as u32);
    put32(&mut sb, 16, free_inodes as u32);

    if lay.rsv > 0 {
        let isz = le16(&sb, 88) as usize;
        let itable = le32(&gdt, 8) as u64;
        let ino_off = itable * bsu + ((RESIZE_INO - 1) * isz) as u64;
        let mut ino = read_vec(io, ino_off, isz)?;
        let dind = le32(&ino, 40 + 13 * 4) as u64;
        if dind == 0 {
            return Err(GrowError::Corrupt("resize inode has no double indirect block"));
        }
        let mut dind_blk = read_vec(io, dind * bsu, bs)?;
        let apb = bsu / 4;
        let mut i_blocks = le32(&ino, 28);
        for i in 0..extra {
            let gdb_num = gdt_old + i;
            let gdb_blk = lay.first + 1 + gdb_num;
            let slot = (gdb_num % apb) as usize * 4;
            if le32(&dind_blk, slot) as u64 != gdb_blk {
                return Err(GrowError::Corrupt("resize inode does not match reserved descriptor blocks"));
            }
            let list = read_vec(io, gdb_blk * bsu, bs)?;
            let mut backups = 0u32;
            let mut last = 0u64;
            for g in 1..old_groups {
                if !has_super(g, sparse) {
                    continue;
                }
                let at = last as usize * 4;
                let v = le32(&list, at);
                if v == 0 {
                    break;
                }
                if v as u64 != gdb_blk + g * lay.bpg {
                    return Err(GrowError::Corrupt("resize inode backup list is inconsistent"));
                }
                backups += 1;
                last += 1;
            }
            put32(&mut dind_blk, slot, 0);
            i_blocks = i_blocks.saturating_sub((backups + 1) * (bs as u32 / 512));
        }
        put32(&mut ino, 28, i_blocks);
        io.write(ino_off, &ino)?;
        io.write(dind * bsu, &dind_blk)?;
        lay.gdt = gdt_new;
        lay.rsv -= extra;
        put16(&mut sb, 206, lay.rsv as u16);
        extend_resize_inode(io, &lay, groups1, &gdt)?;
    } else {
        lay.gdt = gdt_new;
    }

    for g in 1..groups1 {
        if !has_super(g, sparse) {
            continue;
        }
        let start = lay.start(g) * bsu;
        let mut copy = sb.clone();
        put16(&mut copy, 90, g as u16);
        io.write(start, &copy)?;
        io.write(start + bsu, &gdt)?;
    }
    io.write((lay.first + 1) * bsu, &gdt)?;
    io.write(SB as u64, &sb)?;
    Ok(n1)
}

fn extend_resize_inode(io: &mut dyn Io, lay: &Layout, groups: u64, gdt: &[u8]) -> Result<(), GrowError> {
    let bs = lay.bs;
    let bsu = bs as u64;
    let isz = {
        let sb = read_vec(io, SB as u64, 1024)?;
        le16(&sb, 88) as u64
    };
    let ino_off = le32(gdt, 8) as u64 * bsu + (RESIZE_INO as u64 - 1) * isz;
    let mut ino = read_vec(io, ino_off, isz as usize)?;
    let dind = le32(&ino, 40 + 13 * 4) as u64;
    if dind == 0 {
        return Err(GrowError::Corrupt("resize inode has no double indirect block"));
    }
    let dind_blk = read_vec(io, dind * bsu, bs)?;
    let apb = bsu / 4;
    let mut added = 0u32;
    for rsv_off in 0..lay.rsv {
        let gdt_off = (lay.gdt + rsv_off) % apb;
        let gdt_blk = lay.first + 1 + lay.gdt + rsv_off;
        if le32(&dind_blk, gdt_off as usize * 4) as u64 != gdt_blk {
            return Err(GrowError::Corrupt("resize inode does not match reserved descriptor blocks"));
        }
        let mut list = read_vec(io, gdt_blk * bsu, bs)?;
        let mut last = 0usize;
        let mut dirty = false;
        for g in 1..groups {
            if !has_super(g, lay.sparse) {
                continue;
            }
            let expect = (gdt_blk + g * lay.bpg) as u32;
            let at = last * 4;
            match le32(&list, at) {
                0 => {
                    put32(&mut list, at, expect);
                    added += 1;
                    dirty = true;
                }
                v if v == expect => {}
                _ => return Err(GrowError::Corrupt("resize inode backup list is inconsistent")),
            }
            last += 1;
        }
        if dirty {
            io.write(gdt_blk * bsu, &list)?;
        }
    }
    if added > 0 {
        let blocks = le32(&ino, 28) + added * (bs as u32 / 512);
        put32(&mut ino, 28, blocks);
        io.write(ino_off, &ino)?;
    }
    Ok(())
}
