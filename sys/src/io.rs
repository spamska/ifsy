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
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;

use crate::mntfs::{self, MntFile};
use crate::netfs::{self, CsObj, NetObj, Opened};
use alloc::vec;
use alloc::vec::Vec;

use lwext4_rust::bindings::{O_RDONLY, O_RDWR};
use lwext4_rust::{Ext4File, InodeTypes};

use crate::console;
use crate::fs;
use crate::p9::{self, Stat};
use crate::sched;
use crate::sync::SpinLock;

pub const MAX_FDS: usize = 128;
const MAX_IO: u64 = 1 << 20;
const MAX_LINE: usize = 512;
const MAX_READY: usize = 4096;
const PIPE_CAP: usize = 32 * 1024;
const MAX_PATH: usize = 1024;

const OWRITE: u64 = 1;
const ORDWR: u64 = 2;
const OTRUNC: u64 = 0x10;

const LW_CREATE_TRUNC: u32 = 0x241;

pub struct FileObj {
    path: String,
    pos: SpinLock<u64>,
    access: u8,
}

pub struct ConsCtl;

impl Drop for ConsCtl {
    fn drop(&mut self) {
        COOKED.with(|c| c.set_raw(false));
    }
}

pub struct FsCtlObj {
    reply: SpinLock<Vec<u8>>,
    pos: SpinLock<usize>,
}

pub struct TextObj {
    name: &'static str,
    data: Vec<u8>,
    pos: SpinLock<u64>,
}

pub struct DirObj {
    path: String,
    next: SpinLock<usize>,
    members: Vec<String>,
}

struct PipeState {
    q: [VecDeque<u8>; 2],
    ends: [usize; 2],
}

pub struct Pipe {
    st: SpinLock<PipeState>,
    hook: SpinLock<Option<fn(&Pipe)>>,
}

impl Pipe {
    pub fn srv_take(&self) -> Vec<u8> {
        self.st.with(|s| s.q[1].drain(..).collect())
    }

    pub fn srv_put(&self, data: &[u8]) {
        self.st.with(|s| s.q[0].extend(data.iter().copied()));
    }
}

pub struct MuxSlot {
    pub mux: SpinLock<Option<Arc<crate::mnt::Mnt>>>,
    pub busy: SpinLock<bool>,
    pub offset: SpinLock<u64>,
}

pub struct PipeEnd {
    pipe: Arc<Pipe>,
    side: usize,
    pub slot: MuxSlot,
}

impl PipeEnd {
    fn new(pipe: Arc<Pipe>, side: usize) -> PipeEnd {
        PipeEnd {
            pipe,
            side,
            slot: MuxSlot { mux: SpinLock::new(None), busy: SpinLock::new(false), offset: SpinLock::new(0) },
        }
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        self.pipe.st.with(|s| s.ends[self.side] -= 1);
    }
}

#[derive(Clone)]
pub enum Chan {
    Console,
    Null,
    Zero,
    Pid(Arc<SpinLock<u64>>),
    Reboot,
    ConsCtl(Arc<ConsCtl>),
    Text(Arc<TextObj>),
    Ctl(&'static str),
    FsCtl(Arc<FsCtlObj>),
    File(Arc<FileObj>),
    Dir(Arc<DirObj>),
    Pipe(Arc<PipeEnd>),
    Mnt(Arc<MntFile>),
    Net(Arc<NetObj>),
    Cs(Arc<CsObj>),
}

#[derive(Clone)]
pub struct FdTable {
    slots: Vec<Option<Chan>>,
    cwd: String,
}

impl FdTable {
    pub fn stdio() -> Self {
        Self { slots: vec![Some(Chan::Console), Some(Chan::Console), Some(Chan::Console)], cwd: String::from("/") }
    }

    pub fn closed() -> Self {
        Self { slots: Vec::new(), cwd: String::from("/") }
    }

    fn get(&self, fd: u64) -> Option<Chan> {
        self.slots.get(usize::try_from(fd).ok()?).cloned().flatten()
    }

    fn alloc(&mut self, chan: Chan) -> Option<usize> {
        if let Some(i) = self.slots.iter().position(Option::is_none) {
            self.slots[i] = Some(chan);
            return Some(i);
        }
        if self.slots.len() >= MAX_FDS {
            return None;
        }
        self.slots.push(Some(chan));
        Some(self.slots.len() - 1)
    }

    fn set(&mut self, fd: usize, chan: Chan) -> bool {
        if fd >= MAX_FDS {
            return false;
        }
        if fd >= self.slots.len() {
            self.slots.resize(fd + 1, None);
        }
        self.slots[fd] = Some(chan);
        true
    }

    fn close(&mut self, fd: u64) -> bool {
        match usize::try_from(fd).ok().and_then(|i| self.slots.get_mut(i)) {
            Some(slot) if slot.is_some() => {
                *slot = None;
                true
            }
            _ => false,
        }
    }
}

pub type SharedFds = Arc<SpinLock<FdTable>>;

pub fn new_table() -> SharedFds {
    Arc::new(SpinLock::new(FdTable::stdio()))
}

pub fn closed_table() -> SharedFds {
    Arc::new(SpinLock::new(FdTable::closed()))
}

pub fn chan_of(fd: u64) -> Option<Chan> {
    sched::current_fds()?.with(|t| t.get(fd))
}

fn cwd() -> String {
    sched::current_fds().map(|f| f.with(|t| t.cwd.clone())).unwrap_or_else(|| String::from("/"))
}

pub fn resolve(path: &str) -> String {
    p9::normalize(&cwd(), path)
}

pub fn fail(msg: &str) -> i64 {
    sched::set_errstr_current(msg);
    -1
}

pub fn nap(me: sched::Tid) -> bool {
    sched::sys_sleep(10);
    matches!(sched::finish_blocked_syscall(me), Some(r) if r != 0)
}

struct Cooked {
    edit: Vec<u8>,
    ready: VecDeque<u8>,
    eof: bool,
    raw: bool,
}

static COOKED: SpinLock<Cooked> =
    SpinLock::new(Cooked { edit: Vec::new(), ready: VecDeque::new(), eof: false, raw: false });

impl Cooked {
    fn pump(&mut self) {
        while let Some(b) = console::read_byte() {
            self.byte(b);
        }
    }

    fn set_raw(&mut self, on: bool) {
        self.raw = on;
        self.edit.clear();
        self.eof = false;
    }

    fn byte(&mut self, b: u8) {
        if self.raw {
            if self.ready.len() < MAX_READY {
                self.ready.push_back(b);
            }
            return;
        }
        match b {
            b'\r' | b'\n' => {
                console::write_bytes(b"\n");
                self.edit.push(b'\n');
                self.finish_line();
            }
            0x08 | 0x7f => {
                if self.edit.pop().is_some() {
                    console::write_bytes(b"\x08 \x08");
                }
            }
            0x15 => {
                for _ in 0..self.edit.len() {
                    console::write_bytes(b"\x08 \x08");
                }
                self.edit.clear();
            }
            0x04 => {
                if self.edit.is_empty() {
                    self.eof = true;
                } else {
                    self.finish_line();
                }
            }
            0x20..=0x7e => {
                if self.edit.len() < MAX_LINE {
                    self.edit.push(b);
                    console::write_bytes(&[b]);
                }
            }
            0x80..=0xff => {
                if self.edit.len() < MAX_LINE {
                    self.edit.push(b);
                }
            }
            _ => {}
        }
    }

    fn finish_line(&mut self) {
        for b in self.edit.drain(..) {
            if self.ready.len() < MAX_READY {
                self.ready.push_back(b);
            }
        }
    }

