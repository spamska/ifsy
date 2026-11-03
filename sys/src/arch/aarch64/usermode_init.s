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

.equ  HELLO_LEN, 18
.equ  DEV_LEN, 26
    .pushsection .rodata.ring3_init, "a"
    .p2align 4
    .globl ring3_init_start
ring3_init_start:
    mov   x19, x0
    mov   x9, sp
    and   x9, x9, #-16
    sub   x9, x9, #256
    mov   sp, x9

    cmp   x19, #2
    b.hs  .Lcrash

    mov   x8, #53
    svc   #0
    mov   x20, x0
    cmp   x0, #0
    b.le  .Lfail_nsec
    mov   x8, #53
    svc   #0
    cmp   x0, x20
    b.lt  .Lfail_nsec

    adr   x9, .Lpat_p
    ldr   d0, [x9]
    mov   x0, #20
    mov   x8, #17
    svc   #0
    adr   x9, .Lpat_p
    ldr   x10, [x9]
    fmov  x11, d0
    cmp   x10, x11
    b.ne  .Lfail_fp1

    adr   x9, .Lpat_p
    ldr   d0, [x9]
    mov   x0, #16
    mov   x8, #19
    svc   #0
    cmp   x0, #0
    b.lt  .Lfail_rfork
    b.eq  .Lchild

    mov   x0, #30
    mov   x8, #17
    svc   #0
    adr   x9, .Lpat_p
    ldr   x10, [x9]
    fmov  x11, d0
    cmp   x10, x11
    b.ne  .Lfail_fp2

    mov   x8, #53
    svc   #0
    mov   x20, x0
.Lspin:
    movz  x9, #0x93e0
    movk  x9, #0x4, lsl #16
1:  subs  x9, x9, #1
    b.ne  1b
    mov   x8, #53
    svc   #0
    sub   x0, x0, x20
    movz  x9, #0x9680
    movk  x9, #0x98, lsl #16
    cmp   x0, x9
    b.lo  .Lspin

    mov   x0, sp
    mov   x1, #192
    mov   x8, #47
    svc   #0
    cmp   x0, #8
    b.lt  .Lfail_await
    add   x10, sp, x0
    sub   x10, x10, #8
    adr   x11, .Lm_child
    mov   x12, #8
2:  ldrb  w13, [x10], #1
    ldrb  w14, [x11], #1
    cmp   w13, w14
    b.ne  .Lfail_child
    subs  x12, x12, #1
    b.ne  2b

    mov   x0, #1
    adr   x1, .Lm_hello
    mov   x2, #HELLO_LEN
    mov   x3, #-1
    mov   x8, #51
    svc   #0
    cmp   x0, #HELLO_LEN
    b.ne  .Lfail_io

    adr   x0, .Lm_path
    mov   x1, #1
    mov   x8, #14
    svc   #0
    cmp   x0, #3
    b.ne  .Lfail_io
    mov   x21, x0
    mov   x0, x21
    adr   x1, .Lm_dev
    mov   x2, #DEV_LEN
    mov   x8, #20
    svc   #0
    cmp   x0, #DEV_LEN
    b.ne  .Lfail_io
    mov   x0, x21
    mov   x8, #4
    svc   #0
    cbnz  x0, .Lfail_io
    mov   x0, x21
    adr   x1, .Lm_dev
    mov   x2, #1
    mov   x8, #20
    svc   #0
    cmn   x0, #1
    b.ne  .Lfail_io

    adr   x0, .Lm_ok
    b     .Lexit_msg

.Lchild:
    adr   x9, .Lpat_p
    ldr   x10, [x9]
    fmov  x11, d0
    cmp   x10, x11
    b.ne  .Lchild_bad
    adr   x9, .Lpat_q
    ldr   d0, [x9]
    movz  x12, #0x9680
    movk  x12, #0x98, lsl #16
3:  subs  x12, x12, #1
    b.ne  3b
    adr   x9, .Lpat_q
    ldr   x10, [x9]
    fmov  x11, d0
    cmp   x10, x11
    b.ne  .Lchild_bad
    adr   x0, .Lm_child
    b     .Lexit_msg
.Lchild_bad:
    adr   x0, .Lm_childfp
    b     .Lexit_msg

.Lcrash:
    mov   x0, #0
    ldr   x0, [x0]
    .word 0

.Lfail_nsec:
    adr   x0, .Lm_nsec
    b     .Lexit_msg
.Lfail_fp1:
    adr   x0, .Lm_fp1
    b     .Lexit_msg
.Lfail_fp2:
    adr   x0, .Lm_fp2
    b     .Lexit_msg
.Lfail_rfork:
    adr   x0, .Lm_rfork
    b     .Lexit_msg
.Lfail_await:
    adr   x0, .Lm_await
    b     .Lexit_msg
.Lfail_io:
    adr   x0, .Lm_io
    b     .Lexit_msg
.Lfail_child:
    adr   x0, .Lm_childres

.Lexit_msg:
    mov   x8, #8
    svc   #0
    .word 0

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
.Lm_fp1:      .asciz "fail: fp register lost across sleep"
.Lm_fp2:      .asciz "fail: fp register lost across rfork"
.Lm_rfork:    .asciz "fail: rfork"
.Lm_await:    .asciz "fail: await"
.Lm_childres: .asciz "fail: child result"
.Lm_childfp:  .asciz "child: fp register state wrong"
    .p2align 2
    .globl ring3_init_end
ring3_init_end:
    .popsection
