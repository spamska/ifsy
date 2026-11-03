//! lwext4 (ext2/3/4 filesystem library in C) for freestanding Rust kernels.
#![no_std]
#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]

extern crate alloc;

#[macro_use]
extern crate log;

mod ulibc;

pub mod bindings;
pub mod blockdev;
pub mod file;

pub use blockdev::*;
pub use file::{Ext4File, InodeTypes};
