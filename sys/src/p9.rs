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

use alloc::string::String;
use alloc::vec::Vec;

pub const DMDIR: u32 = 0x8000_0000;
pub const QTDIR: u8 = 0x80;
pub const QTFILE: u8 = 0x00;

pub const STATFIXLEN: usize = 2 + 13 + 5 * 2 + 4 * 4 + 8;

#[derive(Clone, Debug)]
pub struct Stat {
    pub name: String,
    pub is_dir: bool,
    pub perm: u32,
    pub length: u64,
    pub mtime: u32,
    pub qid_path: u64,
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put64(v: &mut Vec<u8>, x: u64) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn puts(v: &mut Vec<u8>, s: &str) {
    put16(v, s.len() as u16);
    v.extend_from_slice(s.as_bytes());
}

const WHO: &str = "none";

pub fn pack_stat(st: &Stat) -> Vec<u8> {
    let mut v = Vec::with_capacity(STATFIXLEN + st.name.len() + 3 * WHO.len() + 8);
    put16(&mut v, 0);
    put16(&mut v, 0);
    put32(&mut v, 0);
    v.push(if st.is_dir { QTDIR } else { QTFILE });
    put32(&mut v, 0);
    put64(&mut v, st.qid_path);
    let mode = (st.perm & 0o777) | if st.is_dir { DMDIR } else { 0 };
    put32(&mut v, mode);
    put32(&mut v, st.mtime);
    put32(&mut v, st.mtime);
    put64(&mut v, st.length);
    puts(&mut v, &st.name);
    puts(&mut v, WHO);
    puts(&mut v, WHO);
    puts(&mut v, WHO);
    let size = (v.len() - 2) as u16;
    v[0..2].copy_from_slice(&size.to_le_bytes());
    v
}

pub fn hash_path(p: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in p.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn normalize(cwd: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !path.starts_with('/') {
        for c in cwd.split('/') {
            push_component(&mut parts, c);
        }
    }
    for c in path.split('/') {
        push_component(&mut parts, c);
    }
    let mut out = String::new();
    for p in &parts {
        out.push('/');
        out.push_str(p);
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

fn push_component<'a>(parts: &mut Vec<&'a str>, c: &'a str) {
    match c {
        "" | "." => {}
        ".." => {
            parts.pop();
        }
        _ => parts.push(c),
    }
}

pub fn base_name(path: &str) -> &str {
    match path.rsplit('/').next() {
        Some("") | None => "/",
        Some(n) => n,
    }
}

pub fn errno_msg(e: i32) -> &'static str {
    match e {
        2 => "file does not exist",
        13 | 1 => "permission denied",
        17 => "file already exists",
        20 => "not a directory",
        21 => "is a directory",
        22 => "bad arg in system call",
        28 => "file system full",
        39 => "directory not empty",
        _ => "i/o error",
    }
}
