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

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_char;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use p9fcall::{
    Fcall, Qid, AttachBody, CreateBody, ErrorBody, IoBody, StatBody, VersionBody, WalkBody, BIT16SZ, BIT32SZ,
    BIT8SZ, IOHDRSZ, MAXWELEM, NOFID, NOTAG, QTDIR, TAUTH, TATTACH, TCLUNK, TCREATE, TFLUSH, TMAX,
    TOPEN, TREAD, TREMOVE, TSTAT, TVERSION, TWALK, TWRITE, TWSTAT, VERSION9P,
};

use crate::io::{self, Chan, PipeEnd};
use crate::sched;
use crate::sync::SpinLock;

pub type Res<T> = Result<T, String>;

pub const MAXRPC: usize = IOHDRSZ + 8192;
const MAXRPC0: usize = IOHDRSZ + 8192;
const TAGSHIFT: usize = 5;
const TAGMASK: usize = (1 << TAGSHIFT) - 1;
const NMASK: usize = (64 * 1024) >> TAGSHIFT;
const MNT_DEVCHAR: u16 = b'M' as u16;

pub const EBADARG: &str = "bad arg in system call";
pub const EBADUSEFD: &str = "inappropriate use of fd";
pub const EINTR: &str = "interrupted";
pub const EHUNGUP: &str = "i/o on hungup channel";
pub const EMOUNTRPC: &str = "mount rpc error";
pub const EMSIZE: &str = "message size too small";
pub const ESHORT: &str = "i/o count too small";
pub const ESHORTSTAT: &str = "stat buffer too small";
pub const ESBADSTAT: &str = "invalid directory entry received from server";
pub const ENOVERSION: &str = "version not established for mount channel";

fn err<T>(m: &str) -> Res<T> {
    Err(String::from(m))
}

static TAGS: SpinLock<[u32; NMASK]> = SpinLock::new([0; NMASK]);
static TAGS_READY: AtomicBool = AtomicBool::new(false);
static MNT_ID: AtomicU64 = AtomicU64::new(1);
static CHAN_DEV: AtomicU32 = AtomicU32::new(1);
static FID_NEXT: AtomicU32 = AtomicU32::new(1);

fn alloctag() -> Res<u16> {
    TAGS.with(|t| {
        if !TAGS_READY.swap(true, Ordering::SeqCst) {
            t[0] = 1;
            t[NMASK - 1] = 0x8000_0000;
        }
        for i in 0..NMASK {
            let v = t[i];
            if v == u32::MAX {
                continue;
            }
            let j = v.trailing_ones() as usize;
            t[i] |= 1 << j;
            return Ok(((i << TAGSHIFT) | j) as u16);
        }
        err("no tags left")
    })
}

fn freetag(tag: u16) {
    TAGS.with(|t| t[(tag as usize) >> TAGSHIFT] &= !(1u32 << ((tag as usize) & TAGMASK)));
}

fn newfid() -> u32 {
    loop {
        let f = FID_NEXT.fetch_add(1, Ordering::SeqCst);
        if f != NOFID && f != 0 {
            return f;
        }
    }
}

#[derive(Default, Clone)]
pub struct Request {
    pub typ: u8,
    pub fid: u32,
    pub afid: u32,
    pub newfid: u32,
    pub msize: u32,
    pub version: String,
    pub uname: String,
    pub aname: String,
    pub name: String,
    pub perm: u32,
    pub mode: u8,
    pub wname: Vec<String>,
    pub offset: i64,
    pub count: u32,
    pub data: Vec<u8>,
    pub stat: Vec<u8>,
    pub oldtag: u16,
}

#[derive(Default, Clone)]
pub struct Reply {
    pub typ: u8,
    pub tag: u16,
    pub msize: u32,
    pub version: String,
    pub ename: String,
    pub qid: Qid,
    pub aqid: Qid,
    pub iounit: u32,
    pub wqid: Vec<Qid>,
    pub count: u32,
    pub data: Vec<u8>,
    pub stat: Vec<u8>,
}

