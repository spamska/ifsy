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

# RISC-V trap entry and the user-frame helpers. Included by traps.rs through
# global_asm!; see the module documentation there for the design.
#
# sscratch convention: it is 0 whenever the hart runs in S-mode and holds the
# current thread's kernel-stack top whenever it runs in U-mode. So the first
# instruction, a swap of sp and sscratch, tells the two apart: afterwards sp is
# non-zero if the trap came from U-mode.
#
# User trap frame (560 bytes, 16-byte aligned, at the top of the kernel stack):
#    0.. 255  x0..x31 (x0 unused; x2 = the user's sp)
#  256 sepc  264 sstatus  272 scause  280 stval  288 fcsr  296 (pad)
#  304.. 559  f0..f31

.option arch, +d
.section .text
.global riscv_trap_entry
.balign 4
riscv_trap_entry:
    csrrw sp, sscratch, sp
    bnez  sp, 20f
    csrrw sp, sscratch, sp      # S-mode trap: undo the swap, sp is the kernel's again
    j     .Lkernel_trap

20: # trap from U-mode: sp = kernel stack top, sscratch = the user's sp
    addi sp, sp, -560
    sd   ra,   8(sp)
    sd   gp,  24(sp)
    sd   tp,  32(sp)
    sd   t0,  40(sp)
    sd   t1,  48(sp)
    sd   t2,  56(sp)
    sd   s0,  64(sp)
    sd   s1,  72(sp)
    sd   a0,  80(sp)
    sd   a1,  88(sp)
    sd   a2,  96(sp)
    sd   a3, 104(sp)
    sd   a4, 112(sp)
    sd   a5, 120(sp)
    sd   a6, 128(sp)
    sd   a7, 136(sp)
    sd   s2, 144(sp)
    sd   s3, 152(sp)
    sd   s4, 160(sp)
    sd   s5, 168(sp)
    sd   s6, 176(sp)
    sd   s7, 184(sp)
    sd   s8, 192(sp)
    sd   s9, 200(sp)
    sd   s10, 208(sp)
    sd   s11, 216(sp)
    sd   t3, 224(sp)
    sd   t4, 232(sp)
    sd   t5, 240(sp)
    sd   t6, 248(sp)
    csrr t0, sscratch
    sd   t0,  16(sp)            # the user's sp
    csrw sscratch, zero         # now in S-mode
    csrr t0, sepc
    sd   t0, 256(sp)
    csrr t0, sstatus
    sd   t0, 264(sp)
    csrr t1, scause
    sd   t1, 272(sp)
    csrr t1, stval
    sd   t1, 280(sp)
    li   t1, 0x6000             # sstatus.FS
    and  t1, t1, t0
    beqz t1, 21f
    fsd  f0 , 304(sp)
    fsd  f1 , 312(sp)
    fsd  f2 , 320(sp)
    fsd  f3 , 328(sp)
    fsd  f4 , 336(sp)
    fsd  f5 , 344(sp)
    fsd  f6 , 352(sp)
    fsd  f7 , 360(sp)
    fsd  f8 , 368(sp)
    fsd  f9 , 376(sp)
    fsd  f10, 384(sp)
    fsd  f11, 392(sp)
    fsd  f12, 400(sp)
    fsd  f13, 408(sp)
    fsd  f14, 416(sp)
    fsd  f15, 424(sp)
    fsd  f16, 432(sp)
    fsd  f17, 440(sp)
    fsd  f18, 448(sp)
    fsd  f19, 456(sp)
    fsd  f20, 464(sp)
    fsd  f21, 472(sp)
    fsd  f22, 480(sp)
    fsd  f23, 488(sp)
    fsd  f24, 496(sp)
    fsd  f25, 504(sp)
    fsd  f26, 512(sp)
    fsd  f27, 520(sp)
    fsd  f28, 528(sp)
    fsd  f29, 536(sp)
    fsd  f30, 544(sp)
    fsd  f31, 552(sp)
    frcsr t1
    sd   t1, 288(sp)
21:
    mv   a0, sp
    call riscv_user_trap
    # fall through: return to user mode with the (possibly rewritten) frame at sp

.global riscv_user_exit
riscv_user_exit:
    csrr t0, sstatus
    li   t1, 0x6000
    and  t1, t1, t0
    beqz t1, 22f
    fld  f0 , 304(sp)
    fld  f1 , 312(sp)
    fld  f2 , 320(sp)
    fld  f3 , 328(sp)
    fld  f4 , 336(sp)
    fld  f5 , 344(sp)
    fld  f6 , 352(sp)
    fld  f7 , 360(sp)
    fld  f8 , 368(sp)
    fld  f9 , 376(sp)
    fld  f10, 384(sp)
    fld  f11, 392(sp)
    fld  f12, 400(sp)
    fld  f13, 408(sp)
    fld  f14, 416(sp)
    fld  f15, 424(sp)
    fld  f16, 432(sp)
    fld  f17, 440(sp)
    fld  f18, 448(sp)
    fld  f19, 456(sp)
    fld  f20, 464(sp)
    fld  f21, 472(sp)
    fld  f22, 480(sp)
    fld  f23, 488(sp)
    fld  f24, 496(sp)
    fld  f25, 504(sp)
    fld  f26, 512(sp)
    fld  f27, 520(sp)
    fld  f28, 528(sp)
    fld  f29, 536(sp)
    fld  f30, 544(sp)
    fld  f31, 552(sp)
    ld   t1, 288(sp)
    fscsr t1
