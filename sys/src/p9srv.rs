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

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_char;

use p9fcall::{
    CreateBody, Dir, ErrorBody, Fcall, IoBody, OpenedBody, Qid, StatBody, VersionBody, WalkBody,
    WalkQidBody, DMDIR, IOHDRSZ, MAXWELEM, QTDIR, QTFILE,
};

use crate::io::{self, Chan, Pipe};
use crate::sync::SpinLock;

struct Node {
    name: String,
    dir: bool,
    data: Vec<u8>,
    kids: Vec<usize>,
    parent: usize,
    perm: u32,
}

struct Srv {
    nodes: Vec<Option<Node>>,
    fids: BTreeMap<u32, usize>,
    msize: u32,
    inbuf: Vec<u8>,
}

static SRV: SpinLock<Option<Srv>> = SpinLock::new(None);

fn mknode(name: &str, dir: bool, data: &[u8], parent: usize, perm: u32) -> Node {
    Node { name: String::from(name), dir, data: data.to_vec(), kids: Vec::new(), parent, perm }
}

fn fresh() -> Srv {
    let mut nodes: Vec<Option<Node>> = Vec::new();
    nodes.push(Some(mknode("/", true, &[], 0, 0o755)));
    nodes.push(Some(mknode("hello", false, b"hello from 9p\n", 0, 0o644)));
    nodes.push(Some(mknode("sub", true, &[], 0, 0o755)));
    nodes.push(Some(mknode("deep", false, b"deep file", 2, 0o644)));
    nodes[0].as_mut().unwrap().kids = vec![1, 2];
    nodes[2].as_mut().unwrap().kids = vec![3];
    Srv { nodes, fids: BTreeMap::new(), msize: 0, inbuf: Vec::new() }
}

pub fn client() -> Chan {
    SRV.with(|s| *s = Some(fresh()));
    io::hooked_pipe(serve)
}

pub fn node_count() -> usize {
    SRV.with(|s| s.as_ref().map_or(0, |s| s.nodes.iter().flatten().count()))
}

fn qid_of(srv: &Srv, n: usize) -> Qid {
    let dir = srv.nodes[n].as_ref().is_some_and(|x| x.dir);
    Qid { path: n as u64 + 1, vers: 0, typ: if dir { QTDIR } else { QTFILE } }
}