pub struct Mntrpc {
    pub tag: u16,
    req: SpinLock<Request>,
    rep: SpinLock<Reply>,
    done: AtomicBool,
    flushed: SpinLock<Option<Arc<Mntrpc>>>,
    fid: u32,
}

impl Drop for Mntrpc {
    fn drop(&mut self) {
        freetag(self.tag);
    }
}

struct MntState {
    rip: Option<sched::Tid>,
    queue: Vec<Arc<Mntrpc>>,
    inbuf: VecDeque<u8>,
}

pub struct Mnt {
    pub id: u64,
    pub msize: u32,
    pub version: String,
    conn: Arc<PipeEnd>,
    st: SpinLock<MntState>,
    zombies: SpinLock<Vec<u32>>,
    nchans: AtomicUsize,
}

struct ChanSt {
    qid: Qid,
    iounit: u32,
    mode: u8,
    opened: bool,
    clunked: bool,
}

pub struct MntChan {
    pub mnt: Arc<Mnt>,
    pub fid: u32,
    pub dev: u32,
    st: SpinLock<ChanSt>,
}

impl Drop for MntChan {
    fn drop(&mut self) {
        let clunked = self.st.with(|s| s.clunked);
        if !clunked && self.fid != 0 {
            self.mnt.zombies.with(|z| z.push(self.fid));
        }
        if self.mnt.nchans.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.mnt.conn.slot.mux.with(|m| *m = None);
        }
    }
}

impl MntChan {
    pub fn qid(&self) -> Qid {
        self.st.with(|s| s.qid)
    }

    pub fn is_dir(&self) -> bool {
        self.qid().typ & QTDIR != 0
    }

    pub fn iounit(&self) -> u32 {
        self.st.with(|s| s.iounit)
    }

    pub fn opened(&self) -> bool {
        self.st.with(|s| s.opened)
    }

    pub fn mode(&self) -> u8 {
        self.st.with(|s| s.mode)
    }
}

pub struct Walkqid {
    pub clone: Option<Arc<MntChan>>,
    pub qid: Vec<Qid>,
}

