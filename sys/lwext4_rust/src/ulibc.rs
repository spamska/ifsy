//! The slice of libc that lwext4 needs, on top of the kernel's global
//! allocator. lwext4 is built with `CONFIG_USE_USER_MALLOC=1`, so allocation
//! goes through the `ext4_user_*` hooks below instead of global `malloc`.
//!
//! `mem{cpy,move,set,cmp}` come from `compiler_builtins`.

use alloc::alloc::{alloc, alloc_zeroed, dealloc, Layout};
use core::ffi::{c_char, c_int, c_void};

/// Every allocation is preceded by a header holding its size; 16 bytes keeps
/// the user pointer 16-byte aligned.
const HDR: usize = 16;
const ALIGN: usize = 16;

fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HDR)?, ALIGN).ok()
}

unsafe fn alloc_with(size: usize, zero: bool) -> *mut c_void {
    let Some(l) = layout(size) else { return core::ptr::null_mut() };
    let p = unsafe { if zero { alloc_zeroed(l) } else { alloc(l) } };
    if p.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        (p as *mut usize).write(size);
        p.add(HDR) as *mut c_void
    }
}

#[no_mangle]
pub unsafe extern "C" fn ext4_user_malloc(size: usize) -> *mut c_void {
    unsafe { alloc_with(size, false) }
}

#[no_mangle]
pub unsafe extern "C" fn ext4_user_calloc(n: usize, size: usize) -> *mut c_void {
    match n.checked_mul(size) {
        Some(total) => unsafe { alloc_with(total, true) },
        None => core::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn ext4_user_free(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    unsafe {
        let base = (p as *mut u8).sub(HDR);
        let size = (base as *const usize).read();
        dealloc(base, layout(size).unwrap());
    }
}

#[no_mangle]
pub unsafe extern "C" fn ext4_user_realloc(p: *mut c_void, size: usize) -> *mut c_void {
    if p.is_null() {
        return unsafe { ext4_user_malloc(size) };
    }
    let old = unsafe { ((p as *const u8).sub(HDR) as *const usize).read() };
    let new = unsafe { ext4_user_malloc(size) };
    if !new.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(p as *const u8, new as *mut u8, old.min(size));
            ext4_user_free(p);
        }
    }
    new
}

/// Called by the `assert()` shim in `c/include/assert.h`.
#[no_mangle]
pub unsafe extern "C" fn ext4_rs_assert_fail(file: *const c_char, line: c_int, expr: *const c_char) -> ! {
    use core::ffi::CStr;
    let s = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("?");
    panic!("lwext4 assertion failed: {} ({}:{})", s(expr), s(file), line);
}

// libc string functions. Only on bare metal: hosted builds (unit tests) must
// keep using the real libc.
#[cfg(target_os = "none")]
mod str_fns {
    use core::ffi::{c_char, c_int, c_void};

    #[no_mangle]
    pub unsafe extern "C" fn strlen(s: *const c_char) -> usize {
        let mut n = 0;
        while unsafe { *s.add(n) } != 0 {
            n += 1;
        }
        n
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcmp(a: *const c_char, b: *const c_char) -> c_int {
        let mut i = 0;
        loop {
            let (x, y) = unsafe { (*a.add(i) as u8, *b.add(i) as u8) };
            if x != y || x == 0 {
                return x as c_int - y as c_int;
            }
            i += 1;
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn strncmp(a: *const c_char, b: *const c_char, n: usize) -> c_int {
        for i in 0..n {
            let (x, y) = unsafe { (*a.add(i) as u8, *b.add(i) as u8) };
            if x != y || x == 0 {
                return x as c_int - y as c_int;
            }
        }
        0
    }

    #[no_mangle]
    pub unsafe extern "C" fn strcpy(d: *mut c_char, s: *const c_char) -> *mut c_char {
        let mut i = 0;
        loop {
            let c = unsafe { *s.add(i) };
            unsafe { *d.add(i) = c };
            if c == 0 {
                return d;
            }
            i += 1;
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn strncpy(d: *mut c_char, s: *const c_char, n: usize) -> *mut c_char {
        let mut i = 0;
        while i < n {
            let c = unsafe { *s.add(i) };
            unsafe { *d.add(i) = c };
            i += 1;
            if c == 0 {
                break;
            }
        }
        while i < n {
            unsafe { *d.add(i) = 0 };
            i += 1;
        }
        d
    }

    type Cmp = unsafe extern "C" fn(*const c_void, *const c_void) -> c_int;

    /// In-place heapsort: O(n log n) worst case, no allocation, no recursion.
    #[no_mangle]
    pub unsafe extern "C" fn qsort(base: *mut c_void, n: usize, size: usize, cmp: Cmp) {
        if n < 2 || size == 0 {
            return;
        }
        let b = base as *mut u8;
        let at = |i: usize| unsafe { b.add(i * size) };
        let swap = |i: usize, j: usize| unsafe {
            if i != j {
                core::ptr::swap_nonoverlapping(at(i), at(j), size);
            }
        };
        let sift = |mut root: usize, end: usize| loop {
            let mut child = 2 * root + 1;
            if child >= end {
                break;
            }
            if child + 1 < end && unsafe { cmp(at(child) as _, at(child + 1) as _) } < 0 {
                child += 1;
            }
            if unsafe { cmp(at(root) as _, at(child) as _) } >= 0 {
                break;
            }
            swap(root, child);
            root = child;
        };
        for start in (0..n / 2).rev() {
            sift(start, n);
        }
        for end in (1..n).rev() {
            swap(0, end);
            sift(0, end);
        }
    }
}