    fn take(&mut self, n: usize) -> Option<Vec<u8>> {
        if !self.ready.is_empty() {
            let k = n.min(self.ready.len());
            return Some(self.ready.drain(..k).collect());
        }
        if self.eof {
            self.eof = false;
            return Some(Vec::new());
        }
        None
    }
}

fn console_read(buf: u64, n: u64) -> i64 {
    let n = n.min(MAX_IO).min(4096) as usize;
    if n == 0 {
        return 0;
    }
    let Some(me) = sched::current() else { return -1 };
    loop {
        let got = COOKED.with(|c| {
            c.pump();
            c.take(n)
        });
        if let Some(bytes) = got {
            if bytes.is_empty() {
                return 0;
            }
            if !sched::copy_to_user(buf, &bytes) {
                return fail("bad address in system call");
            }
            return bytes.len() as i64;
        }
        if !sched::switching_active() {
            return fail("no input available");
        }
        sched::sys_sleep(10);
        if let Some(r) = sched::finish_blocked_syscall(me) {
            if r != 0 {
                return -1;
            }
        }
    }
}

fn console_write(buf: u64, n: u64) -> i64 {
    let n = n.min(MAX_IO);
    let mut chunk = [0u8; 256];
    let mut done = 0u64;
    while done < n {
        let take = (n - done).min(chunk.len() as u64) as usize;
        if !sched::copy_from_user(buf.wrapping_add(done), &mut chunk[..take]) {
            if done > 0 {
                break;
            }
            return fail("bad address in system call");
        }
        console::write_bytes(&chunk[..take]);
        done += take as u64;
    }
    done as i64
}

/// Who may change what . root may do anything. Everybody else may change files
/// only under /tmp and in their own home directory /usr/NAME; the rest of the root file system,
/// /bin, /etc, /adm and the other homes included, is read-only to them.
fn may_write(path: &str) -> bool {
    if sched::is_hostowner() {
        return true;
    }
    let under = |base: &str| path == base || path.strip_prefix(base).is_some_and(|r| r.starts_with('/'));
    let home = format!("/usr/{}", sched::current_user());
    under("/tmp") || under(&home)
}

/// Password hashes can be read by root only.
fn may_read(path: &str) -> bool {
    sched::is_hostowner() || !(path == "/adm/shadow" || path.starts_with("/adm/shadow/"))
}

fn reboot_write(buf: u64, n: u64) -> i64 {
    if !sched::is_hostowner() {
        return fail("permission denied");
    }
    let n = n.min(64) as usize;
    let mut cmd = [0u8; 64];
    if !sched::copy_from_user(buf, &mut cmd[..n]) {
        return fail("bad address in system call");
    }
    let text = core::str::from_utf8(&cmd[..n]).unwrap_or("").trim();
    match text {
        "shutdown" | "halt" | "poweroff" => {
            crate::kprintln!("ifsy: shutdown: powering off");
            crate::store::flush();
            crate::arch::power_off()
        }
        "reboot" | "restart" => {
            crate::kprintln!("ifsy: shutdown: restarting");
            crate::store::flush();
            crate::arch::reset()
        }
        _ => fail("unknown command, write shutdown or reboot"),
    }
}

fn consctl_write(buf: u64, n: u64) -> i64 {
    let k = n.min(32) as usize;
    let mut cmd = [0u8; 32];
    if !sched::copy_from_user(buf, &mut cmd[..k]) {
        return fail("bad address in system call");
    }
    match core::str::from_utf8(&cmd[..k]).unwrap_or("").trim() {
        "rawon" => COOKED.with(|c| c.set_raw(true)),
        "rawoff" => COOKED.with(|c| c.set_raw(false)),
        _ => return fail("unknown command, write rawon or rawoff"),
    }
    n as i64
}

fn fsctl_write(o: &FsCtlObj, buf: u64, n: u64) -> i64 {
    let k = n.min(2 * MAX_PATH as u64 + 16) as usize;
    let mut cmd = vec![0u8; k];
    if !sched::copy_from_user(buf, &mut cmd) {
        return fail("bad address in system call");
    }
    let Ok(text) = core::str::from_utf8(&cmd) else { return fail("bad file name") };
    let mut it = text.trim_end_matches(['\n', '\0']).split('\n');
    let (op, a, b) = (it.next().unwrap_or(""), it.next(), it.next());
    if it.next().is_some() {
        return fail("too many arguments");
    }
    o.reply.with(|r| r.clear());
    o.pos.with(|p| *p = 0);
    let protected = |p: &str| is_dev(p) || netfs::is_net(p) || is_env(p) || is_env_child(p) || p == "/";
    let exists = |p: &str| kind_of(p).is_some() || fs::is_symlink(p);
    let denied = |p: &str| protected(p) || !may_write(p);

    match (op, a, b) {
        ("readlink", Some(p), None) => {
            let p = user_path_nofollow(p);
            if protected(&p) || !fs::is_symlink(&p) {
                return fail("not a symbolic link");
            }
            match fs::read_link(&p) {
                Ok(t) => {
                    o.reply.with(|r| *r = format!("{}\n", t).into_bytes());
                    n as i64
                }
                Err(e) => fail(p9::errno_msg(e)),
            }
        }
        ("symlink", Some(target), Some(link)) => {
            let link = user_path_nofollow(link);
            if denied(&link) {
                return fail("permission denied");
            }
            if target.is_empty() {
                return fail("empty link target");
            }
            if exists(&link) {
                return fail("file already exists");
            }
            match fs::make_symlink(target, &link) {
                Ok(()) => n as i64,
                Err(e) => fail(p9::errno_msg(e)),
            }
        }
        ("rename" | "link", Some(from), Some(to)) => {
            let (from, to) = (user_path_nofollow(from), user_path_nofollow(to));
            if denied(&from) || denied(&to) {
                return fail("permission denied");
            }
            let is_link = fs::is_symlink(&from);
            let kind = match (is_link, kind_of(&from)) {
                (true, _) => Kind::File,
                (false, Some(k)) => k,
                (false, None) => return fail("file does not exist"),
            };
            if exists(&to) {
                return fail("file already exists");
            }
            if kind == Kind::Dir && (to == from || to.starts_with(&format!("{}/", from))) {
                return fail("cannot move a directory into itself");
            }
            let r = if op == "link" {
                if kind == Kind::Dir {
                    return fail("cannot link a directory");
                }
                let (Ok(a), Ok(b)) = (alloc::ffi::CString::new(from.as_str()), alloc::ffi::CString::new(to.as_str())) else {
                    return fail("bad file name");
                };
                fs::with_root(|_| {
                    let r = unsafe { lwext4_rust::bindings::ext4_flink(a.as_ptr(), b.as_ptr()) };
                    if r == 0 { Ok(()) } else { Err(p9::errno_msg(r)) }
                })
            } else {
                fs::with_root(|_| {
                    let ty = if kind == Kind::Dir { InodeTypes::EXT4_DE_DIR } else { InodeTypes::EXT4_DE_REG_FILE };
                    let mut f = Ext4File::new(&from, ty);
                    let r = if kind == Kind::Dir { f.dir_mv(&from, &to) } else { f.file_rename(&from, &to) };
                    r.map(|_| ()).map_err(p9::errno_msg)
                })
            };
            match r {
                None => fail("no file system"),
                Some(Ok(())) => n as i64,
                Some(Err(m)) => fail(m),
            }
        }
        _ => fail("unknown command, write rename, link, symlink or readlink"),
    }
}

fn parse_size(s: &str) -> Option<(usize, usize)> {
    let (c, r) = s.split_once(['x', 'X'])?;
    Some((c.trim().parse().ok()?, r.trim().parse().ok()?))
}

fn ctl_write(name: &str, buf: u64, n: u64) -> i64 {
    let k = n.min(64) as usize;
    let mut cmd = [0u8; 64];
    if !sched::copy_from_user(buf, &mut cmd[..k]) {
        return fail("bad address in system call");
    }
    let text = core::str::from_utf8(&cmd[..k]).unwrap_or("").trim();
    let mut words = text.split_whitespace();
    if name == "diskctl" && !sched::is_hostowner() {
        return fail("permission denied");
    }
    match (name, words.next(), words.next(), words.next()) {
        ("userctl", Some(user), None, None) => match sched::set_user(user) {
            Ok(()) => n as i64,
            Err(e) => fail(e),
        },
        ("userctl", _, _, _) => fail("bad user name, write one name"),
        ("dispctl", Some("size"), Some(arg), None) => {
            let size = if arg == "default" {
                None
            } else {
                match parse_size(arg) {
                    Some(s) => Some(s),
                    None => return fail("bad size, want COLSxROWS or default"),
                }
            };
            match console::set_grid(size) {
                Ok(_) => n as i64,
                Err(e) => fail(e),
            }
        }
        ("dispctl", _, _, _) => fail("unknown command, write size COLSxROWS or size default"),
        ("diskctl", Some("grow"), Some(arg), None) => match arg.parse::<u64>() {
            Ok(bytes) => match fs::grow_root(bytes) {
                Ok(_) => n as i64,
                Err(msg) => fail(&msg),
            },
            Err(_) => fail("bad size, want a number of bytes"),
        },
        ("postnote", _, _, _) => {
            let mut parts = text.splitn(2, ' ');
            let pid = parts.next().and_then(|p| p.parse::<sched::Tid>().ok());
            let note = parts.next().unwrap_or("").trim();
            match pid {
                Some(pid) if !note.is_empty() => {
                    let me = sched::current_user();
                    if !sched::is_hostowner() && sched::user_of(pid).as_deref() != Some(me.as_str()) {
                        return fail("permission denied");
                    }
                    let posted = if note == "kill" {
                        sched::postnote(pid, "sys: kill", crate::note::NoteKind::Exit)
                    } else {
                        sched::postnote(pid, note, crate::note::NoteKind::User)
                    };
                    if posted { n as i64 } else { fail("no such process") }
                }
                _ => fail("bad note, write PID NOTE"),
            }
        }
        _ => fail("unknown command, write grow BYTES"),
    }
}

const DEV_BASE: [(&str, u32); 11] = [
    ("cons", 0o620),
    ("null", 0o666),
    ("zero", 0o666),
    ("pid", 0o444),
    ("reboot", 0o220),
    ("consctl", 0o220),
    ("dispctl", 0o220),
    ("diskctl", 0o220),
    ("fsctl", 0o660),
    ("postnote", 0o220),
    ("userctl", 0o220),
];

fn dev_names() -> Vec<(&'static str, u32)> {
    DEV_BASE.iter().chain(crate::procfs::NAMES.iter()).copied().collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
}

fn dev_stat(path: &str) -> Option<Stat> {
    if path == "/dev" {
        return Some(Stat {
            name: String::from("dev"),
            is_dir: true,
            perm: 0o555,
            length: 0,
            mtime: 0,
            qid_path: p9::hash_path(path),
        });
    }
    let name = path.strip_prefix("/dev/")?;
    let (_, perm) = dev_names().into_iter().find(|(n, _)| *n == name)?;
    Some(Stat {
        name: name.to_string(),
        is_dir: false,
        perm: perm,
        length: 0,
        mtime: 0,
        qid_path: p9::hash_path(path),
    })
}

fn is_env(path: &str) -> bool {
    path == "/env"
}

fn is_env_child(path: &str) -> bool {
    path.starts_with("/env/")
}

fn is_dev(path: &str) -> bool {
    path == "/dev" || path.starts_with("/dev/")
}

fn kind_of(path: &str) -> Option<Kind> {
    if mntfs::is_remote(path) {
        return mntfs::is_dir(path).map(|d| if d { Kind::Dir } else { Kind::File });
    }
    if path == "/" {
        return Some(Kind::Dir);
    }
    fs::with_root(|_| {
        let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
        if f.check_inode_exist(path, InodeTypes::EXT4_DE_DIR) {
            Some(Kind::Dir)
        } else if f.check_inode_exist(path, InodeTypes::EXT4_DE_REG_FILE) {
            Some(Kind::File)
        } else {
            None
        }
    })
    .flatten()
}

fn ext4_size(path: &str) -> Option<u64> {
    fs::with_root(|_| {
        let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
        f.file_open(path, O_RDONLY).ok()?;
        let size = f.file_size();
        let _ = f.file_close();
        Some(size)
    })
    .flatten()
}

fn stat_path(path: &str) -> Result<Stat, &'static str> {
    if mntfs::is_remote(path) {
        return mntfs::stat(path);
    }
    if is_env(path) {
        return Ok(Stat {
            name: String::from("env"),
            is_dir: true,
            perm: 0o555,
            length: 0,
            mtime: 0,
            qid_path: p9::hash_path(path),
        });
    }
    if netfs::is_net(path) {
        return netfs::stat(path).ok_or("file does not exist");
    }
    if is_dev(path) {
        return dev_stat(path).ok_or("file does not exist");
    }
    let kind = kind_of(path).ok_or("file does not exist")?;
    let perm = if path == "/" {
        0o755
    } else {
        fs::with_root(|_| {
            let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
            f.file_mode_get().map(|m| m & 0o777).map_err(p9::errno_msg)
        })
        .unwrap_or(Err("no file system"))?
    };
    let mtime = fs::with_root(|_| Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE).file_mtime_get().unwrap_or(0))
        .unwrap_or(0);
    let length = if kind == Kind::File { ext4_size(path).unwrap_or(0) } else { 0 };
    Ok(Stat {
        name: p9::base_name(path).to_string(),
        is_dir: kind == Kind::Dir,
        perm,
        length,
        mtime,
        qid_path: p9::hash_path(path),
    })
}