fn nul(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

pub fn pack_request(req: &Request, tag: u16) -> Res<Vec<u8>> {
    let mut f = Fcall::new(req.typ);
    f.tag = tag;
    f.fid = req.fid;
    let version = nul(&req.version);
    let uname = nul(&req.uname);
    let aname = nul(&req.aname);
    let name = nul(&req.name);
    let wnames: Vec<Vec<u8>> = req.wname.iter().map(|w| nul(w)).collect();
    match req.typ {
        TVERSION => {
            f.body.version = VersionBody { msize: req.msize, version: version.as_ptr() as *const c_char };
        }
        TAUTH => {
            f.fid = req.afid;
            f.body.attach = AttachBody { afid: req.afid, uname: uname.as_ptr() as *const c_char, aname: aname.as_ptr() as *const c_char };
        }
        TATTACH => {
            f.body.attach = AttachBody { afid: req.afid, uname: uname.as_ptr() as *const c_char, aname: aname.as_ptr() as *const c_char };
        }
        TFLUSH => {
            f.body.flush.oldtag = req.oldtag;
        }
        TWALK => {
            if wnames.len() > MAXWELEM {
                return err("devmnt: too many name elements");
            }
            let mut w = WalkBody { newfid: req.newfid, nwname: wnames.len() as u16, wname: [core::ptr::null(); MAXWELEM] };
            for (i, n) in wnames.iter().enumerate() {
                w.wname[i] = n.as_ptr() as *const c_char;
            }
            f.body.walk = w;
        }
        TOPEN => {
            f.body.create = CreateBody { perm: 0, name: core::ptr::null(), mode: req.mode };
        }
        TCREATE => {
            f.body.create = CreateBody { perm: req.perm, name: name.as_ptr() as *const c_char, mode: req.mode };
        }
        TREAD => {
            f.body.io = IoBody { offset: req.offset, count: req.count, data: core::ptr::null() };
        }
        TWRITE => {
            f.body.io = IoBody { offset: req.offset, count: req.data.len() as u32, data: req.data.as_ptr() as *const c_char };
        }
        TCLUNK | TREMOVE | TSTAT => {}
        TWSTAT => {
            f.body.stat = StatBody { nstat: req.stat.len() as u16, stat: req.stat.as_ptr() };
        }
        _ => return err("bad request type"),
    }
    let n = p9fcall::size_s2m(&f);
    if n == 0 {
        return err("bad fcall conversion on send");
    }
    let mut buf = vec![0u8; n];
    let k = p9fcall::conv_s2m(&f, &mut buf);
    if k == 0 {
        return err("bad fcall conversion on send");
    }
    buf.truncate(k);
    Ok(buf)
}

fn reply_from(f: &Fcall) -> Reply {
    let mut r = Reply { typ: f.typ, tag: f.tag, ..Reply::default() };
    // SAFETY: FFI/union access: the C call gets valid NUL-terminated or sized buffers owned by this frame, and the union field read matches the message type just checked.
    unsafe {
        match f.typ {
            p9fcall::RVERSION => {
                r.msize = f.body.version.msize;
                r.version = String::from(p9fcall::cstr(f.body.version.version));
            }
            p9fcall::RAUTH => r.aqid = f.body.authqid.aqid,
            p9fcall::RATTACH => r.qid = f.body.opened.qid,
            p9fcall::RERROR => r.ename = String::from(p9fcall::cstr(f.body.error.ename)),
            p9fcall::RWALK => {
                let n = (f.body.walkqid.nwqid as usize).min(MAXWELEM);
                r.wqid = f.body.walkqid.wqid[..n].to_vec();
                if (f.body.walkqid.nwqid as usize) > MAXWELEM {
                    r.count = f.body.walkqid.nwqid as u32;
                }
            }
            p9fcall::ROPEN | p9fcall::RCREATE => {
                r.qid = f.body.opened.qid;
                r.iounit = f.body.opened.iounit;
            }
            p9fcall::RREAD => {
                r.count = f.body.io.count;
                let p = f.body.io.data as *const u8;
                if !p.is_null() {
                    r.data = core::slice::from_raw_parts(p, f.body.io.count as usize).to_vec();
                }
            }
            p9fcall::RWRITE => r.count = f.body.io.count,
            p9fcall::RSTAT => {
                let n = f.body.stat.nstat as usize;
                let p = f.body.stat.stat;
                if !p.is_null() {
                    r.stat = core::slice::from_raw_parts(p, n).to_vec();
                }
            }
            _ => {}
        }
    }
    r
}

fn tx_write_all(c: &Arc<PipeEnd>, data: &[u8]) -> Res<()> {
    match io::pipe_kwrite(c, data) {
        Ok(k) if k == data.len() => Ok(()),
        Ok(_) => err("short write to mount channel"),
        Err(m) => err(m),
    }
}

fn tx_read(c: &Arc<PipeEnd>, buf: &mut [u8]) -> Res<usize> {
    io::pipe_kread(c, buf).map_err(String::from)
}

struct UmqGuard<'a>(&'a PipeEnd);

impl Drop for UmqGuard<'_> {
    fn drop(&mut self) {
        self.0.slot.busy.with(|b| *b = false);
    }
}

fn umq_lock(pe: &PipeEnd) -> Res<UmqGuard<'_>> {
    let Some(me) = sched::current() else { return err("no current thread") };
    loop {
        let got = pe.slot.busy.with(|b| {
            if *b {
                false
            } else {
                *b = true;
                true
            }
        });
        if got {
            return Ok(UmqGuard(pe));
        }
        if !sched::switching_active() {
            return err("mount channel busy");
        }
        if nap(me) {
            return err(EINTR);
        }
    }
}

fn nap(me: sched::Tid) -> bool {
    sched::sys_sleep(10);
    matches!(sched::finish_blocked_syscall(me), Some(r) if r != 0)
}

