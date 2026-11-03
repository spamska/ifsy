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

# The ring-3 boot test program (x86_64). AT&T syntax; included by usermode.rs
# through global_asm!(options(att_syntax)), copied into a tiny ELF and run as
# /init by the boot self-test.
#
# It is position independent (everything is RIP-relative, no relocations): the
# kernel loads it at a randomised base. It makes its system calls the Plan 9
# amd64 way -- number in %rbp, arguments on the stack starting at 8(%rsp) --
# and uses both entry instructions (`syscall` and `int $64`).
#
# argc == 1 ("init")        the normal run, described below;
# argc >= 2 ("init crash")  read address 0: the kernel must turn the page fault
#                           into the note "sys: trap: fault read addr=0x0 ..."
#                           and end the process with that text.
#
# Normal run, each step ends the process with a "fail: ..." message if wrong:
#   1. the code really runs at CPL 3 (the low bits of %cs);
#   2. nsec() through `syscall` and through `int $64`: positive, monotonic;
#   3. an SSE register survives a sleep(), i.e. a switch to another thread
#      (the idle context here) and back;
#   4. rfork(RFPROC): the child starts with the parent's SSE registers and
#      returns 0; it clobbers xmm0 in its own context, the parent's must stay
#      intact across the switches that follow;
#   5. the parent burns 40 ms in ring 3 (timer interrupts land in user code),
#      then await()s the child and checks its exit message is "child ok";
#   6. console I/O: pwrite to fd 1, open("/dev/cons"), _write, close, and a
#      write to the closed descriptor fails;
#   7. exits("ring3 ok").
#
# Syscall numbers (sys/src/syscall.rs): CLOSE 4, EXITS 8, OPEN 14, SLEEP 17,
# RFORK 19, _WRITE 20, AWAIT 47, PWRITE 51, NSEC 53. RFPROC = 1 << 4.

    .pushsection .rodata.ring3_init, "a"
    .p2align 4
    .globl ring3_init_start
ring3_init_start:
    movq    %rdi, %r12                  # argc
    andq    $-16, %rsp
    subq    $256, %rsp                  # 8(%rsp)..: syscall arguments; 64(%rsp)..: await() record

    movl    %cs, %eax                   # 1. CPL must be 3
    andl    $3, %eax
    cmpl    $3, %eax
    leaq    .Lm_cpl(%rip), %rax
    jne     .Lexit_msg

    cmpq    $2, %r12
    jae     .Lcrash

    movq    $53, %rbp                   # 2. nsec() via syscall
    syscall
    movq    %rax, %r13
    testq   %rax, %rax
    jle     .Lfail_nsec
    movq    $53, %rbp                   #    nsec() via int $64
    int     $64
    cmpq    %r13, %rax
    jl      .Lfail_nsec

    movdqu  .Lpat_p(%rip), %xmm0        # 3. xmm0 across sleep(20)
    movq    $20, 8(%rsp)
    movq    $17, %rbp
    syscall
    movdqu  .Lpat_p(%rip), %xmm1
    pcmpeqb %xmm0, %xmm1
    pmovmskb %xmm1, %eax
    cmpl    $0xffff, %eax
    jne     .Lfail_sse1

    movdqu  .Lpat_p(%rip), %xmm0        # 4. rfork(RFPROC)
    movq    $16, 8(%rsp)
    movq    $19, %rbp
    syscall
    testq   %rax, %rax
    js      .Lfail_rfork
    jz      .Lchild

    movq    $30, 8(%rsp)                #    parent: sleep while the child runs
    movq    $17, %rbp
    syscall
    movdqu  .Lpat_p(%rip), %xmm1
    pcmpeqb %xmm0, %xmm1
    pmovmskb %xmm1, %eax
    cmpl    $0xffff, %eax
    jne     .Lfail_sse2

    movq    $53, %rbp                   # 5. spin 40 ms in ring 3
    syscall
    movq    %rax, %r13
.Lspin:
    movl    $300000, %ecx               #    pure user-mode work between clock reads