fn ext4_read(path: &str, off: u64, n: usize) -> Result<Vec<u8>, &'static str> {
    fs::with_root(|_| {
        let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
        f.file_open(path, O_RDONLY).map_err(p9::errno_msg)?;
        let size = f.file_size();
        let r = (|| -> Result<Vec<u8>, &'static str> {
            if off >= size {
                return Ok(Vec::new());
            }
            f.file_seek(off as i64, 0).map_err(p9::errno_msg)?;
            let want = n.min((size - off) as usize);
            let mut buf = vec![0u8; want];
            let mut done = 0;
            while done < want {
                let k = f.file_read(&mut buf[done..]).map_err(p9::errno_msg)?;
                if k == 0 {
                    break;
                }
                done += k;
            }
            buf.truncate(done);
            Ok(buf)
        })();
        let _ = f.file_close();
        r
    })
    .unwrap_or(Err("no file system"))
}

fn ext4_write(path: &str, off: u64, data: &[u8]) -> Result<usize, &'static str> {
    fs::with_root(|_| {
        let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
        f.file_open(path, O_RDWR).map_err(p9::errno_msg)?;
        let r = (|| -> Result<usize, &'static str> {
            f.file_seek(off as i64, 0).map_err(p9::errno_msg)?;
            let mut done = 0;
            while done < data.len() {
                let k = f.file_write(&data[done..]).map_err(p9::errno_msg)?;
                if k == 0 {
                    return Err("file system full");
                }
                done += k;
            }
            Ok(done)
        })();
        let _ = f.file_close();
        r
    })
    .unwrap_or(Err("no file system"))
}

fn ext4_create_file(path: &str, perm: u32) -> Result<(), &'static str> {
    fs::with_root(|_| {
        let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
        f.file_open(path, LW_CREATE_TRUNC).map_err(p9::errno_msg)?;
        let _ = f.file_close();
        let _ = f.file_mode_set(perm & 0o777);
        Ok(())
    })
    .unwrap_or(Err("no file system"))
}

fn ext4_truncate(path: &str) -> Result<(), &'static str> {
    fs::with_root(|_| {
        let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
        f.file_open(path, LW_CREATE_TRUNC).map_err(p9::errno_msg)?;
        let _ = f.file_close();
        Ok(())
    })
    .unwrap_or(Err("no file system"))
}

