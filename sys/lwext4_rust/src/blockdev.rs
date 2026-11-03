//! Glue between lwext4's C block-device interface and a Rust byte-cursor
//! device ([`KernelDevOp`]).

use crate::bindings::*;
use alloc::boxed::Box;
use core::ffi::{c_char, c_int, c_void};
use core::ptr::null_mut;
use core::slice::{from_raw_parts, from_raw_parts_mut};

/// Physical sector size lwext4 is told to use.
pub const EXT4_DEV_BSIZE: u32 = 512;

/// A byte-addressed device, as seen by lwext4.
///
/// Errors are `errno` values.
pub trait KernelDevOp {
    type DevType;

    fn write(dev: &mut Self::DevType, buf: &[u8]) -> Result<usize, i32>;
    fn read(dev: &mut Self::DevType, buf: &mut [u8]) -> Result<usize, i32>;
    fn seek(dev: &mut Self::DevType, off: i64, whence: i32) -> Result<i64, i32>;
    fn flush(dev: &mut Self::DevType) -> Result<usize, i32>
    where
        Self: Sized;
}

/// lwext4 keeps global mount and device tables, so there is one of these per
/// mounted device and callers must serialise access (it is not thread safe).
pub struct Ext4BlockWrapper<K: KernelDevOp> {
    value: Box<ext4_blockdev>,
    bc_alloc: *mut ext4_bcache,
    name: [u8; 16],
    mount_point: [u8; 4],
    mounted: bool,
    released: bool,
    pd: core::marker::PhantomData<K>,
}

impl<K: KernelDevOp> Ext4BlockWrapper<K> {
    /// Register `block_dev` with lwext4, mount it at `/`, replay the journal
    /// if there is one and enable the write-back cache.
    pub fn new(block_dev: K::DevType) -> Result<Self, i32> {
        let devt_user = Box::into_raw(Box::new(block_dev)) as *mut c_void;
        let bbuf = Box::into_raw(Box::new([0u8; EXT4_DEV_BSIZE as usize]));

        let iface = Box::into_raw(Box::new(ext4_blockdev_iface {
            open: Some(Self::dev_open),
            bread: Some(Self::dev_bread),
            bwrite: Some(Self::dev_bwrite),
            close: Some(Self::dev_close),
            lock: None,
            unlock: None,
            ph_bsize: EXT4_DEV_BSIZE,
            ph_bcnt: 0,
            ph_bbuf: bbuf as *mut u8,
            ph_refctr: 0,
            bread_ctr: 0,
            bwrite_ctr: 0,
            p_user: devt_user,
        }));
        // The block cache is filled in by lwext4 at mount time; it only needs
        // to be zeroed memory that outlives the mount.
        let bc: *mut ext4_bcache = Box::into_raw(Box::new(unsafe { core::mem::zeroed() }));

        let mut this = Self {
            bc_alloc: bc,
            value: Box::new(ext4_blockdev {
                bdif: iface,
                part_offset: 0,
                part_size: 0,
                bc,
                lg_bsize: 0,
                lg_bcnt: 0,
                cache_write_back: 0,
                fs: null_mut(),
                journal: null_mut(),
            }),
            name: *b"ext4_fs\0\0\0\0\0\0\0\0\0",
            mount_point: *b"/\0\0\0",
            mounted: false,
            released: false,
            pd: core::marker::PhantomData,
        };

        // On error `this` is dropped, which frees everything above.
        unsafe { this.mount()? };
        Ok(this)
    }

    unsafe fn mount(&mut self) -> Result<(), i32> {
        let name = self.name.as_ptr() as *const c_char;
        let mp = self.mount_point.as_ptr() as *const c_char;

        let r = unsafe { ext4_device_register(self.value.as_mut(), name) };
        if r != EOK as c_int {
            error!("lwext4: device_register failed: {}", r);
            return Err(r);
        }
        let r = unsafe { ext4_mount(name, mp, false) };
        if r != EOK as c_int {
            error!("lwext4: mount failed: {}", r);
            unsafe { ext4_device_unregister(name) };
            return Err(r);
        }
        self.mounted = true;

        let r = unsafe { ext4_recover(mp) };
        if r != EOK as c_int && r != ENOTSUP as c_int {
            error!("lwext4: journal recovery failed: {}", r);
            return Err(r);
        }
        let r = unsafe { ext4_journal_start(mp) };
        if r != EOK as c_int {
            error!("lwext4: journal start failed: {}", r);
            return Err(r);
        }
        unsafe { ext4_cache_write_back(mp, true) };
        info!("lwext4: mounted");
        Ok(())
    }

    /// Mount point as a C string (`"/"`).
    pub fn mount_point_cstr(&self) -> *const c_char {
        self.mount_point.as_ptr() as *const c_char
    }

