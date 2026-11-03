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

# x86_64 trap entry/exit. AT&T syntax; included by trap.rs through global_asm!
# (options(att_syntax)), so there is no C preprocessor: comments use '#'.
#
# Every way into the kernel -- CPU exceptions, the PIT tick, `int $64` and the
# `syscall` instruction -- ends in trap_common, which builds one TrapFrame on
# the kernel stack and calls trap_dispatch(frame). The layout, low to high:
#
#   r15 r14 r13 r12 r11 r10 r9 r8 rbp rdi rsi rdx rcx rbx rax   (pushed here)
#   vector error                                                (pushed by the stubs)
#   rip cs rflags rsp ss                                        (pushed by the CPU)
#
# = 22 qwords = 176 bytes. trap.rs checks the Rust struct against these numbers
# at compile time.
#
# Alignment: the CPU aligns RSP to 16 before pushing its frame; 22 qwords keep
# it aligned, so at `call` the SysV rule (RSP % 16 == 0) holds as-is.
#
# No SWAPGS: the kernel never uses GS/FS and this port runs one CPU, so the
# syscall stub finds the kernel stack in a plain variable (x86_kernel_rsp).

    .text

# ---- per-vector stubs -------------------------------------------------------
#
# STUB_NOERR pushes a dummy error code so every frame looks alike. Each stub
# also records (vector, address) in a table that idt.rs walks to fill the IDT.

    .pushsection .data.rel.ro.trap_vectors, "aw"
    .p2align 3
    .globl trap_vector_table
trap_vector_table:
    .popsection

    .macro STUB_NOERR vec
    .p2align 4
trap_stub_\vec:
    pushq   $0
    pushq   $\vec
    jmp     trap_common
    .pushsection .data.rel.ro.trap_vectors, "aw"
    .quad   \vec, trap_stub_\vec
    .popsection
    .endm

    .macro STUB_ERR vec
    .p2align 4
trap_stub_\vec:
    pushq   $\vec
    jmp     trap_common
    .pushsection .data.rel.ro.trap_vectors, "aw"
    .quad   \vec, trap_stub_\vec
    .popsection
    .endm

    STUB_NOERR 0        # #DE
    STUB_NOERR 1        # #DB
    STUB_NOERR 2        # NMI
    STUB_NOERR 3        # #BP
    STUB_NOERR 4        # #OF
    STUB_NOERR 5        # #BR
    STUB_NOERR 6        # #UD
    STUB_NOERR 7        # #NM
    STUB_ERR   8        # #DF
    STUB_ERR   10       # #TS
    STUB_ERR   11       # #NP
    STUB_ERR   12       # #SS
    STUB_ERR   13       # #GP
    STUB_ERR   14       # #PF
    STUB_NOERR 16       # #MF
    STUB_ERR   17       # #AC
    STUB_NOERR 18       # #MC
    STUB_NOERR 19       # #XM
    STUB_NOERR 20       # #VE
    STUB_ERR   21       # #CP
    STUB_NOERR 32       # PIT tick (PIC IRQ0)
    STUB_NOERR 39       # spurious IRQ7
    STUB_NOERR 47       # spurious IRQ15
    STUB_NOERR 64       # int $64 (Plan 9 VectorSYSCALL)

    .pushsection .data.rel.ro.trap_vectors, "aw"
    .globl trap_vector_table_end
trap_vector_table_end:
    .popsection

# ---- common entry -----------------------------------------------------------

    .p2align 4
trap_common:
    pushq   %rax
    pushq   %rbx
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %rbp
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11
    pushq   %r12
    pushq   %r13
    pushq   %r14
    pushq   %r15
    cld                         # user code may have set DF; interrupt gates do not clear it
    movq    %rsp, %rdi          # &TrapFrame
    call    trap_dispatch

# ---- common exit: restore a TrapFrame at %rsp and iretq ----------------------
# Also the way a thread enters user mode for the first time (x86_enter_frame).

trap_return:
    popq    %r15
    popq    %r14
    popq    %r13
    popq    %r12
    popq    %r11
    popq    %r10
    popq    %r9
    popq    %r8
    popq    %rbp
    popq    %rdi
    popq    %rsi
    popq    %rdx
    popq    %rcx
    popq    %rbx
    popq    %rax
    addq    $16, %rsp           # vector + error code
    iretq

# void x86_enter_frame(const TrapFrame *frame) -- never returns.
    .globl x86_enter_frame
    .p2align 4
x86_enter_frame:
    movq    %rdi, %rsp
    jmp     trap_return

# ---- the `syscall` instruction ------------------------------------------------
#
# On entry (see user.rs for STAR/LSTAR/SFMASK): RCX = user RIP, R11 = user
# RFLAGS, RSP is still the user's, interrupts are masked (SFMASK), CS/SS are
# the kernel's. Switch to the thread's kernel stack and fabricate the same
# frame an `int $64` would have produced, then join the common path. The
# return is always `iretq` (never `sysretq`): it restores any RIP/RSP, and a
# non-canonical RIP cannot fault in ring 0 the way it does for sysret on
# Intel CPUs.

    .globl syscall_entry
    .p2align 4
syscall_entry:
    movq    %rsp, syscall_user_rsp(%rip)
    movq    x86_kernel_rsp(%rip), %rsp
    pushq   $0x1b               # SS  = USER_DS (0x18 | 3)
    pushq   syscall_user_rsp(%rip)
    pushq   %r11                # RFLAGS
    pushq   $0x23               # CS  = USER_CS (0x20 | 3)
    pushq   %rcx                # RIP
    pushq   $0                  # error code
    pushq   $64                 # vector
    jmp     trap_common

# ---- data ---------------------------------------------------------------------

    .data
    .p2align 3
    .globl x86_kernel_rsp
x86_kernel_rsp:                 # top of the running thread's kernel stack
    .quad   0
syscall_user_rsp:               # scratch: user RSP between `syscall` and the push
    .quad   0
