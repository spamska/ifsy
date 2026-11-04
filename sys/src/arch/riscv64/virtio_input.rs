//! Polled virtio-input keyboard on the QEMU `virt` virtio-mmio bus, so the
//! machine can be typed on in the QEMU window (run QEMU with
//! `-device virtio-keyboard-device`) and not only over the serial line.
//!
//! The eight virtio-mmio slots sit at `0x1000_1000 + n * 0x1000`. Limine does
//! not map device MMIO, so the slots are mapped into the kernel VM window on
//! first use. Both the legacy (version 1) and modern (version 2) register
//! layouts are driven; QEMU defaults to legacy on this board. No interrupt is
//! used: `read_byte` is called by the console's input pump and drains the
//! event queue.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use crate::console::keymap::Keymap;
use crate::mm::{self, PageFlags, PhysAddr};

const PAGE_SIZE: u64 = 4096;
use crate::sync::SpinLock;

const MMIO_BASE: u64 = 0x1000_1000;
const MMIO_SLOTS: u64 = 8;

const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const GUEST_PAGE_SIZE: usize = 0x028; // legacy
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_ALIGN: usize = 0x03c; // legacy
const QUEUE_PFN: usize = 0x040; // legacy
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;

const MAGIC_VALUE: u32 = 0x7472_6976; // "virt"
const ID_INPUT: u32 = 18;

const ST_ACK: u32 = 1;
const ST_DRIVER: u32 = 2;
const ST_DRIVER_OK: u32 = 4;
const ST_FEATURES_OK: u32 = 8;

const EV_KEY: u16 = 1;

const QSIZE: usize = 16;
const DESC_WRITE: u16 = 2;
const EVENT_SIZE: usize = 8; // le16 type, le16 code, le32 value

// Layout of the three pages handed to the device (legacy needs the used ring
// on the next page boundary; modern is happy with it too).
const OFF_DESC: usize = 0;
const OFF_AVAIL: usize = 256;
const OFF_USED: usize = 4096;
const OFF_EVENTS: usize = 2 * 4096;

struct Kbd {
    regs: u64,
    mem: u64,
    mem_phys: u64,
    last_used: u16,
    keys: Keymap,
}

enum State {
    Untried,
    Absent,
    Ready(Kbd),
}

static STATE: SpinLock<State> = SpinLock::new(State::Untried);

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn rd(regs: u64, off: usize) -> u32 {
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    unsafe { read_volatile((regs + off as u64) as *const u32) }
}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn wr(regs: u64, off: usize, v: u32) {
    // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
    unsafe { write_volatile((regs + off as u64) as *mut u32, v) }
}

/// The next typed character, if the keyboard has one waiting.
pub fn read_byte() -> Option<u8> {
    STATE.with(|st| {
        if matches!(st, State::Untried) {
            if !mm::paging_ready() {
                return None; // too early; try again later
            }
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            *st = match unsafe { init() } {
                Some(k) => State::Ready(k),
                None => State::Absent,
            };
        }
        match st {
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            State::Ready(k) => unsafe { k.poll() },
            _ => None,
        }
    })
}

/// Find the keyboard and bring it up.
/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
unsafe fn init() -> Option<Kbd> {
    let va = mm::aspace::kvm::map_phys(PhysAddr(MMIO_BASE), MMIO_SLOTS, PageFlags::KERNEL_RW)?;
    for slot in 0..MMIO_SLOTS {
        let regs = va + slot * PAGE_SIZE;
        // SAFETY: MMIO/port/DMA access to the device this driver probed: addresses are its mapped register window or DMA pages allocated here, offsets are constants inside them, and the driver has exclusive access (&mut self).
        unsafe {
            if rd(regs, MAGIC) != MAGIC_VALUE || rd(regs, DEVICE_ID) != ID_INPUT {
                continue;
            }
            // Of several input devices, the first one that is a keyboard wins;
            // QEMU's keyboard and tablet/mouse look alike here, so just take
            // the first and let non-key events (type != EV_KEY) be ignored.
            if let Some(k) = setup(regs) {
                crate::kprintln!(
                    "ifsy: input: virtio keyboard at {:#x} {}",
                    MMIO_BASE + slot * PAGE_SIZE,
                    crate::console::color::pass()
                );
                return Some(k);
            }
        }
    }
    None
}

