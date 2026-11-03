use std::{env, fs, path::PathBuf};

fn main() {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let pie = env::var_os("IFSY_PIE").is_some();
    let mut b = cc::Build::new();

    let compiler = env::var("LWEXT4_CC").unwrap_or_else(|_| "clang".into());
    b.compiler(compiler);

    let (triple, flags): (&str, &[&str]) = match arch.as_str() {
        "x86_64" => (
            "x86_64-unknown-none-elf",
            &["-mcmodel=kernel", "-mno-red-zone", "-mno-sse", "-mno-mmx", "-msoft-float"],
        ),
        "aarch64" => ("aarch64-unknown-none-elf", &["-mgeneral-regs-only", "-mstrict-align"]),
        "riscv64" => (
            "riscv64-unknown-none-elf",
            &["-march=rv64gc", "-mabi=lp64d", "-mcmodel=medany"],
        ),
        other => panic!("lwext4_rust: unsupported target arch `{other}`"),
    };
    b.flag(&format!("--target={triple}"));
    for f in flags {
        // -mcmodel=kernel assumes a fixed address; a slid PIE kernel needs small.
        if pie && *f == "-mcmodel=kernel" {
            b.flag("-mcmodel=small");
        } else {
            b.flag(f);
        }
    }
    for f in [
        "-ffreestanding", "-nostdlibinc", "-fno-stack-protector",
        "-fno-asynchronous-unwind-tables", "-w",
    ] {
        b.flag(f);
    }
    if !pie {
        b.flag("-fno-pic").flag("-fno-pie");
    }
    b.pic(pie).opt_level(2).debug(false);
    b.include("c/include").include("c/lwext4/include");

    // Shared with scripts/gen-bindings.sh so struct layouts always agree.
    for line in fs::read_to_string("c/config.txt").unwrap().lines() {
        if let Some(def) = line.trim().strip_prefix("-D") {
            let (k, v) = def.split_once('=').unwrap_or((def, "1"));
            b.define(k, v);
        }
    }

    let mut srcs: Vec<PathBuf> = fs::read_dir("c/lwext4/src")
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map_or(false, |e| e == "c"))
        .collect();
    srcs.sort();
    b.files(&srcs);
    b.compile("lwext4");

    println!("cargo:rerun-if-env-changed=IFSY_PIE");
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=LWEXT4_CC");
}