pub fn mntversion(c: &Chan, version: Option<&str>, msize: i32, returnlen: usize) -> Res<(usize, String)> {
    let Some(pe) = io::transport(c) else { return err(EBADUSEFD) };
    let _g = umq_lock(&pe)?;

    let mut msize = msize;
    if msize == 0 {
        msize = MAXRPC as i32;
    }
    let v = match version {
        Some(s) if !s.is_empty() => s,
        _ => VERSION9P,
    };

    if msize < 0 {
        return err("bad iounit in version call");
    }
    if !v.as_bytes().starts_with(VERSION9P.as_bytes()) {
        return err("bad 9P version specification");
    }

    if let Some(m) = pe.slot.mux.with(|w| w.clone()) {
        let k = m.version.len();
        if !v.as_bytes().starts_with(m.version.as_bytes()) {
            return Err(alloc::format!("incompatible 9P versions {} {}", m.version, v));
        }
        if returnlen > 0 && returnlen < k {
            return err(ESHORT);
        }
        return Ok((k, m.version.clone()));
    }

    let req = Request { typ: TVERSION, msize: msize as u32, version: String::from(v), ..Request::default() };
    let msg = pack_request(&req, NOTAG)?;
    if msg.len() > MAXRPC0 {
        return err("bad fversion conversion on send");
    }
    if tx_write_all(&pe, &msg).is_err() {
        return err("short write in fversion");
    }

    let mut buf = vec![0u8; MAXRPC0];
    let mut k = 0usize;
    while k < BIT32SZ || (k < p9fcall::gbit32(&buf) as usize && k < MAXRPC0) {
        let l = tx_read(&pe, &mut buf[k..])?;
        if l == 0 {
            return err("EOF receiving fversion reply");
        }
        k += l;
    }
    let mut f = Fcall::new(0);
    let l = p9fcall::conv_m2s(&mut buf[..k], &mut f);
    if l != k {
        return err("bad fversion conversion on reply");
    }
    let rep = reply_from(&f);
    if rep.typ != p9fcall::RVERSION {
        if rep.typ == p9fcall::RERROR {
            return Err(rep.ename);
        }
        return err("unexpected reply type in fversion");
    }
    if rep.msize > msize as u32 {
        return err("server tries to increase msize in fversion");
    }
    if rep.msize < 256 || rep.msize > 1024 * 1024 {
        return err("nonsense value of msize in fversion");
    }
    let rv = rep.version.clone();
    if !v.as_bytes().starts_with(rv.as_bytes()) {
        return err("bad 9P version returned from server");
    }
    if returnlen > 0 && returnlen < rv.len() {
        return err(ESHORT);
    }

    let m = Arc::new(Mnt {
        id: MNT_ID.fetch_add(1, Ordering::SeqCst),
        msize: rep.msize,
        version: rv.clone(),
        conn: pe.clone(),
        st: SpinLock::new(MntState { rip: None, queue: Vec::new(), inbuf: VecDeque::new() }),
        zombies: SpinLock::new(Vec::new()),
        nchans: AtomicUsize::new(0),
    });
    pe.slot.mux.with(|w| *w = Some(m));
    Ok((rv.len(), rv))
}

fn mntchk(c: &Chan) -> Res<Arc<Mnt>> {
    let Some(pe) = io::transport(c) else { return err(EBADUSEFD) };
    pe.slot.mux.with(|w| w.clone()).ok_or_else(|| String::from(ENOVERSION))
}

fn mux_of(c: &Chan) -> Res<Arc<Mnt>> {
    match mntchk(c) {
        Ok(m) => Ok(m),
        Err(_) => {
            mntversion(c, None, 0, 0)?;
            mntchk(c)
        }
    }
}