22:
    li   t0, 0x100              # sstatus.SPP = 0: sret goes to U-mode
    csrc sstatus, t0
    li   t0, 0x20               # sstatus.SPIE = 1: interrupts on in user mode
    csrs sstatus, t0
    ld   t0, 256(sp)
    csrw sepc, t0
    lla  t0, riscv_kernel_sp
    ld   t0, 0(t0)
    csrw sscratch, t0           # the kernel stack for the next trap from U-mode
    ld   ra,   8(sp)
    ld   gp,  24(sp)
    ld   tp,  32(sp)
    ld   t1,  48(sp)
    ld   t2,  56(sp)
    ld   s0,  64(sp)
    ld   s1,  72(sp)
    ld   a0,  80(sp)
    ld   a1,  88(sp)
    ld   a2,  96(sp)
    ld   a3, 104(sp)
    ld   a4, 112(sp)
    ld   a5, 120(sp)
    ld   a6, 128(sp)
    ld   a7, 136(sp)
    ld   s2, 144(sp)
    ld   s3, 152(sp)
    ld   s4, 160(sp)
    ld   s5, 168(sp)
    ld   s6, 176(sp)
    ld   s7, 184(sp)
    ld   s8, 192(sp)
    ld   s9, 200(sp)
    ld   s10, 208(sp)
    ld   s11, 216(sp)
    ld   t3, 224(sp)
    ld   t4, 232(sp)
    ld   t5, 240(sp)
    ld   t6, 248(sp)
    ld   t0,  40(sp)
    ld   sp,  16(sp)            # the user's sp, last
    sret

# void riscv_enter_frame(const TrapFrame *frame /a0/) -> !
.global riscv_enter_frame
.balign 4
riscv_enter_frame:
    fence.i                     # the image was just written through the direct map
    mv   sp, a0
    j    riscv_user_exit

# void riscv_fpu_save(u64 *state /a0/)    f0-f31 then fcsr
.global riscv_fpu_save
.balign 4
riscv_fpu_save:
    csrr t0, sstatus
    li   t1, 0x6000
    and  t0, t0, t1
    beqz t0, 23f
    fsd  f0 ,   0(a0)
    fsd  f1 ,   8(a0)
    fsd  f2 ,  16(a0)
    fsd  f3 ,  24(a0)
    fsd  f4 ,  32(a0)
    fsd  f5 ,  40(a0)
    fsd  f6 ,  48(a0)
    fsd  f7 ,  56(a0)
    fsd  f8 ,  64(a0)
    fsd  f9 ,  72(a0)
    fsd  f10,  80(a0)
    fsd  f11,  88(a0)
    fsd  f12,  96(a0)
    fsd  f13, 104(a0)
    fsd  f14, 112(a0)
    fsd  f15, 120(a0)
    fsd  f16, 128(a0)
    fsd  f17, 136(a0)
    fsd  f18, 144(a0)
    fsd  f19, 152(a0)
    fsd  f20, 160(a0)
    fsd  f21, 168(a0)
    fsd  f22, 176(a0)
    fsd  f23, 184(a0)
    fsd  f24, 192(a0)
    fsd  f25, 200(a0)
    fsd  f26, 208(a0)
    fsd  f27, 216(a0)
    fsd  f28, 224(a0)
    fsd  f29, 232(a0)
    fsd  f30, 240(a0)
    fsd  f31, 248(a0)
    frcsr t1
    sd   t1, 256(a0)
23:
    ret

# void riscv_fpu_restore(const u64 *state /a0/)
.global riscv_fpu_restore
.balign 4
riscv_fpu_restore:
    csrr t0, sstatus
    li   t1, 0x6000
    and  t0, t0, t1
    beqz t0, 24f
    fld  f0 ,   0(a0)
    fld  f1 ,   8(a0)
    fld  f2 ,  16(a0)
    fld  f3 ,  24(a0)
    fld  f4 ,  32(a0)
    fld  f5 ,  40(a0)
    fld  f6 ,  48(a0)
    fld  f7 ,  56(a0)
    fld  f8 ,  64(a0)
    fld  f9 ,  72(a0)
    fld  f10,  80(a0)
    fld  f11,  88(a0)
    fld  f12,  96(a0)
    fld  f13, 104(a0)
    fld  f14, 112(a0)
    fld  f15, 120(a0)
    fld  f16, 128(a0)
    fld  f17, 136(a0)
    fld  f18, 144(a0)
    fld  f19, 152(a0)
    fld  f20, 160(a0)
    fld  f21, 168(a0)
    fld  f22, 176(a0)
    fld  f23, 184(a0)
    fld  f24, 192(a0)
    fld  f25, 200(a0)
    fld  f26, 208(a0)
    fld  f27, 216(a0)
    fld  f28, 224(a0)
    fld  f29, 232(a0)
    fld  f30, 240(a0)
    fld  f31, 248(a0)
    ld   t1, 256(a0)
    fscsr t1