1:  decl    %ecx
    jnz     1b
    movq    $53, %rbp
    syscall
    subq    %r13, %rax
    cmpq    $40000000, %rax
    jb      .Lspin

    leaq    64(%rsp), %rax              #    await(buf, 192)
    movq    %rax, 8(%rsp)
    movq    $192, 16(%rsp)
    movq    $47, %rbp
    syscall
    cmpq    $8, %rax                    #    -1 (no child) or too short for "child ok"
    jl      .Lfail_await
    movq    56(%rsp,%rax), %rdx         #    the last 8 bytes of "pid utime stime rtime msg"
    movabsq $0x6b6f20646c696863, %rcx   #    "child ok"
    cmpq    %rcx, %rdx
    jne     .Lfail_child

    leaq    .Lm_hello(%rip), %rax       # 6. console output: pwrite(1, hello, n, -1)
    movq    $1, 8(%rsp)
    movq    %rax, 16(%rsp)
    movq    $.Lm_hello_end-.Lm_hello, 24(%rsp)
    movq    $-1, 32(%rsp)
    movq    $51, %rbp
    syscall
    cmpq    $.Lm_hello_end-.Lm_hello, %rax
    jne     .Lfail_io

    leaq    .Lm_path(%rip), %rax        #    open("/dev/cons"): first free fd is 3
    movq    %rax, 8(%rsp)
    movq    $1, 16(%rsp)
    movq    $14, %rbp
    syscall
    cmpq    $3, %rax
    jne     .Lfail_io
    movq    %rax, %r14
    leaq    .Lm_dev(%rip), %rax         #    _write(fd, msg, n)
    movq    %r14, 8(%rsp)
    movq    %rax, 16(%rsp)
    movq    $.Lm_dev_end-.Lm_dev, 24(%rsp)
    movq    $20, %rbp
    syscall
    cmpq    $.Lm_dev_end-.Lm_dev, %rax
    jne     .Lfail_io
    movq    %r14, 8(%rsp)               #    close(fd)
    movq    $4, %rbp
    syscall
    testq   %rax, %rax
    jnz     .Lfail_io
    leaq    .Lm_dev(%rip), %rax         #    a write to the closed fd must fail
    movq    %r14, 8(%rsp)
    movq    %rax, 16(%rsp)
    movq    $1, 24(%rsp)
    movq    $20, %rbp
    syscall
    cmpq    $-1, %rax
    jne     .Lfail_io

    leaq    .Lm_ok(%rip), %rax          # 7. done
    jmp     .Lexit_msg

.Lchild:
    movdqu  .Lpat_p(%rip), %xmm1        #    inherited xmm0 == parent's?
    pcmpeqb %xmm0, %xmm1
    pmovmskb %xmm1, %eax
    cmpl    $0xffff, %eax
    jne     .Lchild_bad
    movdqu  .Lpat_q(%rip), %xmm0        #    now make it ours
    movl    $10000000, %ecx
1:  decl    %ecx
    jnz     1b
    movdqu  .Lpat_q(%rip), %xmm1        #    still ours after whatever switched us out?
    pcmpeqb %xmm0, %xmm1
    pmovmskb %xmm1, %eax
    cmpl    $0xffff, %eax
    jne     .Lchild_bad
    leaq    .Lm_child(%rip), %rax
    jmp     .Lexit_msg
.Lchild_bad:
    leaq    .Lm_childsse(%rip), %rax
    jmp     .Lexit_msg

.Lcrash:
    xorl    %eax, %eax
    movq    (%rax), %rax                # page fault at address 0
    ud2

.Lfail_nsec:
    leaq    .Lm_nsec(%rip), %rax
    jmp     .Lexit_msg
.Lfail_sse1:
    leaq    .Lm_sse1(%rip), %rax
    jmp     .Lexit_msg
.Lfail_sse2:
    leaq    .Lm_sse2(%rip), %rax
    jmp     .Lexit_msg
.Lfail_rfork:
    leaq    .Lm_rfork(%rip), %rax
    jmp     .Lexit_msg
.Lfail_await:
    leaq    .Lm_await(%rip), %rax
    jmp     .Lexit_msg
.Lfail_io:
    leaq    .Lm_io(%rip), %rax
    jmp     .Lexit_msg
.Lfail_child:
    leaq    .Lm_childres(%rip), %rax

.Lexit_msg:                             # exits(%rax)
    movq    %rax, 8(%rsp)
    movq    $8, %rbp
    syscall
    ud2                                 # exits() does not return

    .p2align 4
.Lpat_p:
    .byte   0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xf0, 0x0f
.Lpat_q:
    .byte   0xa5, 0x5a, 0xa5, 0x5a, 0xa5, 0x5a, 0xa5, 0x5a, 0xc3, 0x3c, 0xc3, 0x3c, 0xc3, 0x3c, 0xc3, 0x3c
.Lm_hello:    .ascii "hello from ring 3\n"
.Lm_hello_end:
.Lm_dev:      .ascii "written through /dev/cons\n"
.Lm_dev_end:
.Lm_path:     .asciz "/dev/cons"
.Lm_io:       .asciz "fail: console i/o"
.Lm_ok:       .asciz "ring3 ok"
.Lm_child:    .asciz "child ok"
.Lm_cpl:      .asciz "fail: not running at CPL 3"
.Lm_nsec:     .asciz "fail: nsec"
.Lm_sse1:     .asciz "fail: SSE lost across sleep"
.Lm_sse2:     .asciz "fail: SSE lost across rfork"
.Lm_rfork:    .asciz "fail: rfork"
.Lm_await:    .asciz "fail: await"
.Lm_childres: .asciz "fail: child result"
.Lm_childsse: .asciz "child: SSE state wrong"
    .globl ring3_init_end
ring3_init_end:
    .popsection