fn mntralloc(fid: u32) -> Res<Arc<Mntrpc>> {
    let tag = alloctag()?;
    Ok(Arc::new(Mntrpc {
        tag,
        req: SpinLock::new(Request::default()),
        rep: SpinLock::new(Reply::default()),
        done: AtomicBool::new(false),
        flushed: SpinLock::new(None),
        fid,
    }))
}

fn mntchan(m: &Arc<Mnt>, qid: Qid) -> Arc<MntChan> {
    m.nchans.fetch_add(1, Ordering::SeqCst);
    Arc::new(MntChan {
        mnt: m.clone(),
        fid: newfid(),
        dev: CHAN_DEV.fetch_add(1, Ordering::SeqCst),
        st: SpinLock::new(ChanSt { qid, iounit: 0, mode: 0, opened: false, clunked: false }),
    })
}

pub fn mntauth(c: &Chan, spec: &str, uname: &str) -> Res<Arc<MntChan>> {
    let m = mux_of(c)?;
    let ac = mntchan(&m, Qid::default());
    let r = mntralloc(ac.fid)?;
    r.req.with(|q| {
        q.typ = TAUTH;
        q.afid = ac.fid;
        q.uname = String::from(uname);
        q.aname = String::from(spec);
    });
    if let Err(e) = mountrpc(&m, &r) {
        ac.st.with(|s| s.clunked = true);
        return Err(e);
    }
    let rep = r.rep.with(|x| x.clone());
    ac.st.with(|s| {
        s.qid = rep.aqid;
        s.mode = p9fcall::ORDWR;
        s.iounit = m.msize - IOHDRSZ as u32;
    });
    Ok(ac)
}

pub fn mntattach(c: &Chan, ac: Option<&Arc<MntChan>>, spec: &str, uname: &str) -> Res<Arc<MntChan>> {
    let m = mux_of(c)?;
    if let Some(a) = ac {
        if !Arc::ptr_eq(&a.mnt, &m) {
            return err(EBADUSEFD);
        }
    }
    let nc = mntchan(&m, Qid::default());
    let r = mntralloc(nc.fid)?;
    r.req.with(|q| {
        q.typ = TATTACH;
        q.fid = nc.fid;
        q.afid = ac.map_or(NOFID, |a| a.fid);
        q.uname = String::from(uname);
        q.aname = String::from(spec);
    });
    if let Err(e) = mountrpc(&m, &r) {
        nc.st.with(|s| s.clunked = true);
        return Err(e);
    }
    let rep = r.rep.with(|x| x.clone());
    nc.st.with(|s| s.qid = rep.qid);
    Ok(nc)
}

pub fn mntwalk(c: &MntChan, name: &[String]) -> Res<Option<Walkqid>> {
    if name.len() > MAXWELEM {
        return err("devmnt: too many name elements");
    }
    let m = c.mnt.clone();
    let nc = mntchan(&m, c.qid());
    let r = mntralloc(c.fid)?;
    r.req.with(|q| {
        q.typ = TWALK;
        q.fid = c.fid;
        q.newfid = nc.fid;
        q.wname = name.to_vec();
    });
    if let Err(e) = mountrpc(&m, &r) {
        nc.st.with(|s| s.clunked = true);
        return Err(e);
    }
    let rep = r.rep.with(|x| x.clone());
    if rep.count as usize > name.len() || rep.wqid.len() > name.len() {
        nc.st.with(|s| s.clunked = true);
        return err("too many QIDs returned by walk");
    }
    if rep.wqid.len() < name.len() {
        nc.st.with(|s| s.clunked = true);
        if rep.wqid.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Walkqid { clone: None, qid: rep.wqid }));
    }
    if let Some(last) = rep.wqid.last() {
        nc.st.with(|s| s.qid = *last);
    }
    Ok(Some(Walkqid { clone: Some(nc), qid: rep.wqid }))
}

fn validstat(dp: &mut [u8]) -> Res<()> {
    if !p9fcall::stat_check(dp) {
        return err(ESBADSTAT);
    }
    Ok(())
}