fn ext4_mkdir(path: &str, perm: u32) -> Result<(), &'static str> {
    fs::with_root(|_| {
        let mut d = Ext4File::new(path, InodeTypes::EXT4_DE_DIR);
        d.dir_mk(path).map_err(p9::errno_msg)?;
        let _ = d.file_mode_set(perm & 0o777);
        Ok(())
    })
    .unwrap_or(Err("no file system"))
}

fn child_path(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{}", name) } else { format!("{}/{}", dir, name) }
}

fn ext4_names(path: &str) -> Result<Vec<String>, &'static str> {
    let r = fs::with_root(|_| fs::list_dir(path)).ok_or("no file system")?;
    let entries = r.map_err(p9::errno_msg)?;
    Ok(entries
        .into_iter()
        .filter_map(|(n, _)| String::from_utf8(n).ok())
        .filter(|n| n != "." && n != "..")
        .collect())
}

fn dir_entries(path: &str) -> Result<Vec<Stat>, &'static str> {
    if mntfs::is_remote(path) {
        return mntfs::read_dir_all(path);
    }
    if netfs::is_net(path) {
        return netfs::entries(path).ok_or("file does not exist");
    }
    if is_env(path) {
        return Ok(Vec::new());
    }
    if path == "/dev" {
        return Ok(dev_names().iter().filter_map(|(n, _)| dev_stat(&format!("/dev/{}", n))).collect());
    }
    let mut out = Vec::new();
    for name in ext4_names(path)? {
        let child = child_path(path, &name);
        if fs::is_symlink(&child) {
            let target = fs::read_link(&child).unwrap_or_default();
            let followed = through_links(child.clone(), true);
            let (perm, mtime) = match stat_path(&followed) {
                Ok(t) => (t.perm, t.mtime),
                Err(_) => (0o777, 0),
            };
            out.push(Stat { name, is_dir: false, perm, length: target.len() as u64, mtime, qid_path: p9::hash_path(&child) });
        } else if let Ok(st) = stat_path(&child) {
            out.push(st);
        }
    }
    if path == "/" {
        for p in ["/dev", "/env", "/net"] {
            if let Ok(st) = stat_path(p) {
                out.push(st);
            }
        }
    }
    Ok(out)
}

fn dir_read(d: &DirObj, buf: u64, n: u64) -> i64 {
    let n = n.min(MAX_IO) as usize;
    let listing = if d.members.is_empty() {
        dir_entries(&d.path)
    } else {
        let mut all = Vec::new();
        let mut res = Ok(());
        for m in &d.members {
            match dir_entries(m) {
                Ok(mut e) => all.append(&mut e),
                Err(e) => {
                    res = Err(e);
                    break;
                }
            }
        }
        res.map(|_| all)
    };
    let entries = match listing {
        Ok(e) => e,
        Err(m) => return fail(m),
    };
    let mut idx = d.next.with(|x| *x);
    let mut out: Vec<u8> = Vec::new();
    while idx < entries.len() {
        let rec = p9::pack_stat(&entries[idx]);
        if out.len() + rec.len() > n {
            if out.is_empty() {
                return fail("read buffer too small for a directory entry");
            }
            break;
        }
        out.extend_from_slice(&rec);
        idx += 1;
    }
    if !out.is_empty() && !sched::copy_to_user(buf, &out) {
        return fail("bad address in system call");
    }
    d.next.with(|x| *x = idx);
    out.len() as i64
}

pub fn path_exists(p: &str) -> bool {
    if mntfs::is_remote(p) {
        mntfs::exists(p)
    } else {
        stat_path(p).is_ok()
    }
}

fn ns_active() -> bool {
    sched::current_ns().is_some_and(|n| !n.with(|x| x.is_empty()))
}

pub fn ns_walk(visible: &str, apply_last: bool) -> crate::ns::Walked {
    let plain = crate::ns::Walked { path: String::from(visible), union: None };
    let Some(ns) = sched::current_ns() else { return plain };
    let snap = ns.with(|n| if n.is_empty() { None } else { Some(n.clone()) });
    match snap {
        Some(n) => n.walk(visible, apply_last, &path_exists),
        None => plain,
    }
}

fn user_path_ex(raw: &str, follow_last: bool) -> (String, Option<Vec<(String, u32)>>) {
    if let Some(r) = raw.strip_prefix("#c/") {
        return (format!("/dev/{}", r), None);
    }
    let w = ns_walk(&resolve(raw), true);
    through_links_ex(w.path, w.union, follow_last)
}

fn user_path(raw: &str) -> String {
    user_path_ex(raw, true).0
}

fn user_path_nofollow(raw: &str) -> String {
    user_path_ex(raw, false).0
}

fn through_links(path: String, follow_last: bool) -> String {
    through_links_ex(path, None, follow_last).0
}

fn through_links_ex(
    mut path: String,
    mut union: Option<Vec<(String, u32)>>,
    follow_last: bool,
) -> (String, Option<Vec<(String, u32)>>) {
    let mut hops = 0usize;
    loop {
        if path == "/" || is_dev(&path) || netfs::is_net(&path) || is_env(&path) || is_env_child(&path) || mntfs::is_remote(&path) {
            return (path, union);
        }
        match fs::follow_ex(&path, follow_last, &mut hops) {
            fs::Followed::Done(p) => return (p, union),
            fs::Followed::Abs(target) => {
                let w = ns_walk(&resolve(&target), true);
                path = w.path;
                union = w.union;
            }
        }
    }
}

pub fn resolve_follow(path: &str) -> String {
    through_links(ns_walk(&resolve(path), true).path, true)
}

pub fn read_image(path: &str) -> Result<Vec<u8>, String> {
    if mntfs::is_remote(path) {
        return mntfs::read_image(path).map_err(String::from);
    }
    match fs::with_root(|_| fs::read_file(path)) {
        None => Err(String::from("exec: no root file system")),
        Some(Err(2)) => Err(String::from("file does not exist")),
        Some(Err(e)) => Err(format!("exec: cannot read file (errno {})", e)),
        Some(Ok(v)) => Ok(v),
    }
}

fn split_parent(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(0) => (String::from("/"), String::from(&path[1..])),
        Some(i) => (String::from(&path[..i]), String::from(&path[i + 1..])),
        None => (String::from("/"), String::from(path)),
    }
}

fn remote_split(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(i) => (String::from(&path[..i]), String::from(&path[i + 1..])),
        None => (String::from(path), String::new()),
    }
}

fn create_target(raw: &str) -> Result<String, &'static str> {
    if raw.starts_with("#c/") || !ns_active() {
        return Ok(user_path(raw));
    }
    let visible = resolve(raw);
    let (parent, name) = split_parent(&visible);
    if name.is_empty() {
        return Ok(user_path(raw));
    }
    let w = ns_walk(&parent, true);
    let dir = match w.union {
        Some(ms) => crate::ns::createdir(&ms)?,
        None => w.path,
    };
    Ok(through_links(crate::ns::join(&dir, &name), true))
}

fn open_dev(path: &str) -> Result<Chan, &'static str> {
    if path == "/dev" {
        return Ok(Chan::Dir(Arc::new(DirObj { path: String::from("/dev"), next: SpinLock::new(0), members: Vec::new() })));
    }
    match path.strip_prefix("/dev/") {
        Some("cons") => Ok(Chan::Console),
        Some("null") => Ok(Chan::Null),
        Some("zero") => Ok(Chan::Zero),
        Some("pid") => Ok(Chan::Pid(Arc::new(SpinLock::new(0)))),
        Some("reboot") => Ok(Chan::Reboot),
        Some("consctl") => Ok(Chan::ConsCtl(Arc::new(ConsCtl))),
        Some("dispctl") => Ok(Chan::Ctl("dispctl")),
        Some("diskctl") => Ok(Chan::Ctl("diskctl")),
        Some("postnote") => Ok(Chan::Ctl("postnote")),
        Some("userctl") => Ok(Chan::Ctl("userctl")),
        Some("fsctl") => Ok(Chan::FsCtl(Arc::new(FsCtlObj { reply: SpinLock::new(Vec::new()), pos: SpinLock::new(0) }))),
        Some(name) if crate::procfs::is_text(name) => {
            let name = crate::procfs::NAMES.iter().find(|(n, _)| *n == name).map(|(n, _)| *n).unwrap_or("sysname");
            let data = crate::procfs::content(name).ok_or("file does not exist")?;
            Ok(Chan::Text(Arc::new(TextObj { name, data, pos: SpinLock::new(0) })))
        }
        _ => Err("file does not exist"),
    }
}

