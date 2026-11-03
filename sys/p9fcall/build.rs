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

use std::env;

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let pie = env::var_os("IFSY_PIE").is_some();
    let mut c = cc::Build::new();
    if os == "none" {
        c.compiler(env::var("LWEXT4_CC").unwrap_or_else(|_| "clang".into()));
        let triple = match arch.as_str() {
            "x86_64" => "x86_64-unknown-none-elf",
            "aarch64" => "aarch64-unknown-none-elf",
            "riscv64" => "riscv64-unknown-none-elf",
            other => panic!("p9fcall: unsupported target arch `{other}`"),
        };
        c.flag(&format!("--target={triple}"));
        for f in [
            "-ffreestanding", "-nostdlibinc", "-fno-stack-protector",
            "-fno-asynchronous-unwind-tables",
        ] {
            c.flag(f);
        }
        if !pie {
            c.flag("-fno-pic").flag("-fno-pie");
        }
        let arch_flags: &[&str] = match arch.as_str() {
            "x86_64" => &["-mcmodel=kernel", "-mno-red-zone", "-mno-sse", "-mno-mmx", "-msoft-float"],
            "aarch64" => &["-mgeneral-regs-only", "-mstrict-align"],
            "riscv64" => &["-march=rv64gc", "-mabi=lp64d", "-mcmodel=medany"],
            _ => unreachable!(),
        };
        for f in arch_flags {
            // -mcmodel=kernel assumes a fixed address; a slid PIE kernel needs small.
            if pie && *f == "-mcmodel=kernel" {
                c.flag("-mcmodel=small");
            } else {
                c.flag(f);
            }
        }
        c.pic(pie).debug(false);
    }
    c.include("c/include");
    c.flag("-Wno-unknown-pragmas").flag("-Wno-sign-compare");
    for f in ["convM2S", "convS2M", "convM2D", "convD2M", "shim"] {
        c.file(format!("c/{f}.c"));
    }
    c.opt_level(2);
    c.compile("p9fcall_c");
    println!("cargo:rerun-if-env-changed=IFSY_PIE");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LWEXT4_CC");
}