fn nul(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn stat_blob(srv: &Srv, n: usize) -> Vec<u8> {
    let node = srv.nodes[n].as_ref().unwrap();
    let name = nul(&node.name);
    let who = nul("none");
    let mut d = Dir::new();
    d.qid = qid_of(srv, n);
    d.mode = node.perm | if node.dir { DMDIR } else { 0 };
    d.length = if node.dir { 0 } else { node.data.len() as i64 };
    d.name = name.as_ptr() as *const c_char;
    d.uid = who.as_ptr() as *const c_char;
    d.gid = who.as_ptr() as *const c_char;
    d.muid = who.as_ptr() as *const c_char;
    let sz = p9fcall::size_d2m(&d);
    let mut b = vec![0u8; sz];
    let k = p9fcall::conv_d2m(&d, &mut b);
    b.truncate(k);
    b
}

fn encode(f: &Fcall) -> Vec<u8> {
    let n = p9fcall::size_s2m(f);
    let mut b = vec![0u8; n];
    let k = p9fcall::conv_s2m(f, &mut b);
    b.truncate(k);
    b
}

fn rerror(tag: u16, msg: &str) -> Vec<u8> {
    let e = nul(msg);
    let mut f = Fcall::new(p9fcall::RERROR);
    f.tag = tag;
    f.body.error = ErrorBody { ename: e.as_ptr() as *const c_char };
    encode(&f)
}

fn simple(typ: u8, tag: u16) -> Vec<u8> {
    let mut f = Fcall::new(typ);
    f.tag = tag;
    encode(&f)
}

fn handle(srv: &mut Srv, msg: &mut [u8]) -> Vec<u8> {
    let mut f = Fcall::new(0);
    if p9fcall::conv_m2s(msg, &mut f) == 0 {
        return rerror(p9fcall::NOTAG, "bad message");
    }
    let tag = f.tag;
    // SAFETY: FFI/union access: the C call gets valid NUL-terminated or sized buffers owned by this frame, and the union field read matches the message type just checked.
    unsafe {
        match f.typ {
            p9fcall::TVERSION => {
                let m = f.body.version.msize.min(8192 + IOHDRSZ as u32);
                srv.msize = m;
                srv.fids.clear();
                let ver = nul(if p9fcall::cstr(f.body.version.version).starts_with("9P2000") { "9P2000" } else { "unknown" });
                let mut r = Fcall::new(p9fcall::RVERSION);
                r.tag = tag;
                r.body.version = VersionBody { msize: m, version: ver.as_ptr() as *const c_char };
                encode(&r)
            }
            p9fcall::TAUTH => rerror(tag, "no authentication required"),
            p9fcall::TATTACH => {
                srv.fids.insert(f.fid, 0);
                let mut r = Fcall::new(p9fcall::RATTACH);
                r.tag = tag;
                r.body.opened = OpenedBody { qid: qid_of(srv, 0), iounit: 0 };
                encode(&r)
            }
            p9fcall::TFLUSH => simple(p9fcall::RFLUSH, tag),
            p9fcall::TWALK => {
                let Some(&start) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let w = f.body.walk;
                let mut cur = start;
                let mut qids: Vec<Qid> = Vec::new();
                for i in 0..w.nwname as usize {
                    let name = p9fcall::cstr(w.wname[i]);
                    let node = srv.nodes[cur].as_ref().unwrap();
                    let next = if name == ".." {
                        Some(node.parent)
                    } else if node.dir {
                        node.kids.iter().copied().find(|k| srv.nodes[*k].as_ref().is_some_and(|x| x.name == name))
                    } else {
                        None
                    };
                    match next {
                        Some(n) => {
                            cur = n;
                            qids.push(qid_of(srv, n));
                        }
                        None => {
                            if i == 0 {
                                return rerror(tag, "file does not exist");
                            }
                            break;
                        }
                    }
                }
                if qids.len() == w.nwname as usize {
                    srv.fids.insert(w.newfid, cur);
                }
                let mut r = Fcall::new(p9fcall::RWALK);
                r.tag = tag;
                let mut wq = WalkQidBody { nwqid: qids.len() as u16, wqid: [Qid::default(); MAXWELEM] };
                for (i, q) in qids.iter().enumerate() {
                    wq.wqid[i] = *q;
                }
                r.body.walkqid = wq;
                encode(&r)
            }
            p9fcall::TOPEN => {
                let Some(&n) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let mode = f.body.create.mode;
                if srv.nodes[n].as_ref().unwrap().dir && mode & 3 != 0 {
                    return rerror(tag, "is a directory");
                }
                if mode & p9fcall::OTRUNC != 0 {
                    if let Some(node) = srv.nodes[n].as_mut() {
                        node.data.clear();
                    }
                }
                let mut r = Fcall::new(p9fcall::ROPEN);
                r.tag = tag;
                r.body.opened = OpenedBody { qid: qid_of(srv, n), iounit: 0 };
                encode(&r)
            }
            p9fcall::TCREATE => {
                let Some(&parent) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let CreateBody { perm, name, .. } = f.body.create;
                let name = String::from(p9fcall::cstr(name));
                if !srv.nodes[parent].as_ref().unwrap().dir {
                    return rerror(tag, "not a directory");
                }
                let exists = srv.nodes[parent]
                    .as_ref()
                    .unwrap()
                    .kids
                    .iter()
                    .any(|k| srv.nodes[*k].as_ref().is_some_and(|x| x.name == name));
                if exists {
                    return rerror(tag, "file already exists");
                }
                let idx = srv.nodes.len();
                srv.nodes.push(Some(mknode(&name, perm & DMDIR != 0, &[], parent, perm & 0o777)));
                srv.nodes[parent].as_mut().unwrap().kids.push(idx);
                srv.fids.insert(f.fid, idx);
                let mut r = Fcall::new(p9fcall::RCREATE);
                r.tag = tag;
                r.body.opened = OpenedBody { qid: qid_of(srv, idx), iounit: 0 };
                encode(&r)
            }
            p9fcall::TREAD => {
                let Some(&n) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let IoBody { offset, count, .. } = f.body.io;
                let node = srv.nodes[n].as_ref().unwrap();
                let out: Vec<u8> = if node.dir {
                    let mut o = Vec::new();
                    let mut pos = 0i64;
                    for k in node.kids.clone() {
                        if srv.nodes[k].is_none() {
                            continue;
                        }
                        let b = stat_blob(srv, k);
                        if pos >= offset && o.len() + b.len() <= count as usize {
                            o.extend_from_slice(&b);
                        }
                        pos += b.len() as i64;
                    }
                    o
                } else {
                    let s = (offset as usize).min(node.data.len());
                    let e = (s + count as usize).min(node.data.len());
                    node.data[s..e].to_vec()
                };
                let mut r = Fcall::new(p9fcall::RREAD);
                r.tag = tag;
                r.body.io = IoBody { offset: 0, count: out.len() as u32, data: out.as_ptr() as *const c_char };
                encode(&r)
            }
            p9fcall::TWRITE => {
                let Some(&n) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let IoBody { offset, count, data } = f.body.io;
                let src = core::slice::from_raw_parts(data as *const u8, count as usize);
                let node = srv.nodes[n].as_mut().unwrap();
                if node.dir {
                    return rerror(tag, "is a directory");
                }
                let off = offset as usize;
                if node.data.len() < off + src.len() {
                    node.data.resize(off + src.len(), 0);
                }
                node.data[off..off + src.len()].copy_from_slice(src);
                let mut r = Fcall::new(p9fcall::RWRITE);
                r.tag = tag;
                r.body.io = IoBody { offset: 0, count, data: core::ptr::null() };
                encode(&r)
            }
            p9fcall::TCLUNK => {
                srv.fids.remove(&f.fid);
                simple(p9fcall::RCLUNK, tag)
            }
            p9fcall::TREMOVE => {
                let Some(n) = srv.fids.remove(&f.fid) else { return rerror(tag, "unknown fid") };
                if n == 0 {
                    return rerror(tag, "permission denied");
                }
                if !srv.nodes[n].as_ref().unwrap().kids.is_empty() {
                    return rerror(tag, "directory not empty");
                }
                let parent = srv.nodes[n].as_ref().unwrap().parent;
                srv.nodes[parent].as_mut().unwrap().kids.retain(|k| *k != n);
                srv.nodes[n] = None;
                simple(p9fcall::RREMOVE, tag)
            }
            p9fcall::TSTAT => {
                let Some(&n) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let b = stat_blob(srv, n);
                let mut r = Fcall::new(p9fcall::RSTAT);
                r.tag = tag;
                r.body.stat = StatBody { nstat: b.len() as u16, stat: b.as_ptr() };
                encode(&r)
            }
            p9fcall::TWSTAT => {
                let Some(&n) = srv.fids.get(&f.fid) else { return rerror(tag, "unknown fid") };
                let st = f.body.stat;
                let mut raw = core::slice::from_raw_parts(st.stat, st.nstat as usize).to_vec();
                let mut strs = vec![0u8; raw.len() + 8];
                let mut d = Dir::new();
                if p9fcall::conv_m2d(&mut raw, &mut d, &mut strs) == 0 {
                    return rerror(tag, "bad stat");
                }
                let name = String::from(p9fcall::cstr(d.name));
                if !name.is_empty() {
                    let parent = srv.nodes[n].as_ref().unwrap().parent;
                    let clash = srv.nodes[parent]
                        .as_ref()
                        .unwrap()
                        .kids
                        .iter()
                        .any(|k| *k != n && srv.nodes[*k].as_ref().is_some_and(|x| x.name == name));
                    if clash {
                        return rerror(tag, "file already exists");
                    }
                    srv.nodes[n].as_mut().unwrap().name = name;
                }
                if d.mode != u32::MAX {
                    srv.nodes[n].as_mut().unwrap().perm = d.mode & 0o777;
                }
                simple(p9fcall::RWSTAT, tag)
            }
            _ => rerror(tag, "bad message type"),
        }
    }
}

fn serve(p: &Pipe) {
    let incoming = p.srv_take();
    let replies: Vec<Vec<u8>> = SRV.with(|g| {
        let Some(srv) = g.as_mut() else { return Vec::new() };
        srv.inbuf.extend_from_slice(&incoming);
        let mut out = Vec::new();
        loop {
            if srv.inbuf.len() < 4 {
                break;
            }
            let n = p9fcall::gbit32(&srv.inbuf) as usize;
            if n < 7 || srv.inbuf.len() < n {
                break;
            }
            let mut msg: Vec<u8> = srv.inbuf.drain(..n).collect();
            out.push(handle(srv, &mut msg));
        }
        out
    });
    for r in replies {
        p.srv_put(&r);
    }
}
