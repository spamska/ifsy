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
use alloc::sync::Arc;
use alloc::vec::Vec;

use p9fcall::{MAFTER, MBEFORE, MCREATE, MORDER, MREPL};

use crate::mnt::MntChan;
use crate::sync::SpinLock;

pub const EMOUNT: &str = "inconsistent mount";
pub const EUNMOUNT: &str = "not mounted";
pub const EUNION: &str = "not in union";
pub const ENOCREATE: &str = "mounted directory forbids creation";

#[derive(Clone)]
pub struct Mount {
    pub to: String,
    pub mflag: u32,
    pub spec: String,
    pub src: String,
    pub root: Option<Arc<MntChan>>,
}

#[derive(Clone)]
pub struct Mhead {
    pub from: String,
    pub mount: Vec<Mount>,
}

#[derive(Clone, Default)]
pub struct Ns {
    heads: Vec<Mhead>,
}

pub type SharedNs = Arc<SpinLock<Ns>>;

pub fn new_ns() -> SharedNs {
    Arc::new(SpinLock::new(Ns::default()))
}

pub fn copy_ns(ns: &SharedNs) -> SharedNs {
    Arc::new(SpinLock::new(ns.with(|n| n.clone())))
}

pub struct Walked {
    pub path: String,
    pub union: Option<Vec<(String, u32)>>,
}

pub fn join(base: &str, comp: &str) -> String {
    if base == "/" {
        let mut s = String::from("/");
        s.push_str(comp);
        s
    } else {
        let mut s = String::from(base);
        s.push('/');
        s.push_str(comp);
        s
    }
}

impl Ns {
    pub fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }

    fn findmount(&self, concrete: &str) -> Option<&Mhead> {
        self.heads.iter().find(|h| h.from == concrete)
    }

    pub fn walk(&self, visible: &str, apply_last: bool, exists: &dyn Fn(&str) -> bool) -> Walked {
        if self.heads.is_empty() {
            return Walked { path: String::from(visible), union: None };
        }
        let comps: Vec<&str> = visible.split('/').filter(|c| !c.is_empty()).collect();
        let mut cur = String::from("/");
        let mut members: Option<Vec<(String, u32)>> = None;
        if apply_last || !comps.is_empty() {
            if let Some(h) = self.findmount(&cur) {
                let ms: Vec<(String, u32)> = h.mount.iter().map(|m| (m.to.clone(), m.mflag)).collect();
                if let Some(first) = ms.first() {
                    cur = first.0.clone();
                }
                members = Some(ms);
            }
        }
        let n = comps.len();
        for (i, comp) in comps.iter().enumerate() {
            let last = i + 1 == n;
            let cands: Vec<String> = match &members {
                Some(ms) => ms.iter().map(|m| join(&m.0, comp)).collect(),
                None => alloc::vec![join(&cur, comp)],
            };
            let pick = if cands.len() == 1 {
                cands[0].clone()
            } else {
                cands.iter().find(|c| exists(c)).unwrap_or(&cands[0]).clone()
            };
            cur = pick;
            members = None;
            if apply_last || !last {
                if let Some(h) = self.findmount(&cur) {
                    let ms: Vec<(String, u32)> = h.mount.iter().map(|m| (m.to.clone(), m.mflag)).collect();
                    if let Some(first) = ms.first() {
                        cur = first.0.clone();
                    }
                    members = Some(ms);
                }
            }
        }
        Walked { path: cur, union: members }
    }

    pub fn cmount(
        &mut self,
        new: &Walked,
        old: &str,
        flag: u32,
        spec: &str,
        src: &str,
        root: Option<Arc<MntChan>>,
        new_is_dir: bool,
        old_is_dir: bool,
    ) -> Result<(), &'static str> {
        if new_is_dir != old_is_dir {
            return Err(EMOUNT);
        }
        let order = flag & MORDER;
        if !old_is_dir && order != MREPL {
            return Err(EMOUNT);
        }

        let mut nm: Vec<Mount> = Vec::new();
        nm.push(Mount { to: new.path.clone(), mflag: flag, spec: String::from(spec), src: String::from(src), root });
        if let Some(um) = &new.union {
            if flag & MCREATE != 0 && (um.len() > 1 || um[0].1 & MCREATE == 0) {
                return Err(EMOUNT);
            }
            let eff = if order == MREPL { MAFTER } else { order };
            for (to, _) in um.iter().skip(1) {
                nm.push(Mount { to: to.clone(), mflag: eff, spec: String::from(spec), src: String::from(src), root: None });
            }
        }

        let pos = self.heads.iter().position(|h| h.from == old);
        let idx = match pos {
            Some(i) => i,
            None => {
                let mut mh = Mhead { from: String::from(old), mount: Vec::new() };
                if order != MREPL {
                    mh.mount.push(Mount { to: String::from(old), mflag: 0, spec: String::new(), src: String::new(), root: None });
                }
                self.heads.push(mh);
                self.heads.len() - 1
            }
        };
        let m = &mut self.heads[idx];
        if !m.mount.is_empty() && order == MAFTER {
            m.mount.extend(nm);
        } else if order != MREPL {
            let mut v = nm;
            v.extend(core::mem::take(&mut m.mount));
            m.mount = v;
        } else {
            m.mount = nm;
        }
        Ok(())
    }

    pub fn cunmount(&mut self, old: &str, mounted: Option<(&str, &str)>) -> Result<(), &'static str> {
        let Some(hi) = self.heads.iter().position(|h| h.from == old) else { return Err(EUNMOUNT) };
        match mounted {
            None => {
                self.heads.remove(hi);
                Ok(())
            }
            Some((concrete, visible)) => {
                let h = &mut self.heads[hi];
                let Some(mi) = h.mount.iter().position(|f| f.to == concrete || (!f.src.is_empty() && f.src == visible)) else {
                    return Err(EUNION);
                };
                h.mount.remove(mi);
                if h.mount.is_empty() {
                    self.heads.remove(hi);
                }
                Ok(())
            }
        }
    }
}

pub fn createdir(members: &[(String, u32)]) -> Result<String, &'static str> {
    members.iter().find(|m| m.1 & MCREATE != 0).map(|m| m.0.clone()).ok_or(ENOCREATE)
}

pub fn ordered(flag: u32) -> bool {
    flag & MORDER != MBEFORE | MAFTER
}
