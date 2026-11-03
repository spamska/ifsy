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

# The U-mode boot test program (riscv64). GNU syntax; included by usermode.rs
# through global_asm!, copied into a tiny ELF and run as /init by the boot
# self-test. It mirrors the x86_64 program (arch/x86_64/usermode_init.s).
#
# It is position independent (everything is pc-relative via `lla`, no
# relocations): the kernel loads it at a randomised base. System calls are the
# riscv64 way: number in a7, arguments in a0-a5, result in a0.
#
# argc == 1 ("init")        the normal run, described below;
# argc >= 2 ("init crash")  load from address 0: the kernel must turn the page
#                           fault into the note "sys: trap: fault read addr=0x0 ..."
#                           and end the process with that text.
#
# Normal run, each step ends the process with a "fail: ..." message if wrong:
#   1. nsec() is positive and monotonic;
#   2. an f register survives a sleep(), i.e. a switch to another thread and back;
#   3. rfork(RFPROC): the child starts with the parent's f registers and returns
#      0; it clobbers f0 in its own context, the parent's must stay intact;
#   4. the parent burns 40 ms in U-mode (timer interrupts land in user code),
#      then await()s the child and checks its exit message ends in "child ok";
#   5. console I/O: pwrite to fd 1, open("/dev/cons"), _write, close, and a
#      write to the closed descriptor fails;
#   6. exits("ring3 ok").
#
# Syscall numbers (sys/src/syscall.rs): CLOSE 4, EXITS 8, OPEN 14, SLEEP 17,
# RFORK 19, _WRITE 20, AWAIT 47, PWRITE 51, NSEC 53. RFPROC = 1 << 4.

.equ  HELLO_LEN, 18
.equ  DEV_LEN, 26
    .pushsection .rodata.ring3_init, "a"
    .option push
    .option norelax
    .option arch, +d
    .p2align 4
    .globl ring3_init_start
ring3_init_start:
    mv    s0, a0                    # argc
    andi  sp, sp, -16
    addi  sp, sp, -256              # 0(sp).. the await() record

    li    t0, 2
    bgeu  s0, t0, .Lcrash

    li    a7, 53                    # 1. nsec()
    ecall
    mv    s1, a0
    blez  a0, .Lfail_nsec
    li    a7, 53
    ecall
    blt   a0, s1, .Lfail_nsec

    lla   t0, .Lpat_p               # 2. f0 across sleep(20)
    fld   f0, 0(t0)
    li    a0, 20
    li    a7, 17
    ecall
    lla   t0, .Lpat_p
    ld    t1, 0(t0)
    fmv.x.d t2, f0
    bne   t1, t2, .Lfail_fp1

    lla   t0, .Lpat_p               # 3. rfork(RFPROC)
    fld   f0, 0(t0)
    li    a0, 16
    li    a7, 19
    ecall
    bltz  a0, .Lfail_rfork
    beqz  a0, .Lchild

    li    a0, 30                    #    parent: sleep while the child runs
    li    a7, 17
    ecall
    lla   t0, .Lpat_p
    ld    t1, 0(t0)
    fmv.x.d t2, f0
    bne   t1, t2, .Lfail_fp2

    li    a7, 53                    # 4. spin 40 ms in U-mode
    ecall
    mv    s1, a0
.Lspin:
    li    t0, 300000                #    pure user-mode work between clock reads
1:  addi  t0, t0, -1
    bnez  t0, 1b
    li    a7, 53
    ecall
    sub   a0, a0, s1
    li    t0, 40000000
    bltu  a0, t0, .Lspin

    mv    a0, sp                    #    await(buf, 192)
    li    a1, 192
    li    a7, 47
    ecall
    li    t0, 8
    blt   a0, t0, .Lfail_await      #    -1 (no child) or too short for "child ok"
    add   t1, sp, a0                #    the last 8 bytes of "pid utime stime rtime msg"
    addi  t1, t1, -8
    lla   t2, .Lm_child
    li    t3, 8
