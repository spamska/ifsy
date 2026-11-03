use std::env;
use std::path::Path;

/// Target flags shared by the C and assembly halves.
fn base(os: &str, arch: &str, b: &mut cc::Build) {
    let pie = env::var_os("IFSY_PIE").is_some();
    if os == "none" {
        let compiler = env::var("LWEXT4_CC").unwrap_or_else(|_| "clang".into());
        b.compiler(compiler);

        let triple = match arch {
            "x86_64" => "x86_64-unknown-none-elf",
            "aarch64" => "aarch64-unknown-none-elf",
            "riscv64" => "riscv64-unknown-none-elf",
            other => panic!("sched_4bsd: unsupported target arch `{other}`"),
        };
        b.flag(&format!("--target={triple}"));
        for f in [
            "-ffreestanding", "-nostdlibinc", "-fno-stack-protector",
            "-fno-asynchronous-unwind-tables",
        ] {
            b.flag(f);
        }
        if !pie {
            b.flag("-fno-pic").flag("-fno-pie");
        }
        b.pic(pie).debug(false);
    }
    b.include("c");
    b.include(Path::new("arch").join(arch));
}

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let pie = env::var_os("IFSY_PIE").is_some();
    let arch_dir = Path::new("arch").join(&arch);

    // The C half: the scheduler proper and the machine-dependent helpers.
    // Flags here keep the compiler away from state the kernel does not save
    // (SSE on x86_64, FP/SIMD on aarch64).
    let mut c = cc::Build::new();
    base(&os, &arch, &mut c);
    if os == "none" {
        let flags: &[&str] = match arch.as_str() {
            "x86_64" => &["-mcmodel=kernel", "-mno-red-zone", "-mno-sse", "-mno-mmx", "-msoft-float"],
            "aarch64" => &["-mgeneral-regs-only", "-mstrict-align"],
            "riscv64" => &["-march=rv64gc", "-mabi=lp64d", "-mcmodel=medany"],
            _ => unreachable!(),
        };
        for f in flags {
            // -mcmodel=kernel assumes a fixed address; a slid PIE kernel needs small.
            if pie && *f == "-mcmodel=kernel" {
                c.flag("-mcmodel=small");
            } else {
                c.flag(f);
            }
        }
    }
    c.flag("-Wall").flag("-Wextra");
    c.file("c/4bsd.c");
    c.file(arch_dir.join("machdep.c"));
    c.opt_level(2);
    c.compile("4bsd_c");

    // The assembly half is built separately: it must be allowed to touch the
    // registers the C flags above forbid (cpu_switch saves d8-d15 on aarch64
    // and fs0-fs11 on riscv64, which -mgeneral-regs-only would make the
    // assembler reject).
    let mut s = cc::Build::new();
    base(&os, &arch, &mut s);
    if os == "none" && arch == "riscv64" {
        s.flag("-march=rv64gc").flag("-mabi=lp64d");
    }
    s.file(arch_dir.join("switch.S"));
    s.compile("4bsd_asm");

    println!("cargo:rerun-if-env-changed=IFSY_PIE");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=arch");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LWEXT4_CC");
}