fn open_chan(raw: &str, mode: u64) -> Result<Chan, &'static str> {
    if let Some(n) = raw.strip_prefix("#d/").or_else(|| raw.strip_prefix("/fd/")) {
        return n.parse::<u64>().ok().and_then(chan_of).ok_or("fd out of range or not open");
    }
    let (path, union) = user_path_ex(raw, true);
    if mntfs::is_remote(&path) && union.as_ref().is_none_or(|u| u.len() < 2) {
        return mntfs::open(&path, mode);
    }
    if let Some(ms) = union.filter(|u| u.len() > 1) {
        let members: Vec<String> = ms.into_iter().map(|m| m.0).collect();
        return Ok(Chan::Dir(Arc::new(DirObj { path, next: SpinLock::new(0), members })));
    }
    if netfs::is_net(&path) {
        return match netfs::open(&path, mode)? {
            Opened::Dir(p) => Ok(Chan::Dir(Arc::new(DirObj { path: p, next: SpinLock::new(0), members: Vec::new() }))),
            Opened::Obj(o) => Ok(Chan::Net(Arc::new(o))),
            Opened::Cs(o) => Ok(Chan::Cs(Arc::new(o))),
        };
    }
    if is_dev(&path) {
        return open_dev(&path);
    }
    if is_env(&path) {
        return Ok(Chan::Dir(Arc::new(DirObj { path, next: SpinLock::new(0), members: Vec::new() })));
    }
    if is_env_child(&path) {
        return Err("file does not exist");
    }
    let acc = mode & 3;
    match kind_of(&path) {
        None => Err("file does not exist"),
        Some(Kind::Dir) => {
            if acc != 0 && acc != 3 {
                return Err("is a directory");
            }
            Ok(Chan::Dir(Arc::new(DirObj { path, next: SpinLock::new(0), members: Vec::new() })))
        }
        Some(Kind::File) => {
            if !may_read(&path) {
                return Err("permission denied");
            }
            if (acc == OWRITE || acc == ORDWR || mode & OTRUNC != 0) && !may_write(&path) {
                return Err("permission denied");
            }
            if mode & OTRUNC != 0 && (acc == OWRITE || acc == ORDWR) {
                ext4_truncate(&path)?;
            }
            let access = match acc {
                1 => 1,
                2 => 2,
                _ => 0,
            };
            Ok(Chan::File(Arc::new(FileObj { path, pos: SpinLock::new(0), access })))
        }
    }
}

fn chan_path(c: &Chan) -> String {
    match c {
        Chan::Console => String::from("/dev/cons"),
        Chan::Null => String::from("/dev/null"),
        Chan::Zero => String::from("/dev/zero"),
        Chan::Pid(_) => String::from("/dev/pid"),
        Chan::Reboot => String::from("/dev/reboot"),
        Chan::Text(t) => format!("/dev/{}", t.name),
        Chan::ConsCtl(_) => String::from("/dev/consctl"),
        Chan::Ctl(n) => format!("/dev/{}", n),
        Chan::FsCtl(_) => String::from("/dev/fsctl"),
        Chan::File(f) => f.path.clone(),
        Chan::Dir(d) => d.path.clone(),
        Chan::Pipe(_) => String::from("#|/data"),
        Chan::Mnt(f) => f.path.clone(),
        Chan::Net(o) => o.path(),
        Chan::Cs(_) => String::from("/net/cs"),
    }
}

fn chan_stat(c: &Chan) -> Result<Stat, &'static str> {
    match c {
        Chan::Pipe(_) => Ok(Stat {
            name: String::from("data"),
            is_dir: false,
            perm: 0o600,
            length: 0,
            mtime: 0,
            qid_path: 1,
        }),
        Chan::Text(t) => {
            let mut st = stat_path(&chan_path(c))?;
            st.length = t.data.len() as u64;
            Ok(st)
        }
        Chan::Mnt(f) => mntfs::file_stat(f),
        Chan::Net(o) => Ok(o.stat()),
        Chan::Cs(_) => netfs::stat("/net/cs").ok_or("file does not exist"),
        other => stat_path(&chan_path(other)),
    }
}

pub fn pipe_kread(e: &PipeEnd, out: &mut [u8]) -> Result<usize, &'static str> {
    if out.is_empty() {
        return Ok(0);
    }
    let Some(me) = sched::current() else { return Err("no current thread") };
    loop {
        let got = e.pipe.st.with(|s| {
            let q = &mut s.q[e.side];
            if !q.is_empty() {
                let k = out.len().min(q.len());
                for (i, b) in q.drain(..k).enumerate() {
                    out[i] = b;
                }
                Some(k)
            } else if s.ends[1 - e.side] == 0 {
                Some(0)
            } else {
                None
            }
        });
        if let Some(k) = got {
            return Ok(k);
        }
        if !sched::switching_active() {
            return Err("no data available");
        }
        if nap(me) {
            return Err("interrupted");
        }
    }
}

pub fn pipe_kwrite(e: &PipeEnd, data: &[u8]) -> Result<usize, &'static str> {
    if data.is_empty() {
        return Ok(0);
    }
    let Some(me) = sched::current() else { return Err("no current thread") };
    let mut done = 0usize;
    loop {
        let (wrote, hung) = e.pipe.st.with(|s| {
            if s.ends[1 - e.side] == 0 {
                (0, true)
            } else {
                let q = &mut s.q[1 - e.side];
                let room = PIPE_CAP.saturating_sub(q.len());
                let k = room.min(data.len() - done);
                q.extend(data[done..done + k].iter().copied());
                (k, false)
            }
        });
        if hung {
            return if done > 0 { Ok(done) } else { Err("write on closed pipe") };
        }
        done += wrote;
        if wrote > 0 {
            let hook = e.pipe.hook.with(|h| *h);
            if let Some(h) = hook {
                h(&e.pipe);
            }
        }
        if done >= data.len() {
            return Ok(done);
        }
        if !sched::switching_active() {
            return if done > 0 { Ok(done) } else { Err("pipe is full") };
        }
        if nap(me) {
            return if done > 0 { Ok(done) } else { Err("interrupted") };
        }
    }
}

pub fn hooked_pipe(hook: fn(&Pipe)) -> Chan {
    let pipe = Arc::new(Pipe {
        st: SpinLock::new(PipeState { q: [VecDeque::new(), VecDeque::new()], ends: [1, 1] }),
        hook: SpinLock::new(Some(hook)),
    });
    Chan::Pipe(Arc::new(PipeEnd::new(pipe, 0)))
}

pub fn pipe_pair() -> (Chan, Chan) {
    let pipe = Arc::new(Pipe {
        st: SpinLock::new(PipeState { q: [VecDeque::new(), VecDeque::new()], ends: [1, 1] }),
        hook: SpinLock::new(None),
    });
    (Chan::Pipe(Arc::new(PipeEnd::new(pipe.clone(), 0))), Chan::Pipe(Arc::new(PipeEnd::new(pipe, 1))))
}

pub fn transport(chan: &Chan) -> Option<Arc<PipeEnd>> {
    match chan {
        Chan::Pipe(e) => Some(e.clone()),
        _ => None,
    }
}

fn pipe_read(e: &PipeEnd, buf: u64, n: u64) -> i64 {
    let n = n.min(MAX_IO).min(PIPE_CAP as u64) as usize;
    if n == 0 {
        return 0;
    }
    let mut tmp = vec![0u8; n];
    match pipe_kread(e, &mut tmp) {
        Ok(0) => 0,
        Ok(k) => {
            if !sched::copy_to_user(buf, &tmp[..k]) {
                return fail("bad address in system call");
            }
            k as i64
        }
        Err(m) => fail(m),
    }
}

