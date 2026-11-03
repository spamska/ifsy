/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * x86_64 per-thread CPU context ("process control block").
 *
 * Modelled on FreeBSD sys/amd64/include/pcb.h, cut down to what a kernel
 * thread switch needs: the System V callee-saved registers, the stack pointer
 * and the address cpu_switch() will "return" to.  Everything else (%rax,
 * %rcx, %rdx, %rsi, %rdi, %r8-%r11, flags) is caller-saved: cpu_switch() is an
 * ordinary function call as far as the compiler is concerned, so the caller
 * has already dealt with it.
 *
 * Not saved on purpose, as in FreeBSD's cpu_switch for kernel threads:
 *  - %rflags: interrupts are masked around a switch by the caller and each
 *    thread restores its own interrupt state from its own stack afterwards;
 *  - fs/gs base, debug registers, x87/SSE state: kernel threads do not use
 *    them (the kernel is built -mno-sse -msoft-float).
 *
 * The PCB_* offsets are used by switch.S, which includes this file, so the
 * struct is hidden from the assembler and the two are checked against each
 * other at compile time.
 */
#ifndef _PCB_H_
#define _PCB_H_

#define PCB_R15   0
#define PCB_R14   8
#define PCB_R13   16
#define PCB_R12   24
#define PCB_RBP   32
#define PCB_RSP   40
#define PCB_RBX   48
#define PCB_RIP   56
#define PCB_SIZE  64

#ifndef __ASSEMBLER__
#include <stdint.h>
#include <stddef.h>

struct pcb {
    uint64_t pcb_r15;
    uint64_t pcb_r14;
    uint64_t pcb_r13;
    uint64_t pcb_r12;
    uint64_t pcb_rbp;
    uint64_t pcb_rsp;   /* points at the slot holding the resume address */
    uint64_t pcb_rbx;
    uint64_t pcb_rip;   /* where cpu_switch() resumes this thread */
};

_Static_assert(offsetof(struct pcb, pcb_r15) == PCB_R15, "PCB_R15");
_Static_assert(offsetof(struct pcb, pcb_r14) == PCB_R14, "PCB_R14");
_Static_assert(offsetof(struct pcb, pcb_r13) == PCB_R13, "PCB_R13");
_Static_assert(offsetof(struct pcb, pcb_r12) == PCB_R12, "PCB_R12");
_Static_assert(offsetof(struct pcb, pcb_rbp) == PCB_RBP, "PCB_RBP");
_Static_assert(offsetof(struct pcb, pcb_rsp) == PCB_RSP, "PCB_RSP");
_Static_assert(offsetof(struct pcb, pcb_rbx) == PCB_RBX, "PCB_RBX");
_Static_assert(offsetof(struct pcb, pcb_rip) == PCB_RIP, "PCB_RIP");
_Static_assert(sizeof(struct pcb) == PCB_SIZE, "PCB_SIZE");
#endif /* !__ASSEMBLER__ */

#endif /* !_PCB_H_ */
