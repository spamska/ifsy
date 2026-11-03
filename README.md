<p align=>
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="https://d.uguu.se/WLVUcHuZ.png">
    <img src="https://d.uguu.se/lMKmPTvB.png" alt="logo" width="75%""/>
  </picture>
</p>

[![BSD 2-Clause license](https://img.shields.io/badge/license-BSD--2--Clause-blue.svg?style=plastic&logo=book)](./LICENSE)
[![Build](https://github.com/spamska/ifsy/actions/workflows/makefile.yml/badge.svg)](https://github.com/spamska/ifsy/actions/workflows/makefile.yml)
![Toolchain](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fspamska%2Fifsy%2Fmain%2Frust-toolchain.toml&query=%24.toolchain.channel&style=plastic&color=orange&label=toolchain)
![Version](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fspamska%2Fifsy%2Fmain%2Fsys%2FCargo.toml&query=%24.package.version&style=plastic&label=version)
![Stars](https://img.shields.io/github/stars/spamska/ifsy?style=plastic&color=yellow)
[![Discord](https://img.shields.io/badge/discord-join_us-%235865F2?style=plastic&logo=discord&logoColor=white)](https://discord.gg/HF65J3DDKu)

# About

> &#9888;&#65039;&#9888;&#65039; IFSY IS STILL IN EARLY DEVELOPMENT. USE WITH CAUTION. &#9888;&#65039;&#9888;&#65039;

Ifsy is a small Unix-like kernel written in Rust that brings Plan 9's elegant design philosophy to modern hardware. 

It's aim is to provide a portable and modular operating system that leverages modern computing principles such as type-safety and security hardening while maintaining the simplicity and streamline of Plan 9's system call interface.

Along with Plan 9's features, Ifsy also implements Unix and BSD lineage code into it's compilation process. This allows the kernel to be held up by a mature and developed backbone.


## Supported Architectures

| Architecture | Supported? |
|-|-|
| x86_64 | Yes |
| AArch64 | Yes |
| RISC-V 64 | Yes |

## Features

- **Boot & Security**
  - Limine bootloader
  - W^X memory protection
  - Kernel Address Space Layout Randomization (KASLR)
  - Static ELF programs with randomized addresses

- **Plan 9 System Interface**
  - Plan 9 system call interface
  - Namespaces and process management
  - 9P protocol (9P2000 mounts)
  - Bind, mount, and union directories

- **Storage & Filesystems**
  - ext4 filesystem (via lwext4 library)
  - Disk partition support

- **Memory Management**
  - Virtual memory
  - Page cache

- **Process Management**
  - Preemptive scheduler (forked from FreeBSD 4BSD scheduler)
  - CPU topology enumeration
  - Symmetric MultiProcessing (SMP)

- **Networking**
  - TCP/IP stack (via smoltcp)
  - Network drivers:
    - virtio-net
    - e1000
    - RTL8139

- **User Environment**
  - rc shell (Plan 9 shell)
  - 21 core utilities
  - Console shell boot


## Documentation

For detailed instructions on building and running Ifsy, see the [Makefile](GNUmakefile) for correct usage of Make commands.

Ifsy requires:
- Rust/Cargo
- Make tool (make/gmake/cmake)
- C Compiler (e.g. GCC, CC, etc.)
- Assembler (e.g. NASM, AS, etc.)

## Building

Clone the repository and follow the build instructions in the kernel root Makefile's top-level comment.

```sh
git clone https://github.com/spamska/ifsy
cd ifsy
make <option, e.g. N/A, run, all-hdd>