fn mntdirfix(dirbuf: &mut [u8], c: &MntChan) {
    p9fcall::pbit16(&mut dirbuf[BIT16SZ..], MNT_DEVCHAR);
    p9fcall::pbit32(&mut dirbuf[2 * BIT16SZ..], c.dev);
}

pub fn mntstat(c: &MntChan) -> Res<Vec<u8>> {
    let m = c.mnt.clone();
    let r = mntralloc(c.fid)?;
    r.req.with(|q| {
        q.typ = TSTAT;
        q.fid = c.fid;
    });
    mountrpc(&m, &r)?;
    let mut st = r.rep.with(|x| x.stat.clone());
    validstat(&mut st)?;
    mntdirfix(&mut st, c);
    Ok(st)
}

fn openmode(o: u8) -> u8 {
    o & !(p9fcall::OTRUNC | p9fcall::ORCLOSE) & 3
}

fn mntopencreate(typ: u8, c: &MntChan, name: &str, omode: u8, perm: u32) -> Res<()> {
    let m = c.mnt.clone();
    let r = mntralloc(c.fid)?;
    r.req.with(|q| {
        q.typ = typ;
        q.fid = c.fid;
        q.mode = omode;
        if typ == TCREATE {
            q.perm = perm;
            q.name = String::from(name);
        }
    });
    mountrpc(&m, &r)?;
    let rep = r.rep.with(|x| x.clone());
    c.st.with(|s| {
        s.qid = rep.qid;
        s.mode = openmode(omode);
        s.iounit = rep.iounit;
        if s.iounit == 0 || s.iounit > m.msize - IOHDRSZ as u32 {
            s.iounit = m.msize - IOHDRSZ as u32;
        }
        s.opened = true;
    });
    Ok(())
}

pub fn mntopen(c: &MntChan, omode: u8) -> Res<()> {
    mntopencreate(TOPEN, c, "", omode, 0)
}

pub fn mntcreate(c: &MntChan, name: &str, omode: u8, perm: u32) -> Res<()> {
    mntopencreate(TCREATE, c, name, omode, perm)
}

fn mntclunk(c: &MntChan, t: u8) -> Res<()> {
    let m = c.mnt.clone();
    c.st.with(|s| s.clunked = true);
    let r = mntralloc(c.fid)?;
    r.req.with(|q| {
        q.typ = t;
        q.fid = c.fid;
    });
    mountrpc(&m, &r)
}

pub fn mntclose(c: &MntChan) -> Res<()> {
    mntclunk(c, TCLUNK)
}

pub fn mntremove(c: &MntChan) -> Res<()> {
    mntclunk(c, TREMOVE)
}

pub fn mntwstat(c: &MntChan, dp: &[u8]) -> Res<usize> {
    let m = c.mnt.clone();
    let r = mntralloc(c.fid)?;
    r.req.with(|q| {
        q.typ = TWSTAT;
        q.fid = c.fid;
        q.stat = dp.to_vec();
    });
    mountrpc(&m, &r)?;
    Ok(dp.len())
}

fn mntrdwr(typ: u8, c: &MntChan, wdata: &[u8], n: usize, off: i64) -> Res<(Vec<u8>, usize)> {
    let m = c.mnt.clone();
    let iounit = c.iounit() as usize;
    let mut out: Vec<u8> = Vec::new();
    let mut off = off;
    let mut left = n;
    let mut done = 0usize;
    loop {
        let mut nreq = left;
        if iounit != 0 && nreq > iounit {
            nreq = iounit;
        }
        let r = mntralloc(c.fid)?;
        r.req.with(|q| {
            q.typ = typ;
            q.fid = c.fid;
            q.offset = off;
            q.count = nreq as u32;
            if typ == TWRITE {
                q.data = wdata[done..done + nreq].to_vec();
            }
        });
        mountrpc(&m, &r)?;
        let rep = r.rep.with(|x| x.clone());
        let mut nr = rep.count as usize;
        if nr > nreq {
            nr = nreq;
        }
        if typ == TREAD {
            out.extend_from_slice(&rep.data[..nr.min(rep.data.len())]);
            nr = nr.min(rep.data.len());
        }
        off += nr as i64;
        done += nr;
        left -= nr;
        let noted = sched::current().map_or(false, |t| sched::notes_pending(t) > 0);
        if nr != nreq || left == 0 || noted {
            break;
        }
    }
    Ok((out, done))
}

