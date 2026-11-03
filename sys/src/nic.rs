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

use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::e1000::{self, E1000, E1000Rx, E1000Tx};
use crate::rtl8139::{self, Rtl8139, Rtl8139Rx, Rtl8139Tx};
use crate::virtio_net::{self, NetRx, NetTx, VirtioNet};

pub enum Nic {
    Virtio(VirtioNet),
        E1000(E1000),
        Rtl8139(Rtl8139),
}

pub enum NicRx<'a> {
    Virtio(NetRx<'a>),
        E1000(E1000Rx<'a>),
        Rtl8139(Rtl8139Rx<'a>),
}

pub enum NicTx<'a> {
    Virtio(NetTx<'a>),
        E1000(E1000Tx<'a>),
        Rtl8139(Rtl8139Tx<'a>),
}

impl RxToken for NicRx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        match self {
            NicRx::Virtio(t) => t.consume(f),
                        NicRx::E1000(t) => t.consume(f),
                        NicRx::Rtl8139(t) => t.consume(f),
        }
    }
}

impl TxToken for NicTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        match self {
            NicTx::Virtio(t) => t.consume(len, f),
                        NicTx::E1000(t) => t.consume(len, f),
                        NicTx::Rtl8139(t) => t.consume(len, f),
        }
    }
}

impl Nic {
    pub fn mac(&self) -> [u8; 6] {
        match self {
            Nic::Virtio(d) => d.mac(),
                        Nic::E1000(d) => d.mac(),
                        Nic::Rtl8139(d) => d.mac(),
        }
    }
}

impl Device for Nic {
    type RxToken<'a> = NicRx<'a>;
    type TxToken<'a> = NicTx<'a>;

    fn receive(&mut self, ts: Instant) -> Option<(NicRx<'_>, NicTx<'_>)> {
        match self {
            Nic::Virtio(d) => d.receive(ts).map(|(r, t)| (NicRx::Virtio(r), NicTx::Virtio(t))),
                        Nic::E1000(d) => d.receive(ts).map(|(r, t)| (NicRx::E1000(r), NicTx::E1000(t))),
                        Nic::Rtl8139(d) => d.receive(ts).map(|(r, t)| (NicRx::Rtl8139(r), NicTx::Rtl8139(t))),
        }
    }

    fn transmit(&mut self, ts: Instant) -> Option<NicTx<'_>> {
        match self {
            Nic::Virtio(d) => d.transmit(ts).map(NicTx::Virtio),
                        Nic::E1000(d) => d.transmit(ts).map(NicTx::E1000),
                        Nic::Rtl8139(d) => d.transmit(ts).map(NicTx::Rtl8139),
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        match self {
            Nic::Virtio(d) => d.capabilities(),
                        Nic::E1000(d) => d.capabilities(),
                        Nic::Rtl8139(d) => d.capabilities(),
        }
    }
}

pub fn probe() -> Option<Nic> {
    if let Some(d) = virtio_net::probe() {
        return Some(Nic::Virtio(d));
    }
        {
        if let Some(d) = e1000::probe() {
            return Some(Nic::E1000(d));
        }
        if let Some(d) = rtl8139::probe() {
            return Some(Nic::Rtl8139(d));
        }
    }
    None
}