24:
    ret

# Traps taken in S-mode (the kernel itself). Exceptions are fatal and touch no
# stack; interrupts (the timer tick) save the caller-saved registers on the
# current stack, call riscv_irq_handler and sret back.
.balign 4
.Lkernel_trap:
    csrw sscratch, t0           # t0 is borrowed until the cause is known
    csrr t0, scause
    bgez t0, 1f              # exception: old path, stack untouched
    addi sp, sp, -304
    sd   ra,   0(sp)
    csrr ra, sscratch
    sd   ra,   8(sp)         # original t0
    csrw sscratch, zero      # t0 is saved: sscratch must read 0 from here on, even if this
                             # handler switches to another thread before it returns
    sd   t1,  16(sp)
    sd   t2,  24(sp)
    sd   t3,  32(sp)
    sd   t4,  40(sp)
    sd   t5,  48(sp)
    sd   t6,  56(sp)
    sd   a0,  64(sp)
    sd   a1,  72(sp)
    sd   a2,  80(sp)
    sd   a3,  88(sp)
    sd   a4,  96(sp)
    sd   a5, 104(sp)
    sd   a6, 112(sp)
    sd   a7, 120(sp)
    csrr t1, sepc
    sd   t1, 128(sp)
    csrr t1, sstatus
    sd   t1, 136(sp)         # SPP/SPIE of the interrupted code: another thread
                             # may take interrupts while this one is switched out
    li   t2, 0x6000
    and  t1, t1, t2
    beqz t1, 2f
    fsd  f0,  144(sp)
    fsd  f1,  152(sp)
    fsd  f2,  160(sp)
    fsd  f3,  168(sp)
    fsd  f4,  176(sp)
    fsd  f5,  184(sp)
    fsd  f6,  192(sp)
    fsd  f7,  200(sp)
    fsd  f10, 208(sp)
    fsd  f11, 216(sp)
    fsd  f12, 224(sp)
    fsd  f13, 232(sp)
    fsd  f14, 240(sp)
    fsd  f15, 248(sp)
    fsd  f16, 256(sp)
    fsd  f17, 264(sp)
    fsd  f28, 272(sp)
    fsd  f29, 280(sp)
    fsd  f30, 288(sp)
    fsd  f31, 296(sp)
2:
    csrr a0, scause
    call riscv_irq_handler
    csrr t1, sstatus
    li   t2, 0x6000
    and  t1, t1, t2
    beqz t1, 3f
    fld  f0,  144(sp)
    fld  f1,  152(sp)
    fld  f2,  160(sp)
    fld  f3,  168(sp)
    fld  f4,  176(sp)
    fld  f5,  184(sp)
    fld  f6,  192(sp)
    fld  f7,  200(sp)
    fld  f10, 208(sp)
    fld  f11, 216(sp)
    fld  f12, 224(sp)
    fld  f13, 232(sp)
    fld  f14, 240(sp)
    fld  f15, 248(sp)
    fld  f16, 256(sp)
    fld  f17, 264(sp)
    fld  f28, 272(sp)
    fld  f29, 280(sp)
    fld  f30, 288(sp)
    fld  f31, 296(sp)
3:
    ld   t1, 128(sp)
    csrw sepc, t1
    ld   t1, 136(sp)
    li   t2, 0x120           # SPP | SPIE
    and  t1, t1, t2
    csrc sstatus, t2
    csrs sstatus, t1
    ld   ra,   0(sp)
    ld   t0,   8(sp)
    ld   t1,  16(sp)
    ld   t2,  24(sp)
    ld   t3,  32(sp)
    ld   t4,  40(sp)
    ld   t5,  48(sp)
    ld   t6,  56(sp)
    ld   a0,  64(sp)
    ld   a1,  72(sp)
    ld   a2,  80(sp)
    ld   a3,  88(sp)
    ld   a4,  96(sp)
    ld   a5, 104(sp)
    ld   a6, 112(sp)
    ld   a7, 120(sp)
    addi sp, sp, 304
    csrw sscratch, zero      # S-mode convention: sscratch == 0 while in the kernel
    sret
1:
    csrr t0, sscratch
    csrw sscratch, zero
    j    riscv_trap_common