fn pipe_write(e: &PipeEnd, buf: u64, n: u64) -> i64 {
    let n = n.min(MAX_IO) as usize;
    if n == 0 {
        return 0;
    }
    let mut data = vec![0u8; n];
    if !sched::copy_from_user(buf, &mut data) {
        return fail("bad address in system call");
    }
    match pipe_kwrite(e, &data) {
        Ok(k) => k as i64,
        Err(m) => fail(m),
    }
}

pub const CURRENT_OFFSET: u64 = u64::MAX;

pub fn sys_read(fd: u64, buf: u64, n: u64, off: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    match chan {
        Chan::Console => console_read(buf, n),
        Chan::Null => 0,
        Chan::Reboot | Chan::ConsCtl(_) | Chan::Ctl(_) => fail("permission denied"),
        Chan::FsCtl(o) => {
            let explicit = off != CURRENT_OFFSET;
            let start = if explicit { off as usize } else { o.pos.with(|p| *p) };
            let data = o.reply.with(|r| r.get(start..).map(|s| s[..s.len().min(n.min(MAX_IO) as usize)].to_vec()).unwrap_or_default());
            if !data.is_empty() && !sched::copy_to_user(buf, &data) {
                return fail("bad address in system call");
            }
            if !explicit {
                o.pos.with(|p| *p += data.len());
            }
            data.len() as i64
        }
        Chan::Text(t) => {
            let explicit = off != CURRENT_OFFSET;
            let start = if explicit { off } else { t.pos.with(|p| *p) } as usize;
            if start >= t.data.len() {
                return 0;
            }
            let k = (n.min(MAX_IO) as usize).min(t.data.len() - start);
            if !sched::copy_to_user(buf, &t.data[start..start + k]) {
                return fail("bad address in system call");
            }
            if !explicit {
                t.pos.with(|p| *p += k as u64);
            }
            k as i64
        }
        Chan::Zero => {
            let n = n.min(MAX_IO);
            let zeros = [0u8; 256];
            let mut done = 0u64;
            while done < n {
                let k = (n - done).min(zeros.len() as u64) as usize;
                if !sched::copy_to_user(buf.wrapping_add(done), &zeros[..k]) {
                    return if done > 0 { done as i64 } else { fail("bad address in system call") };
                }
                done += k as u64;
            }
            done as i64
        }
        Chan::Pid(pos) => {
            let text = format!("{:11} ", sched::current().unwrap_or(0));
            let explicit = off != CURRENT_OFFSET;
            let start = if explicit { off } else { pos.with(|p| *p) } as usize;
            let t = text.as_bytes();
            if start >= t.len() {
                return 0;
            }
            let k = (n as usize).min(t.len() - start);
            if !sched::copy_to_user(buf, &t[start..start + k]) {
                return fail("bad address in system call");
            }
            if !explicit {
                pos.with(|p| *p += k as u64);
            }
            k as i64
        }
        Chan::File(f) => {
            if f.access == 1 {
                return fail("file not open for reading");
            }
            let explicit = off != CURRENT_OFFSET;
            let at = if explicit { off } else { f.pos.with(|p| *p) };
            match ext4_read(&f.path, at, n.min(MAX_IO) as usize) {
                Ok(d) => {
                    if !d.is_empty() && !sched::copy_to_user(buf, &d) {
                        return fail("bad address in system call");
                    }
                    if !explicit {
                        f.pos.with(|p| *p += d.len() as u64);
                    }
                    d.len() as i64
                }
                Err(m) => fail(m),
            }
        }
        Chan::Dir(d) => dir_read(&d, buf, n),
        Chan::Pipe(e) => pipe_read(&e, buf, n),
        Chan::Mnt(f) => mntfs::file_read(&f, buf, n, off),
        Chan::Net(o) => netfs::read(&o, buf, n, off),
        Chan::Cs(o) => netfs::cs_read(&o, buf, n),
    }
}

pub fn sys_write(fd: u64, buf: u64, n: u64, off: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    match chan {
        Chan::Console => console_write(buf, n),
        Chan::Null | Chan::Zero => n.min(MAX_IO) as i64,
        Chan::Pid(_) | Chan::Text(_) => fail("permission denied"),
        Chan::Reboot => reboot_write(buf, n),
        Chan::ConsCtl(_) => consctl_write(buf, n),
        Chan::Ctl(name) => ctl_write(name, buf, n),
        Chan::FsCtl(o) => fsctl_write(&o, buf, n),
        Chan::File(f) => {
            if f.access == 0 {
                return fail("file not open for writing");
            }
            let n = n.min(MAX_IO) as usize;
            let mut data = vec![0u8; n];
            if !sched::copy_from_user(buf, &mut data) {
                return fail("bad address in system call");
            }
            let explicit = off != CURRENT_OFFSET;
            let at = if explicit { off } else { f.pos.with(|p| *p) };
            match ext4_write(&f.path, at, &data) {
                Ok(k) => {
                    if !explicit {
                        f.pos.with(|p| *p += k as u64);
                    }
                    k as i64
                }
                Err(m) => fail(m),
            }
        }
        Chan::Dir(_) => fail("is a directory"),
        Chan::Pipe(e) => pipe_write(&e, buf, n),
        Chan::Mnt(f) => mntfs::file_write(&f, buf, n, off),
        Chan::Net(o) => netfs::write(&o, buf, n),
        Chan::Cs(o) => netfs::cs_write(&o, buf, n),
    }
}

pub fn install(chan: Chan) -> i64 {
    let Some(fds) = sched::current_fds() else { return -1 };
    match fds.with(|t| t.alloc(chan)) {
        Some(fd) => fd as i64,
        None => fail("out of file descriptors"),
    }
}

pub fn user_name(ptr: u64) -> Option<String> {
    sched::cstr_from_user(ptr, MAX_PATH)
}

pub fn sys_open(path_ptr: u64, mode: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    match open_chan(&raw, mode) {
        Ok(c) => install(c),
        Err(m) => fail(m),
    }
}

pub fn sys_create(path_ptr: u64, mode: u64, perm: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    let path = match create_target(&raw) {
        Ok(p) => p,
        Err(m) => return fail(m),
    };
    if mntfs::is_remote(&path) {
        let (parent, name) = remote_split(&path);
        return match mntfs::create(&parent, &name, mode, perm as u32) {
            Ok(c) => install(c),
            Err(m) => fail(m),
        };
    }
    if netfs::is_net(&path) {
        return fail("permission denied");
    }
    if is_env_child(&path) && perm as u32 & p9::DMDIR == 0 {
        return install(Chan::Null);
    }
    if is_env(&path) || is_env_child(&path) {
        return fail("permission denied");
    }
    if is_dev(&path) {
        if perm as u32 & p9::DMDIR == 0 && path != "/dev" {
            if let Ok(c) = open_dev(&path) {
                return install(c);
            }
        }
        return fail("permission denied");
    }
    if !may_write(&path) {
        return fail("permission denied");
    }
    let perm = perm as u32;
    let existing = kind_of(&path);
    let chan = if perm & p9::DMDIR != 0 {
        if existing.is_some() {
            return fail("file already exists");
        }
        if let Err(m) = ext4_mkdir(&path, perm) {
            return fail(m);
        }
        Chan::Dir(Arc::new(DirObj { path, next: SpinLock::new(0), members: Vec::new() }))
    } else {
        if existing == Some(Kind::Dir) {
            return fail("is a directory");
        }
        if let Err(m) = ext4_create_file(&path, perm) {
            return fail(m);
        }
        let access = match mode & 3 {
            1 => 1,
            2 => 2,
            _ => 0,
        };
        Chan::File(Arc::new(FileObj { path, pos: SpinLock::new(0), access }))
    };
    install(chan)
}