pub fn mntread(c: &MntChan, n: usize, off: i64) -> Res<Vec<u8>> {
    let (mut buf, _) = mntrdwr(TREAD, c, &[], n, off)?;
    if c.is_dir() {
        let mut p = 0usize;
        let e = buf.len();
        while p + BIT16SZ < e {
            let dirlen = BIT16SZ + p9fcall::gbit16(&buf[p..]) as usize;
            if p + dirlen > e {
                break;
            }
            validstat(&mut buf[p..p + dirlen])?;
            mntdirfix(&mut buf[p..p + dirlen], c);
            p += dirlen;
        }
        if p != e {
            return err(ESBADSTAT);
        }
    }
    Ok(buf)
}

pub fn mntwrite(c: &MntChan, data: &[u8], off: i64) -> Res<usize> {
    let (_, done) = mntrdwr(TWRITE, c, data, data.len(), off)?;
    Ok(done)
}

pub fn mntreap(m: &Arc<Mnt>) {
    let fids: Vec<u32> = m.zombies.with(|z| core::mem::take(z));
    for fid in fids {
        let Ok(r) = mntralloc(fid) else { continue };
        r.req.with(|q| {
            q.typ = TCLUNK;
            q.fid = fid;
        });
        let _ = mountrpc_raw(m, &r);
    }
}

fn mountrpc(m: &Arc<Mnt>, r: &Arc<Mntrpc>) -> Res<()> {
    mntreap(m);
    mountrpc_raw(m, r)
}

fn mountrpc_raw(m: &Arc<Mnt>, r: &Arc<Mntrpc>) -> Res<()> {
    r.rep.with(|x| {
        x.tag = 0;
        x.typ = TMAX;
    });
    mountio(m, r.clone())?;
    let (t, ename) = r.rep.with(|x| (x.typ, x.ename.clone()));
    let want = r.req.with(|q| q.typ) + 1;
    match t {
        p9fcall::RERROR => Err(ename),
        p9fcall::RFLUSH => err(EINTR),
        t if t == want => Ok(()),
        _ => err(EMOUNTRPC),
    }
}

fn rpcattn(m: &Arc<Mnt>, r: &Arc<Mntrpc>) -> bool {
    r.done.load(Ordering::SeqCst) || m.st.with(|s| s.rip.is_none())
}

fn mountio_once(m: &Arc<Mnt>, r: &Arc<Mntrpc>) -> Res<()> {
    let Some(me) = sched::current() else { return err("no current thread") };
    m.st.with(|s| s.queue.insert(0, r.clone()));

    let req = r.req.with(|q| q.clone());
    let msg = pack_request(&req, r.tag)?;
    if msg.len() > m.msize as usize {
        return err(EMSIZE);
    }
    tx_write_all(&m.conn, &msg)?;

    loop {
        let got = m.st.with(|s| {
            if s.rip.is_none() {
                s.rip = Some(me);
                true
            } else {
                false
            }
        });
        if got {
            break;
        }
        while !rpcattn(m, r) {
            if !sched::switching_active() {
                return err("mount rpc would block");
            }
            if nap(me) {
                return err(EINTR);
            }
        }
        if r.done.load(Ordering::SeqCst) {
            return Ok(());
        }
    }
    while !r.done.load(Ordering::SeqCst) {
        mntrpcread(m, r)?;
        mountmux(m, r);
    }
    mntgate(m);
    Ok(())
}