2:  lbu   t4, 0(t1)
    lbu   t5, 0(t2)
    bne   t4, t5, .Lfail_child
    addi  t1, t1, 1
    addi  t2, t2, 1
    addi  t3, t3, -1
    bnez  t3, 2b

    li    a0, 1                     # 5. console output: pwrite(1, hello, n, -1)
    lla   a1, .Lm_hello
    li    a2, HELLO_LEN
    li    a3, -1
    li    a7, 51
    ecall
    li    t0, HELLO_LEN
    bne   a0, t0, .Lfail_io

    lla   a0, .Lm_path              #    open("/dev/cons"): first free fd is 3
    li    a1, 1
    li    a7, 14
    ecall
    li    t0, 3
    bne   a0, t0, .Lfail_io
    mv    s2, a0
    mv    a0, s2                    #    _write(fd, msg, n)
    lla   a1, .Lm_dev
    li    a2, DEV_LEN
    li    a7, 20
    ecall
    li    t0, DEV_LEN
    bne   a0, t0, .Lfail_io
    mv    a0, s2                    #    close(fd)
    li    a7, 4
    ecall
    bnez  a0, .Lfail_io
    mv    a0, s2                    #    a write to the closed fd must fail
    lla   a1, .Lm_dev
    li    a2, 1
    li    a7, 20
    ecall
    li    t0, -1
    bne   a0, t0, .Lfail_io

    lla   a0, .Lm_ok                # 6. done
    j     .Lexit_msg

.Lchild:
    lla   t0, .Lpat_p               #    inherited f0 == parent's?
    ld    t1, 0(t0)
    fmv.x.d t2, f0
    bne   t1, t2, .Lchild_bad
    lla   t0, .Lpat_q               #    now make it ours
    fld   f0, 0(t0)
    li    t3, 10000000
3:  addi  t3, t3, -1
    bnez  t3, 3b
    lla   t0, .Lpat_q               #    still ours after whatever switched us out?
    ld    t1, 0(t0)
    fmv.x.d t2, f0
    bne   t1, t2, .Lchild_bad
    lla   a0, .Lm_child
    j     .Lexit_msg
.Lchild_bad:
    lla   a0, .Lm_childfp
    j     .Lexit_msg

.Lcrash:
    li    a0, 0
    ld    a0, 0(a0)                 # page fault at address 0
    .word 0

.Lfail_nsec:
    lla   a0, .Lm_nsec
    j     .Lexit_msg
.Lfail_fp1:
    lla   a0, .Lm_fp1
    j     .Lexit_msg
.Lfail_fp2:
    lla   a0, .Lm_fp2
    j     .Lexit_msg
.Lfail_rfork:
    lla   a0, .Lm_rfork
    j     .Lexit_msg
.Lfail_await:
    lla   a0, .Lm_await
    j     .Lexit_msg
.Lfail_io:
    lla   a0, .Lm_io
    j     .Lexit_msg
.Lfail_child:
    lla   a0, .Lm_childres

.Lexit_msg:                         # exits(a0)
    li    a7, 8
    ecall
    .word 0                         # exits() does not return

    .p2align 3
.Lpat_p:
    .quad 0x1122334455667788
.Lpat_q:
    .quad 0xc33cc33cc33cc33c
.Lm_hello:    .ascii "hello from ring 3\n"
.Lm_dev:      .ascii "written through /dev/cons\n"
.Lm_path:     .asciz "/dev/cons"
.Lm_io:       .asciz "fail: console i/o"
.Lm_ok:       .asciz "ring3 ok"
.Lm_child:    .asciz "child ok"
.Lm_nsec:     .asciz "fail: nsec"
.Lm_fp1:      .asciz "fail: f register lost across sleep"
.Lm_fp2:      .asciz "fail: f register lost across rfork"
.Lm_rfork:    .asciz "fail: rfork"
.Lm_await:    .asciz "fail: await"
.Lm_childres: .asciz "fail: child result"
.Lm_childfp:  .asciz "child: f register state wrong"
    .p2align 2
    .globl ring3_init_end
ring3_init_end:
    .option pop
    .popsection
