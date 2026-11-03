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

.section .text
.macro USER_ENTER kind
    sub  sp, sp, #832
    stp  x0, x1, [sp, #0]
    stp  x2, x3, [sp, #16]
    stp  x4, x5, [sp, #32]
    stp  x6, x7, [sp, #48]
    stp  x8, x9, [sp, #64]
    stp  x10, x11, [sp, #80]
    stp  x12, x13, [sp, #96]
    stp  x14, x15, [sp, #112]
    stp  x16, x17, [sp, #128]
    stp  x18, x19, [sp, #144]
    stp  x20, x21, [sp, #160]
    stp  x22, x23, [sp, #176]
    stp  x24, x25, [sp, #192]
    stp  x26, x27, [sp, #208]
    stp  x28, x29, [sp, #224]
    str  x30, [sp, #240]
    mrs  x0, sp_el0
    str  x0, [sp, #248]
    mrs  x0, elr_el1
    mrs  x1, spsr_el1
    stp  x0, x1, [sp, #256]
    mrs  x0, esr_el1
    mrs  x1, far_el1
    stp  x0, x1, [sp, #272]
    mrs  x0, fpsr
    mrs  x1, fpcr
    stp  x0, x1, [sp, #288]
    stp  q0, q1, [sp, #320]
    stp  q2, q3, [sp, #352]
    stp  q4, q5, [sp, #384]
    stp  q6, q7, [sp, #416]
    stp  q8, q9, [sp, #448]
    stp  q10, q11, [sp, #480]
    stp  q12, q13, [sp, #512]
    stp  q14, q15, [sp, #544]
    stp  q16, q17, [sp, #576]
    stp  q18, q19, [sp, #608]
    stp  q20, q21, [sp, #640]
    stp  q22, q23, [sp, #672]
    stp  q24, q25, [sp, #704]
    stp  q26, q27, [sp, #736]
    stp  q28, q29, [sp, #768]
    stp  q30, q31, [sp, #800]
    mov  x0, #1
    msr  tpidrro_el0, x0
    mov  x0, sp
    mov  x1, #\kind
    bl   aarch64_user_trap
    b    aarch64_user_exit
.endm

.global aarch64_user_sync_entry
.balign 4
aarch64_user_sync_entry:
    USER_ENTER 0

.global aarch64_user_irq_entry
.balign 4
aarch64_user_irq_entry:
    USER_ENTER 1

.global aarch64_user_exit
.balign 4
aarch64_user_exit:
    msr  tpidrro_el0, xzr
    ldp  q0, q1, [sp, #320]
    ldp  q2, q3, [sp, #352]
    ldp  q4, q5, [sp, #384]
    ldp  q6, q7, [sp, #416]
    ldp  q8, q9, [sp, #448]
    ldp  q10, q11, [sp, #480]
    ldp  q12, q13, [sp, #512]
    ldp  q14, q15, [sp, #544]
    ldp  q16, q17, [sp, #576]
    ldp  q18, q19, [sp, #608]
    ldp  q20, q21, [sp, #640]
    ldp  q22, q23, [sp, #672]
    ldp  q24, q25, [sp, #704]
    ldp  q26, q27, [sp, #736]
    ldp  q28, q29, [sp, #768]
    ldp  q30, q31, [sp, #800]
    ldp  x0, x1, [sp, #288]
    msr  fpsr, x0
    msr  fpcr, x1
    ldr  x0, [sp, #248]
    msr  sp_el0, x0
    ldp  x0, x1, [sp, #256]
    msr  elr_el1, x0
    msr  spsr_el1, x1
    ldr  x30, [sp, #240]
    ldp  x0, x1, [sp, #0]
    ldp  x2, x3, [sp, #16]
    ldp  x4, x5, [sp, #32]
    ldp  x6, x7, [sp, #48]
    ldp  x8, x9, [sp, #64]
    ldp  x10, x11, [sp, #80]
    ldp  x12, x13, [sp, #96]
    ldp  x14, x15, [sp, #112]
    ldp  x16, x17, [sp, #128]
    ldp  x18, x19, [sp, #144]
    ldp  x20, x21, [sp, #160]
    ldp  x22, x23, [sp, #176]
    ldp  x24, x25, [sp, #192]
    ldp  x26, x27, [sp, #208]
    ldp  x28, x29, [sp, #224]
    add  sp, sp, #832
    eret

.global aarch64_enter_frame
.balign 4
aarch64_enter_frame:
    adrp x1, aarch64_kernel_sp
    ldr  x1, [x1, :lo12:aarch64_kernel_sp]
    sub  x1, x1, #832
    mov  x2, x1
    mov  x3, #832
1:  ldp  x4, x5, [x0], #16
    stp  x4, x5, [x2], #16
    subs x3, x3, #16
    b.ne 1b
    mov  sp, x1
    dsb  ish
    ic   ialluis
    dsb  ish
    isb
    b    aarch64_user_exit


.global aarch64_fpu_save
.balign 4
aarch64_fpu_save:
    stp  q0, q1, [x0, #0]
    stp  q2, q3, [x0, #32]
    stp  q4, q5, [x0, #64]
    stp  q6, q7, [x0, #96]
    stp  q8, q9, [x0, #128]
    stp  q10, q11, [x0, #160]
    stp  q12, q13, [x0, #192]
    stp  q14, q15, [x0, #224]
    stp  q16, q17, [x0, #256]
    stp  q18, q19, [x0, #288]
    stp  q20, q21, [x0, #320]
    stp  q22, q23, [x0, #352]
    stp  q24, q25, [x0, #384]
    stp  q26, q27, [x0, #416]
    stp  q28, q29, [x0, #448]
    stp  q30, q31, [x0, #480]
    mrs  x1, fpsr
    mrs  x2, fpcr
    add  x3, x0, #512
    stp  x1, x2, [x3]
    ret

.global aarch64_fpu_restore
.balign 4
aarch64_fpu_restore:
    ldp  q0, q1, [x0, #0]
    ldp  q2, q3, [x0, #32]
    ldp  q4, q5, [x0, #64]
    ldp  q6, q7, [x0, #96]
    ldp  q8, q9, [x0, #128]
    ldp  q10, q11, [x0, #160]
    ldp  q12, q13, [x0, #192]
    ldp  q14, q15, [x0, #224]
    ldp  q16, q17, [x0, #256]
    ldp  q18, q19, [x0, #288]
    ldp  q20, q21, [x0, #320]
    ldp  q22, q23, [x0, #352]
    ldp  q24, q25, [x0, #384]
    ldp  q26, q27, [x0, #416]
    ldp  q28, q29, [x0, #448]
    ldp  q30, q31, [x0, #480]
    add  x3, x0, #512
    ldp  x1, x2, [x3]
    msr  fpsr, x1
    msr  fpcr, x2
    ret