fn mountio(m: &Arc<Mnt>, r: Arc<Mntrpc>) -> Res<()> {
    let Some(me) = sched::current() else { return err("no current thread") };
    let mut cur = r;
    loop {
        match mountio_once(m, &cur) {
            Ok(()) => {
                mntflushfree(m, cur);
                return Ok(());
            }
            Err(e) => {
                if m.st.with(|s| s.rip == Some(me)) {
                    mntgate(m);
                }
                if e != EINTR {
                    mntflushfree(m, cur);
                    return Err(e);
                }
                cur = mntflushalloc(&cur)?;
            }
        }
    }
}

fn doread(m: &Arc<Mnt>, len: usize) -> Res<()> {
    while m.st.with(|s| s.inbuf.len()) < len {
        let mut tmp = vec![0u8; m.msize as usize];
        let n = tx_read(&m.conn, &mut tmp)?;
        if n == 0 {
            return err(EMOUNTRPC);
        }
        m.st.with(|s| s.inbuf.extend(tmp[..n].iter().copied()));
    }
    Ok(())
}

fn mntrpcread(m: &Arc<Mnt>, r: &Arc<Mntrpc>) -> Res<()> {
    r.rep.with(|x| {
        x.typ = 0;
        x.tag = 0;
    });
    let hdr = BIT32SZ + BIT8SZ + BIT16SZ;
    doread(m, hdr)?;
    let mut head = [0u8; 4];
    m.st.with(|s| {
        for (i, b) in s.inbuf.iter().take(4).enumerate() {
            head[i] = *b;
        }
    });
    let len = p9fcall::gbit32(&head) as usize;
    if len < hdr || len > m.msize as usize {
        m.st.with(|s| s.inbuf.clear());
        return err(EMOUNTRPC);
    }
    doread(m, len)?;
    let mut msg: Vec<u8> = m.st.with(|s| s.inbuf.drain(..len).collect());
    let mut f = Fcall::new(0);
    if p9fcall::conv_m2s(&mut msg, &mut f) == 0 {
        return err(EMOUNTRPC);
    }
    let rep = reply_from(&f);
    r.rep.with(|x| *x = rep);
    Ok(())
}

fn mntgate(m: &Arc<Mnt>) {
    m.st.with(|s| s.rip = None);
}

fn mountmux(m: &Arc<Mnt>, r: &Arc<Mntrpc>) {
    let tag = r.rep.with(|x| x.tag);
    let found = m.st.with(|s| {
        let pos = s.queue.iter().position(|q| q.tag == tag)?;
        Some(s.queue.remove(pos))
    });
    match found {
        Some(q) => {
            if !Arc::ptr_eq(&q, r) {
                let rep = r.rep.with(|x| x.clone());
                q.rep.with(|x| *x = rep);
            }
            q.done.store(true, Ordering::SeqCst);
        }
        None => {
            log::warn!("mnt: unexpected reply from mount {} tag {}", m.id, tag);
        }
    }
}

fn mntflushalloc(r: &Arc<Mntrpc>) -> Res<Arc<Mntrpc>> {
    let fr = mntralloc(r.fid)?;
    let (t, old, tag) = r.req.with(|q| (q.typ, q.oldtag, r.tag));
    fr.req.with(|q| {
        q.typ = TFLUSH;
        q.oldtag = if t == TFLUSH { old } else { tag };
    });
    fr.flushed.with(|f| *f = Some(r.clone()));
    Ok(fr)
}

fn mntflushfree(m: &Arc<Mnt>, r: Arc<Mntrpc>) -> Arc<Mntrpc> {
    let mut r = r;
    loop {
        let fr = r.flushed.with(|f| f.take());
        if !r.done.load(Ordering::SeqCst) {
            r.rep.with(|x| x.typ = p9fcall::RFLUSH);
            mntqrm(m, &r);
        }
        match fr {
            None => break,
            Some(next) => r = next,
        }
    }
    r
}

fn mntqrm(m: &Arc<Mnt>, r: &Arc<Mntrpc>) {
    r.done.store(true, Ordering::SeqCst);
    m.st.with(|s| s.queue.retain(|q| !Arc::ptr_eq(q, r)));
}