pub fn sys_close(fd: u64) -> i64 {
    let Some(fds) = sched::current_fds() else { return -1 };
    if fds.with(|t| t.close(fd)) { 0 } else { fail("fd out of range or not open") }
}

pub fn sys_dup(old: u64, new: u64) -> i64 {
    let Some(fds) = sched::current_fds() else { return -1 };
    let new = new as i64;
    let r = fds.with(|t| {
        let chan = t.get(old)?;
        if new < 0 {
            t.alloc(chan)
        } else if t.set(new as usize, chan) {
            Some(new as usize)
        } else {
            None
        }
    });
    match r {
        Some(fd) => fd as i64,
        None => fail("fd out of range or not open"),
    }
}

pub fn sys_pipe(fdp: u64) -> i64 {
    let pipe = Arc::new(Pipe {
        st: SpinLock::new(PipeState { q: [VecDeque::new(), VecDeque::new()], ends: [1, 1] }),
        hook: SpinLock::new(None),
    });
    let rd = Chan::Pipe(Arc::new(PipeEnd::new(pipe.clone(), 0)));
    let wr = Chan::Pipe(Arc::new(PipeEnd::new(pipe, 1)));
    let Some(fds) = sched::current_fds() else { return -1 };
    let got = fds.with(|t| {
        let a = t.alloc(rd)?;
        match t.alloc(wr) {
            Some(b) => Some((a, b)),
            None => {
                t.close(a as u64);
                None
            }
        }
    });
    let Some((a, b)) = got else { return fail("out of file descriptors") };
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(a as i32).to_le_bytes());
    out[4..].copy_from_slice(&(b as i32).to_le_bytes());
    if !sched::copy_to_user(fdp, &out) {
        fds.with(|t| {
            t.close(a as u64);
            t.close(b as u64);
        });
        return fail("bad address in system call");
    }
    0
}

pub fn sys_chdir(path_ptr: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    let path = user_path(&raw);
    let keep = if ns_active() { resolve(&raw) } else { path.clone() };
    let ok = if netfs::is_net(&path) { netfs::stat(&path).is_some_and(|s| s.is_dir) } else if is_dev(&path) { path == "/dev" } else if is_env(&path) { true } else { kind_of(&path) == Some(Kind::Dir) };
    if !ok {
        return fail(if kind_of(&path).is_some() || is_dev(&path) || netfs::is_net(&path) { "not a directory" } else { "file does not exist" });
    }
    let Some(fds) = sched::current_fds() else { return -1 };
    fds.with(|t| t.cwd = keep);
    0
}

fn seek_to(fd: u64, off: i64, whence: u64) -> Result<u64, &'static str> {
    let new = match chan_of(fd) {
        Some(Chan::File(f)) => {
            let base = match whence {
                0 => 0i64,
                1 => f.pos.with(|p| *p as i64),
                2 => ext4_size(&f.path).unwrap_or(0) as i64,
                _ => return Err("bad arg in system call"),
            };
            let Some(n) = base.checked_add(off).filter(|n| *n >= 0) else { return Err("bad arg in system call") };
            f.pos.with(|p| *p = n as u64);
            n
        }
        Some(Chan::Dir(d)) => {
            if off == 0 && whence == 0 {
                d.next.with(|x| *x = 0);
                0
            } else {
                return Err("bad arg in system call");
            }
        }
        Some(Chan::Mnt(f)) => {
            let base = match whence {
                0 => 0i64,
                1 => f.pos.with(|p| *p as i64),
                2 => mntfs::file_size(&f).unwrap_or(0) as i64,
                _ => return Err("bad arg in system call"),
            };
            let Some(n) = base.checked_add(off).filter(|n| *n >= 0) else { return Err("bad arg in system call") };
            f.pos.with(|p| *p = n as u64);
            n
        }
        Some(Chan::Text(t)) => {
            let base = match whence {
                0 => 0i64,
                1 => t.pos.with(|p| *p as i64),
                2 => t.data.len() as i64,
                _ => return Err("bad arg in system call"),
            };
            let Some(n) = base.checked_add(off).filter(|n| *n >= 0) else { return Err("bad arg in system call") };
            t.pos.with(|p| *p = n as u64);
            n
        }
        Some(_) => return Err("illegal seek"),
        None => return Err("fd out of range or not open"),
    };
    Ok(new as u64)
}

pub fn sys_seek(retp: u64, fd: u64, off: u64, whence: u64) -> i64 {
    let new = match seek_to(fd, off as i64, whence) {
        Ok(n) => n,
        Err(m) => return fail(m),
    };
    if !sched::copy_to_user(retp, &new.to_le_bytes()) {
        return fail("bad address in system call");
    }
    0
}

pub fn sys_oseek(fd: u64, off: u64, whence: u64) -> i64 {
    match seek_to(fd, off as u32 as i32 as i64, whence) {
        Ok(n) if n <= i32::MAX as u64 => n as i64,
        Ok(_) => fail("offset too large for oseek"),
        Err(m) => fail(m),
    }
}

fn put_stat(st: &Stat, buf: u64, n: u64) -> i64 {
    let rec = p9::pack_stat(st);
    if n as usize >= rec.len() {
        if !sched::copy_to_user(buf, &rec) {
            return fail("bad address in system call");
        }
        rec.len() as i64
    } else if n >= 2 {
        if !sched::copy_to_user(buf, &rec[..2]) {
            return fail("bad address in system call");
        }
        2
    } else {
        fail("stat buffer too small")
    }
}

pub fn sys_stat(path_ptr: u64, buf: u64, n: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    match stat_path(&user_path(&raw)) {
        Ok(st) => put_stat(&st, buf, n),
        Err(m) => fail(m),
    }
}

pub fn sys_fstat(fd: u64, buf: u64, n: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    match chan_stat(&chan) {
        Ok(st) => put_stat(&st, buf, n),
        Err(m) => fail(m),
    }
}

pub fn sys_fd2path(fd: u64, buf: u64, n: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    let mut p = chan_path(&chan).into_bytes();
    if p.len() + 1 > n as usize {
        return fail("buffer too small");
    }
    p.push(0);
    if !sched::copy_to_user(buf, &p) {
        return fail("bad address in system call");
    }
    0
}

pub fn sys_remove(path_ptr: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    let path = user_path_nofollow(&raw);
    if mntfs::is_remote(&path) {
        return match mntfs::remove(&path) {
            Ok(()) => 0,
            Err(m) => fail(m),
        };
    }
    if is_dev(&path) || netfs::is_net(&path) || is_env(&path) || is_env_child(&path) || path == "/" {
        return fail("permission denied");
    }
    if !may_write(&path) {
        return fail("permission denied");
    }
    let r = match if fs::is_symlink(&path) { Some(Kind::File) } else { kind_of(&path) } {
        None => return fail("file does not exist"),
        Some(Kind::File) => fs::with_root(|_| {
            Ext4File::new(&path, InodeTypes::EXT4_DE_REG_FILE).file_remove(&path).map(|_| ()).map_err(p9::errno_msg)
        })
        .unwrap_or(Err("no file system")),
        Some(Kind::Dir) => match ext4_names(&path) {
            Ok(v) if !v.is_empty() => Err("directory not empty"),
            Ok(_) => fs::with_root(|_| {
                Ext4File::new(&path, InodeTypes::EXT4_DE_DIR).dir_rm(&path).map(|_| ()).map_err(p9::errno_msg)
            })
            .unwrap_or(Err("no file system")),
            Err(m) => Err(m),
        },
    };
    match r {
        Ok(()) => 0,
        Err(m) => fail(m),
    }
}

struct WStat {
    mode: u32,
    mtime: u32,
    length: u64,
    name: String,
}