/// # Safety
/// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
unsafe fn setup(regs: u64) -> Option<Kbd> {
    let version = unsafe { rd(regs, VERSION) };
    if version != 1 && version != 2 {
        return None;
    }
    let hhdm = mm::hhdm();
    let phys = mm::with_frames(|f| f.alloc_contiguous(3, 1))?;
    let mem = phys.0 + hhdm;
    // SAFETY: pointer and length come from memory owned by this code (frames mapped via the HHDM or a live slice) sized by the same computation, and no other reference to it is live.
    unsafe { core::ptr::write_bytes(mem as *mut u8, 0, 3 * PAGE_SIZE as usize) };

    unsafe {
        wr(regs, STATUS, 0);
        wr(regs, STATUS, ST_ACK);
        wr(regs, STATUS, ST_ACK | ST_DRIVER);

        if version == 2 {
            // Only VIRTIO_F_VERSION_1 (bit 32) is wanted.
            wr(regs, DEVICE_FEATURES_SEL, 1);
            let hi = rd(regs, DEVICE_FEATURES);
            if hi & 1 == 0 {
                return None;
            }
            wr(regs, DRIVER_FEATURES_SEL, 0);
            wr(regs, DRIVER_FEATURES, 0);
            wr(regs, DRIVER_FEATURES_SEL, 1);
            wr(regs, DRIVER_FEATURES, 1);
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_FEATURES_OK);
            if rd(regs, STATUS) & ST_FEATURES_OK == 0 {
                return None;
            }
        } else {
            wr(regs, DRIVER_FEATURES, 0);
            wr(regs, GUEST_PAGE_SIZE, PAGE_SIZE as u32);
        }

        // Queue 0 is the event queue.
        wr(regs, QUEUE_SEL, 0);
        if rd(regs, QUEUE_NUM_MAX) < QSIZE as u32 {
            return None;
        }
        wr(regs, QUEUE_NUM, QSIZE as u32);
        if version == 1 {
            wr(regs, QUEUE_ALIGN, PAGE_SIZE as u32);
            wr(regs, QUEUE_PFN, (phys.0 / PAGE_SIZE) as u32);
        } else {
            let (d, a, u) = (phys.0 + OFF_DESC as u64, phys.0 + OFF_AVAIL as u64, phys.0 + OFF_USED as u64);
            wr(regs, QUEUE_DESC_LOW, d as u32);
            wr(regs, QUEUE_DESC_HIGH, (d >> 32) as u32);
            wr(regs, QUEUE_DRIVER_LOW, a as u32);
            wr(regs, QUEUE_DRIVER_HIGH, (a >> 32) as u32);
            wr(regs, QUEUE_DEVICE_LOW, u as u32);
            wr(regs, QUEUE_DEVICE_HIGH, (u >> 32) as u32);
        }

        // Every descriptor is a device-writable event buffer, all offered up front.
        let desc = (mem as usize + OFF_DESC) as *mut u8;
        let avail = (mem as usize + OFF_AVAIL) as *mut u16;
        for i in 0..QSIZE {
            let d = desc.add(i * 16);
            let addr = phys.0 + (OFF_EVENTS + i * EVENT_SIZE) as u64;
            write_volatile(d as *mut u64, addr);
            write_volatile(d.add(8) as *mut u32, EVENT_SIZE as u32);
            write_volatile(d.add(12) as *mut u16, DESC_WRITE);
            write_volatile(d.add(14) as *mut u16, 0);
            write_volatile(avail.add(2 + i), i as u16);
        }
        fence(Ordering::SeqCst);
        write_volatile(avail.add(1), QSIZE as u16);
        fence(Ordering::SeqCst);

        if version == 2 {
            wr(regs, QUEUE_READY, 1);
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_FEATURES_OK | ST_DRIVER_OK);
        } else {
            wr(regs, STATUS, ST_ACK | ST_DRIVER | ST_DRIVER_OK);
        }
        wr(regs, QUEUE_NOTIFY, 0);
    }
    Some(Kbd { regs, mem, mem_phys: phys.0, last_used: 0, keys: Keymap::new() })
}

impl Kbd {
    /// Drain finished events until one yields a character.
    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    unsafe fn poll(&mut self) -> Option<u8> {
        let used = (self.mem as usize + OFF_USED) as *const u8;
        let avail = (self.mem as usize + OFF_AVAIL) as *mut u16;
        loop {
            fence(Ordering::SeqCst);
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            let idx = unsafe { read_volatile(used.add(2) as *const u16) };
            if idx == self.last_used {
                return None;
            }
            let slot = self.last_used as usize % QSIZE;
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            let id = unsafe { read_volatile(used.add(4 + slot * 8) as *const u32) } as usize % QSIZE;
            self.last_used = self.last_used.wrapping_add(1);

            let ev = (self.mem as usize + OFF_EVENTS + id * EVENT_SIZE) as *const u8;
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            let (ty, code, value) = unsafe {
                (read_volatile(ev as *const u16), read_volatile(ev.add(2) as *const u16), read_volatile(ev.add(4) as *const u32))
            };

            // Hand the buffer back to the device.
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            unsafe {
                let aidx = read_volatile(avail.add(1));
                write_volatile(avail.add(2 + aidx as usize % QSIZE), id as u16);
                fence(Ordering::SeqCst);
                write_volatile(avail.add(1), aidx.wrapping_add(1));
                fence(Ordering::SeqCst);
                let st = rd(self.regs, INTERRUPT_STATUS);
                if st != 0 {
                    wr(self.regs, INTERRUPT_ACK, st);
                }
                wr(self.regs, QUEUE_NOTIFY, 0);
            }

            if ty == EV_KEY {
                if let Some(c) = self.keys.key(code, value) {
                    return Some(c);
                }
            }
        }
    }
}
