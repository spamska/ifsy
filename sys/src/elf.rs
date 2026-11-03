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

use alloc::vec::Vec;

use crate::mm::aspace::{AddressSpace, AsError, IMAGE_LO, IMAGE_MAX, Kind, USER_MIN};
use crate::mm::PageFlags;

const PAGE: u64 = 4096;
pub const IMAGE_LIMIT: u64 = 256 * 1024 * 1024;
const MAX_PHNUM: usize = 64;
const MAX_RELOCS: u64 = 1 << 20;

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;

pub use crate::arch::port::elf::{MACHINE, R_RELATIVE};

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
const PT_GNU_RELRO: u32 = 0x6474_e552;

const PF_X: u32 = 1;
const PF_W: u32 = 2;

const DT_NULL: u64 = 0;
const DT_NEEDED: u64 = 1;
const DT_PLTRELSZ: u64 = 2;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const DT_REL: u64 = 17;
const DT_RELSZ: u64 = 18;
const DT_PLTREL: u64 = 20;
const DT_JMPREL: u64 = 23;
const DT_RELR: u64 = 36;
const DT_RELRSZ: u64 = 35;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ElfError {
    Truncated,
    BadMagic,
    BadClass,
    BadType,
    BadMachine,
    BadPhdr,
    Dynamic,
    NoLoad,
    BadSegment,
    TooBig,
    BadAddress,
    WriteXorExec,
    BadEntry,
    BadReloc,
    NoMemory,
}

impl ElfError {
    pub fn msg(&self) -> &'static str {
        match self {
            ElfError::Truncated | ElfError::BadMagic | ElfError::BadClass | ElfError::BadType | ElfError::BadPhdr
            | ElfError::NoLoad | ElfError::BadSegment => "exec header invalid",
            ElfError::BadMachine => "exec: wrong architecture",
            ElfError::Dynamic => "exec: dynamic linking not supported",
            ElfError::TooBig => "exec: image too large",
            ElfError::BadAddress => "exec: segment outside the user layout",
            ElfError::WriteXorExec => "exec: segment would be writable and executable",
            ElfError::BadEntry => "exec: entry point not in executable memory",
            ElfError::BadReloc => "exec: unsupported relocation",
            ElfError::NoMemory => "exec: out of memory",
        }
    }
}

impl From<AsError> for ElfError {
    fn from(e: AsError) -> Self {
        match e {
            AsError::NoMemory => ElfError::NoMemory,
            AsError::Overlap => ElfError::BadSegment,
            _ => ElfError::BadAddress,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub entry: u64,
    pub bias: u64,
    pub low: u64,
    pub high: u64,
    pub phdr: u64,
    pub phnum: u16,
    pub pie: bool,
}

fn rd16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o.checked_add(2)?)?.try_into().ok()?))
}
fn rd32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o.checked_add(4)?)?.try_into().ok()?))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o.checked_add(8)?)?.try_into().ok()?))
}

fn down(v: u64) -> u64 {
    v & !(PAGE - 1)
}
fn up(v: u64) -> Option<u64> {
    v.checked_add(PAGE - 1).map(down)
}

#[derive(Clone, Copy)]
struct Phdr {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
}

fn phdrs(image: &[u8]) -> Result<(u16, Vec<Phdr>, u64, u16), ElfError> {
    if image.len() < 64 {
        return Err(ElfError::Truncated);
    }
    if image[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(ElfError::BadMagic);
    }
    if image[4] != 2 || image[5] != 1 || image[6] != 1 {
        return Err(ElfError::BadClass);
    }
    let etype = rd16(image, 16).ok_or(ElfError::Truncated)?;
    if etype != ET_EXEC && etype != ET_DYN {
        return Err(ElfError::BadType);
    }
    if rd16(image, 18).ok_or(ElfError::Truncated)? != MACHINE {
        return Err(ElfError::BadMachine);
    }
    if rd32(image, 20).ok_or(ElfError::Truncated)? != 1 {
        return Err(ElfError::BadClass);
    }
    let entry = rd64(image, 24).ok_or(ElfError::Truncated)?;
    let phoff = rd64(image, 32).ok_or(ElfError::Truncated)?;
    let phentsize = rd16(image, 54).ok_or(ElfError::Truncated)?;
    let phnum = rd16(image, 56).ok_or(ElfError::Truncated)?;
    if phentsize != 56 || phnum == 0 || phnum as usize > MAX_PHNUM {
        return Err(ElfError::BadPhdr);
    }
    let table_end = phoff.checked_add(56 * u64::from(phnum)).ok_or(ElfError::BadPhdr)?;
    if table_end > image.len() as u64 {
        return Err(ElfError::Truncated);
    }
    let mut v = Vec::with_capacity(phnum as usize);
    for i in 0..phnum as usize {
        let o = phoff as usize + 56 * i;
        v.push(Phdr {
            kind: rd32(image, o).ok_or(ElfError::Truncated)?,
            flags: rd32(image, o + 4).ok_or(ElfError::Truncated)?,
            offset: rd64(image, o + 8).ok_or(ElfError::Truncated)?,
            vaddr: rd64(image, o + 16).ok_or(ElfError::Truncated)?,
            filesz: rd64(image, o + 32).ok_or(ElfError::Truncated)?,
            memsz: rd64(image, o + 40).ok_or(ElfError::Truncated)?,
        });
    }
    Ok((etype, v, entry, phnum))
}