fn parse_wstat(buf: u64, n: u64) -> Result<WStat, &'static str> {
    let n = n as usize;
    if !(p9::STATFIXLEN..=4096).contains(&n) {
        return Err("bad stat buffer");
    }
    let mut b = vec![0u8; n];
    if !sched::copy_from_user(buf, &mut b) {
        return Err("bad address in system call");
    }
    let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mode = u32at(21);
    let mtime = u32at(29);
    let mut l = [0u8; 8];
    l.copy_from_slice(&b[33..41]);
    let length = u64::from_le_bytes(l);
    let nl = u16::from_le_bytes([b[41], b[42]]) as usize;
    if 43 + nl > n {
        return Err("bad stat buffer");
    }
    let name = String::from_utf8(b[43..43 + nl].to_vec()).map_err(|_| "bad stat buffer")?;
    Ok(WStat { mode, mtime, length, name })
}

fn apply_wstat(path: &str, w: &WStat) -> Result<(), &'static str> {
    if mntfs::is_remote(path) {
        return mntfs::wstat_fields(path, w.mode, w.mtime, w.length, &w.name);
    }
    if is_dev(path) || netfs::is_net(path) || is_env(path) || is_env_child(path) || path == "/" {
        return Err("permission denied");
    }
    if !may_write(path) {
        return Err("permission denied");
    }
    let kind = kind_of(path).or_else(|| fs::is_symlink(path).then_some(Kind::File)).ok_or("file does not exist")?;
    if w.length != u64::MAX {
        let cur = if kind == Kind::File { ext4_size(path).unwrap_or(0) } else { 0 };
        if w.length != cur {
            return Err("cannot change length");
        }
    }
    if w.mode != u32::MAX {
        let perm = w.mode & 0o777;
        fs::with_root(|_| {
            let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
            f.file_mode_set(perm).map(|_| ()).map_err(p9::errno_msg)
        })
        .unwrap_or(Err("no file system"))?;
    }
    if w.mtime != u32::MAX {
        let c = alloc::ffi::CString::new(path).map_err(|_| "bad file name")?;
        let r = fs::with_root(|_| {
            unsafe { lwext4_rust::bindings::ext4_mtime_set(c.as_ptr(), w.mtime) }
        })
        .ok_or("no file system")?;
        if r != 0 {
            return Err(p9::errno_msg(r));
        }
    }
    if !w.name.is_empty() && w.name != p9::base_name(path) {
        if w.name.contains('/') {
            return Err("bad file name");
        }
        let dir = match path.rfind('/') {
            Some(0) | None => String::from("/"),
            Some(i) => path[..i].to_string(),
        };
        let to = child_path(&dir, &w.name);
        if kind_of(&to).is_some() {
            return Err("file already exists");
        }
        fs::with_root(|_| {
            let mut f = Ext4File::new(path, InodeTypes::EXT4_DE_REG_FILE);
            f.file_rename(path, &to).map(|_| ()).map_err(p9::errno_msg)
        })
        .unwrap_or(Err("no file system"))?;
    }
    Ok(())
}

fn forward_wstat(path: &str, buf: u64, n: u64) -> i64 {
    let n = n as usize;
    if !(p9::STATFIXLEN..=65535).contains(&n) {
        return fail("bad stat buffer");
    }
    let mut b = vec![0u8; n];
    if !sched::copy_from_user(buf, &mut b) {
        return fail("bad address in system call");
    }
    match mntfs::wstat_raw(path, &b) {
        Ok(()) => 0,
        Err(m) => fail(m),
    }
}

pub fn sys_wstat(path_ptr: u64, buf: u64, n: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    let rp = user_path_nofollow(&raw);
    if mntfs::is_remote(&rp) {
        return forward_wstat(&rp, buf, n);
    }
    let w = match parse_wstat(buf, n) {
        Ok(w) => w,
        Err(m) => return fail(m),
    };
    let path = if w.name.is_empty() { user_path(&raw) } else { user_path_nofollow(&raw) };
    match apply_wstat(&path, &w) {
        Ok(()) => 0,
        Err(m) => fail(m),
    }
}

pub fn sys_fwstat(fd: u64, buf: u64, n: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    if let Chan::Mnt(f) = &chan {
        return forward_wstat(&f.path, buf, n);
    }
    let w = match parse_wstat(buf, n) {
        Ok(w) => w,
        Err(m) => return fail(m),
    };
    match apply_wstat(&chan_path(&chan), &w) {
        Ok(()) => 0,
        Err(m) => fail(m),
    }
}

const OLD_NAMELEN: usize = 28;
const OLD_DIRLEN: usize = 116;

fn old_dir(st: &Stat) -> [u8; OLD_DIRLEN] {
    let mut d = [0u8; OLD_DIRLEN];
    let name = st.name.as_bytes();
    let n = name.len().min(OLD_NAMELEN - 1);
    d[..n].copy_from_slice(&name[..n]);
    for who in [OLD_NAMELEN, 2 * OLD_NAMELEN] {
        d[who..who + 4].copy_from_slice(b"none");
    }
    let chdir: u32 = if st.is_dir { 0x8000_0000 } else { 0 };
    let qpath = (st.qid_path as u32 & 0x7fff_ffff) | chdir;
    let mode = (st.perm & 0o777) | chdir;
    d[84..88].copy_from_slice(&qpath.to_le_bytes());
    d[88..92].copy_from_slice(&0u32.to_le_bytes());
    d[92..96].copy_from_slice(&mode.to_le_bytes());
    d[96..100].copy_from_slice(&st.mtime.to_le_bytes());
    d[100..104].copy_from_slice(&st.mtime.to_le_bytes());
    d[104..112].copy_from_slice(&st.length.to_le_bytes());
    d
}

fn put_old_stat(st: &Stat, buf: u64) -> i64 {
    if !sched::copy_to_user(buf, &old_dir(st)) {
        return fail("bad address in system call");
    }
    0
}

pub fn sys_old_stat(path_ptr: u64, buf: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    match stat_path(&user_path(&raw)) {
        Ok(st) => put_old_stat(&st, buf),
        Err(m) => fail(m),
    }
}

pub fn sys_old_fstat(fd: u64, buf: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    match chan_stat(&chan) {
        Ok(st) => put_old_stat(&st, buf),
        Err(m) => fail(m),
    }
}

fn parse_old_wstat(buf: u64) -> Result<WStat, &'static str> {
    let mut b = [0u8; OLD_DIRLEN];
    if !sched::copy_from_user(buf, &mut b) {
        return Err("bad address in system call");
    }
    let end = b[..OLD_NAMELEN].iter().position(|&c| c == 0).unwrap_or(OLD_NAMELEN);
    let name = String::from_utf8(b[..end].to_vec()).map_err(|_| "bad stat buffer")?;
    let u32at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let mut l = [0u8; 8];
    l.copy_from_slice(&b[104..112]);
    Ok(WStat { mode: u32at(92), mtime: u32at(100), length: u64::from_le_bytes(l), name })
}

pub fn sys_old_wstat(path_ptr: u64, buf: u64) -> i64 {
    let Some(raw) = user_name(path_ptr) else { return fail("bad address in system call") };
    let w = match parse_old_wstat(buf) {
        Ok(w) => w,
        Err(m) => return fail(m),
    };
    let renames = !w.name.is_empty() && w.name != p9::base_name(&user_path_nofollow(&raw));
    let path = if renames { user_path_nofollow(&raw) } else { user_path(&raw) };
    match apply_wstat(&path, &w) {
        Ok(()) => 0,
        Err(m) => fail(m),
    }
}

pub fn sys_old_fwstat(fd: u64, buf: u64) -> i64 {
    let Some(chan) = chan_of(fd) else { return fail("fd out of range or not open") };
    let w = match parse_old_wstat(buf) {
        Ok(w) => w,
        Err(m) => return fail(m),
    };
    match apply_wstat(&chan_path(&chan), &w) {
        Ok(()) => 0,
        Err(m) => fail(m),
    }
}

pub fn path_is_dir(p: &str) -> Option<bool> {
    stat_path(p).ok().map(|s| s.is_dir)
}
