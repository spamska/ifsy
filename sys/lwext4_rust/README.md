# lwext4_rust (vendored, reworked for Ifsy)

Rust bindings and glue for [lwext4](https://github.com/gkostka/lwext4) (commit in `c/lwext4/VERSION`).

Differences from upstream `lwext4_rust`:
- C is built by `build.rs` with `cc` + `clang` (freestanding, per-arch flags for x86_64/aarch64/riscv64); no musl cross gcc, cmake, git submodule or patch.
- `c/include/` holds minimal libc header shims; `src/ulibc.rs` implements the libc pieces lwext4 needs (allocator hooks, `str*`, `qsort`, assert).
- `src/bindings.rs` is pre-generated; regenerate with `scripts/gen-bindings.sh` (needs `bindgen`). `c/config.txt` holds the lwext4 config and is shared by both.
- `Ext4BlockWrapper::new` returns `Err(errno)` instead of panicking, frees everything on drop, adds `sync()` and `into_device()` (unmount and take the device back).
- lwext4 debug printf is compiled out; errors go through the `log` crate.