const P_MAP: u8 = 1;
const P_W: u8 = 2;
const P_X: u8 = 4;

fn final_flags(perm: u8) -> PageFlags {
    if perm & P_X != 0 {
        PageFlags::USER_EXEC
    } else if perm & P_W != 0 {
        PageFlags::USER_RW
    } else {
        PageFlags::USER_RO
    }
}

pub fn load(a: &mut AddressSpace, image: &[u8]) -> Result<Loaded, ElfError> {
    let (etype, headers, entry, phnum) = phdrs(image)?;

    if headers.iter().any(|h| h.kind == PT_INTERP) {
        return Err(ElfError::Dynamic);
    }

    let mut segs: Vec<Phdr> = headers.iter().copied().filter(|h| h.kind == PT_LOAD && h.memsz != 0).collect();
    if segs.is_empty() {
        return Err(ElfError::NoLoad);
    }
    for s in &segs {
        let file_end = s.offset.checked_add(s.filesz).ok_or(ElfError::BadSegment)?;
        if s.filesz > s.memsz || file_end > image.len() as u64 {
            return Err(ElfError::BadSegment);
        }
        s.vaddr.checked_add(s.memsz).ok_or(ElfError::BadSegment)?;
        if s.flags & PF_W != 0 && s.flags & PF_X != 0 {
            return Err(ElfError::WriteXorExec);
        }
    }
    segs.sort_by_key(|s| s.vaddr);
    for w in segs.windows(2) {
        if w[0].vaddr + w[0].memsz > w[1].vaddr {
            return Err(ElfError::BadSegment);
        }
    }

    let lo = down(segs[0].vaddr);
    let last = segs[segs.len() - 1];
    let hi = up(last.vaddr + last.memsz).ok_or(ElfError::BadSegment)?;
    let span = hi - lo;
    if span > IMAGE_LIMIT.min(IMAGE_MAX) {
        return Err(ElfError::TooBig);
    }

    let pie = etype == ET_DYN;
    let bias = if pie {
        a.layout().image_base.wrapping_sub(lo)
    } else {
        if lo < USER_MIN || hi > IMAGE_LO + IMAGE_MAX {
            return Err(ElfError::BadAddress);
        }
        0
    };
    let base = lo.wrapping_add(bias);
    base.checked_add(span).ok_or(ElfError::BadAddress)?;

    let pages = (span / PAGE) as usize;
    let mut perm = alloc::vec![0u8; pages];
    for s in &segs {
        let first = ((down(s.vaddr) - lo) / PAGE) as usize;
        let end = ((up(s.vaddr + s.memsz).ok_or(ElfError::BadSegment)? - lo) / PAGE) as usize;
        let bits = P_MAP | if s.flags & PF_W != 0 { P_W } else { 0 } | if s.flags & PF_X != 0 { P_X } else { 0 };
        for p in &mut perm[first..end] {
            *p |= bits;
            if *p & P_W != 0 && *p & P_X != 0 {
                return Err(ElfError::WriteXorExec);
            }
        }
    }

    let mut i = 0;
    while i < pages {
        if perm[i] & P_MAP == 0 {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < pages && perm[j] == perm[i] {
            j += 1;
        }
        a.map_region(base + i as u64 * PAGE, (j - i) as u64 * PAGE, PageFlags::USER_RW, Kind::Image)?;
        i = j;
    }

    for s in &segs {
        if s.filesz != 0 {
            let src = &image[s.offset as usize..(s.offset + s.filesz) as usize];
            a.write_bytes(s.vaddr.wrapping_add(bias), src)?;
        }
    }

    if let Some(d) = headers.iter().find(|h| h.kind == PT_DYNAMIC) {
        apply_relocs(a, image, d, bias)?;
    }

    let mut i = 0;
    while i < pages {
        if perm[i] & P_MAP == 0 {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < pages && perm[j] == perm[i] {
            j += 1;
        }
        if perm[i] & P_W == 0 {
            a.protect(base + i as u64 * PAGE, (j - i) as u64 * PAGE, final_flags(perm[i]))?;
        }
        i = j;
    }

    if let Some(r) = headers.iter().find(|h| h.kind == PT_GNU_RELRO && h.memsz != 0) {
        let s = down(r.vaddr.wrapping_add(bias));
        let e = down(r.vaddr.wrapping_add(bias).wrapping_add(r.memsz));
        if e > s && s >= base && e <= base + span {
            let mut va = s;
            while va < e {
                let idx = ((va - base) / PAGE) as usize;
                if perm[idx] & P_MAP != 0 && perm[idx] & P_X == 0 {
                    a.protect(va, PAGE, PageFlags::USER_RO)?;
                }
                va += PAGE;
            }
        }
    }

    let entry = entry.wrapping_add(bias);
    if entry < base || entry >= base + span || perm[((down(entry) - base) / PAGE) as usize] & P_X == 0 {
        return Err(ElfError::BadEntry);
    }

    let phdr = headers
        .iter()
        .find(|h| h.kind == PT_PHDR)
        .map(|h| h.vaddr.wrapping_add(bias))
        .or_else(|| {
            let off = rd64(image, 32)?;
            segs.iter().find(|s| off >= s.offset && off < s.offset + s.filesz).map(|s| s.vaddr + (off - s.offset) + bias)
        })
        .unwrap_or(0);

    Ok(Loaded { entry, bias, low: base, high: base + span, phdr, phnum, pie })
}

fn apply_relocs(a: &mut AddressSpace, image: &[u8], dynamic: &Phdr, bias: u64) -> Result<(), ElfError> {
    let end = dynamic.offset.checked_add(dynamic.filesz).ok_or(ElfError::BadSegment)?;
    if end > image.len() as u64 {
        return Err(ElfError::BadSegment);
    }
    let (mut rela, mut relasz, mut relaent) = (0u64, 0u64, 24u64);
    let (mut jmprel, mut pltrelsz, mut pltrel) = (0u64, 0u64, DT_RELA);
    let mut o = dynamic.offset as usize;
    while o + 16 <= end as usize {
        let tag = rd64(image, o).ok_or(ElfError::Truncated)?;
        let val = rd64(image, o + 8).ok_or(ElfError::Truncated)?;
        match tag {
            DT_NULL => break,
            DT_NEEDED => return Err(ElfError::Dynamic),
            DT_REL | DT_RELSZ | DT_RELR | DT_RELRSZ if val != 0 => return Err(ElfError::BadReloc),
            DT_RELA => rela = val,
            DT_RELASZ => relasz = val,
            DT_RELAENT => relaent = val,
            DT_JMPREL => jmprel = val,
            DT_PLTRELSZ => pltrelsz = val,
            DT_PLTREL => pltrel = val,
            _ => {}
        }
        o += 16;
    }
    if relaent != 24 {
        return Err(ElfError::BadReloc);
    }
    if pltrelsz != 0 && pltrel != DT_RELA {
        return Err(ElfError::BadReloc);
    }
    for (table, size) in [(rela, relasz), (jmprel, pltrelsz)] {
        if size == 0 {
            continue;
        }
        if table == 0 || size % 24 != 0 || size / 24 > MAX_RELOCS {
            return Err(ElfError::BadReloc);
        }
        let mut buf = alloc::vec![0u8; size as usize];
        a.read_bytes(table.wrapping_add(bias), &mut buf).map_err(|_| ElfError::BadReloc)?;
        for r in buf.chunks_exact(24) {
            let offset = u64::from_le_bytes(r[0..8].try_into().unwrap());
            let info = u64::from_le_bytes(r[8..16].try_into().unwrap());
            let addend = i64::from_le_bytes(r[16..24].try_into().unwrap());
            let (ty, sym) = (info as u32, (info >> 32) as u32);
            match (ty, sym) {
                (0, _) => {}
                (t, 0) if t == R_RELATIVE => {
                    let value = bias.wrapping_add(addend as u64);
                    a.write_bytes(offset.wrapping_add(bias), &value.to_le_bytes()).map_err(|_| ElfError::BadReloc)?;
                }
                _ => return Err(ElfError::BadReloc),
            }
        }
    }
    Ok(())
}

pub const MAX_ARGS: usize = 256;
pub const MAX_ARG_BYTES: usize = 128 * 1024;
pub fn build_stack(a: &AddressSpace, top: u64, args: &[Vec<u8>]) -> Result<(u64, u64), AsError> {
    let mut cur = top;
    let mut addrs: Vec<u64> = Vec::with_capacity(args.len());
    for arg in args.iter().rev() {
        cur = cur.checked_sub(arg.len() as u64 + 1).ok_or(AsError::OutOfRange)?;
        a.write_bytes(cur, arg)?;
        a.write_bytes(cur + arg.len() as u64, &[0])?;
        addrs.push(cur);
    }
    addrs.reverse();
    cur &= !15;
    let words = args.len() as u64 + 2;
    let sp = cur.checked_sub((words * 8 + 15) & !15).ok_or(AsError::OutOfRange)?;

    a.write_bytes(sp, &(args.len() as u64).to_le_bytes())?;
    for (i, p) in addrs.iter().enumerate() {
        a.write_bytes(sp + 8 + 8 * i as u64, &p.to_le_bytes())?;
    }
    a.write_bytes(sp + 8 + 8 * args.len() as u64, &0u64.to_le_bytes())?;
    Ok((sp, sp + 8))
}
