/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * aarch64 context switch, derived from FreeBSD sys/arm64/arm64/swtch.S
 * (cpu_switch / cpu_throw / fork_trampoline), reduced to kernel threads on
 * one CPU.  The VFP half differs from FreeBSD on purpose, see pcb.h.
 */
#include "pcb.h"

	.text

/*
 * void cpu_switch(struct pcb *old /x0/, struct pcb *new /x1/)
 *
 * Save the callee-saved state of the calling thread in *old and resume *new.
 * Control returns to the new thread through its saved x30.
 */
	.globl	cpu_switch
	.type	cpu_switch,%function
	.p2align 4
cpu_switch:
	/* Callee-saved integer registers, including fp (x29) and lr (x30). */
	stp	x19, x20, [x0, #(PCB_X + (PCB_X19 + 0) * 8)]
	stp	x21, x22, [x0, #(PCB_X + (PCB_X19 + 2) * 8)]
	stp	x23, x24, [x0, #(PCB_X + (PCB_X19 + 4) * 8)]
	stp	x25, x26, [x0, #(PCB_X + (PCB_X19 + 6) * 8)]
	stp	x27, x28, [x0, #(PCB_X + (PCB_X19 + 8) * 8)]
	stp	x29, x30, [x0, #(PCB_X + (PCB_X19 + 10) * 8)]

	/* Stack pointer and the EL0 thread pointers. */
	mov	x5, sp
	mrs	x6, tpidr_el0
	stp	x5, x6, [x0, #PCB_SP]
	mrs	x7, tpidrro_el0
	str	x7, [x0, #PCB_TPIDRRO]

	/* Callee-saved FP/SIMD: d8-d15, plus FPCR/FPSR. */
	stp	d8,  d9,  [x0, #(PCB_D + 0 * 8)]
	stp	d10, d11, [x0, #(PCB_D + 2 * 8)]
	stp	d12, d13, [x0, #(PCB_D + 4 * 8)]
	stp	d14, d15, [x0, #(PCB_D + 6 * 8)]
	mrs	x5, fpcr
	mrs	x6, fpsr
	stp	x5, x6, [x0, #PCB_FPCR]

	mov	x0, x1			/* new context, then fall into cpu_throw */
	/* FALLTHROUGH */
	.size	cpu_switch,.-cpu_switch

/*
 * void cpu_throw(struct pcb *new /x0/)
 *
 * Resume *new without saving anything (the caller has exited).
 */
	.globl	cpu_throw
	.type	cpu_throw,%function
cpu_throw:
	mov	x4, x0

	ldp	x5, x6, [x4, #PCB_FPCR]
	msr	fpcr, x5
	msr	fpsr, x6
	ldp	d8,  d9,  [x4, #(PCB_D + 0 * 8)]
	ldp	d10, d11, [x4, #(PCB_D + 2 * 8)]
	ldp	d12, d13, [x4, #(PCB_D + 4 * 8)]
	ldp	d14, d15, [x4, #(PCB_D + 6 * 8)]

	ldp	x5, x6, [x4, #PCB_SP]
	mov	sp, x5
	msr	tpidr_el0, x6
	ldr	x7, [x4, #PCB_TPIDRRO]
	msr	tpidrro_el0, x7

	ldp	x19, x20, [x4, #(PCB_X + (PCB_X19 + 0) * 8)]
	ldp	x21, x22, [x4, #(PCB_X + (PCB_X19 + 2) * 8)]
	ldp	x23, x24, [x4, #(PCB_X + (PCB_X19 + 4) * 8)]
	ldp	x25, x26, [x4, #(PCB_X + (PCB_X19 + 6) * 8)]
	ldp	x27, x28, [x4, #(PCB_X + (PCB_X19 + 8) * 8)]
	ldp	x29, x30, [x4, #(PCB_X + (PCB_X19 + 10) * 8)]

	ret
	.size	cpu_throw,.-cpu_throw

/*
 * First code a new thread runs (cpu_thread_setup() puts its address in the
 * saved x30).  x19 = entry function, x20 = its argument.  Entry functions
 * never return; if one does, park the CPU.
 */
	.globl	fork_trampoline
	.type	fork_trampoline,%function
	.p2align 4
fork_trampoline:
	mov	x0, x20
	mov	x29, xzr		/* stack traceback stops here */
	blr	x19
1:	msr	daifset, #0xf
	wfe
	b	1b
	.size	fork_trampoline,.-fork_trampoline

	.section .note.GNU-stack,"",@progbits