    /// Flush the block cache and the device so everything written so far is
    /// durable.
    pub fn sync(&mut self) -> Result<(), i32> {
        let r = unsafe { ext4_cache_flush(self.mount_point_cstr()) };
        if r != EOK as c_int {
            return Err(r);
        }
        let dev = unsafe { &mut *((*self.value.bdif).p_user as *mut K::DevType) };
        K::flush(dev).map(|_| ())
    }

    unsafe extern "C" fn dev_open(bdev: *mut ext4_blockdev) -> c_int {
        let devt = unsafe { &mut *((*(*bdev).bdif).p_user as *mut K::DevType) };
        let size = match K::seek(devt, 0, SEEK_END as i32) {
            Ok(v) if v >= 0 => v as u64,
            _ => return EFAULT as c_int,
        };
        unsafe {
            (*bdev).part_offset = 0;
            (*bdev).part_size = size;
            (*(*bdev).bdif).ph_bcnt = size / (*(*bdev).bdif).ph_bsize as u64;
        }
        EOK as c_int
    }

    unsafe fn xfer<'a>(bdev: *mut ext4_blockdev, blk_id: u64, blk_cnt: u32) -> Option<(&'a mut K::DevType, usize)> {
        let bsize = unsafe { (*(*bdev).bdif).ph_bsize } as u64;
        let off = blk_id.checked_mul(bsize)?;
        let len = (blk_cnt as u64).checked_mul(bsize)?;
        let devt = unsafe { &mut *((*(*bdev).bdif).p_user as *mut K::DevType) };
        K::seek(devt, i64::try_from(off).ok()?, SEEK_SET as i32).ok()?;
        Some((devt, usize::try_from(len).ok()?))
    }

    unsafe extern "C" fn dev_bread(bdev: *mut ext4_blockdev, buf: *mut c_void, blk_id: u64, blk_cnt: u32) -> c_int {
        let Some((devt, len)) = (unsafe { Self::xfer(bdev, blk_id, blk_cnt) }) else { return EIO as c_int };
        if len == 0 {
            return EOK as c_int;
        }
        match K::read(devt, unsafe { from_raw_parts_mut(buf as *mut u8, len) }) {
            Ok(n) if n == len => EOK as c_int,
            _ => EIO as c_int,
        }
    }

    unsafe extern "C" fn dev_bwrite(bdev: *mut ext4_blockdev, buf: *const c_void, blk_id: u64, blk_cnt: u32) -> c_int {
        let Some((devt, len)) = (unsafe { Self::xfer(bdev, blk_id, blk_cnt) }) else { return EIO as c_int };
        if len == 0 {
            return EOK as c_int;
        }
        match K::write(devt, unsafe { from_raw_parts(buf as *const u8, len) }) {
            Ok(n) if n == len => EOK as c_int,
            _ => EIO as c_int,
        }
    }

    unsafe extern "C" fn dev_close(_bdev: *mut ext4_blockdev) -> c_int {
        EOK as c_int
    }
}

impl<K: KernelDevOp> Ext4BlockWrapper<K> {
    /// Flush, unmount and give the device back (e.g. to mount it again).
    pub fn into_device(mut self) -> K::DevType {
        *self.teardown()
    }

    /// Unmount and free every lwext4-side allocation; returns the device.
    fn teardown(&mut self) -> Box<K::DevType> {
        assert!(!self.released);
        self.released = true;
        let name = self.name.as_ptr() as *const c_char;
        let mp = self.mount_point.as_ptr() as *const c_char;
        unsafe {
            if self.mounted {
                ext4_cache_write_back(mp, false);
                let r = ext4_journal_stop(mp);
                if r != EOK as c_int {
                    error!("lwext4: journal_stop: {}", r);
                }
                let r = ext4_umount(mp);
                if r != EOK as c_int {
                    error!("lwext4: umount: {}", r);
                }
                ext4_device_unregister(name);
                self.mounted = false;
            }
            // Release what `new` leaked into raw pointers.
            let iface = self.value.bdif;
            let dev = Box::from_raw((*iface).p_user as *mut K::DevType);
            drop(Box::from_raw((*iface).ph_bbuf as *mut [u8; EXT4_DEV_BSIZE as usize]));
            drop(Box::from_raw(iface));
            drop(Box::from_raw(self.bc_alloc));
            dev
        }
    }
}

unsafe impl<K: KernelDevOp> Send for Ext4BlockWrapper<K> where K::DevType: Send {}

impl<K: KernelDevOp> Drop for Ext4BlockWrapper<K> {
    fn drop(&mut self) {
        if !self.released {
            drop(self.teardown());
        }
    }
}
