/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * riscv64 context switch, derived from FreeBSD sys/riscv/riscv/swtch.S
 * (cpu_switch / cpu_throw / fork_trampoline), reduced to kernel threads on
 * one hart.  Like FreeBSD, gp and tp are left alone (see pcb.h).  The FP
 * state is only the callee-saved subset and is skipped when sstatus.FS is
 * Off.
 */
#include "pcb.h"

#define SSTATUS_FS_MASK	0x6000

	.text

/*
 * void cpu_switch(struct pcb *old /a0/, struct pcb *new /a1/)
 *
 * Save ra, sp, s0-s11 (and fs0-fs11, fcsr when the FPU is on) of the calling
 * thread in *old and resume *new through its saved ra.
 */
	.globl	cpu_switch
	.type	cpu_switch,@function
	.p2align 2
cpu_switch:
	sd	ra, (PCB_RA)(a0)
	sd	sp, (PCB_SP)(a0)

	sd	s0,  (PCB_S + 0 * 8)(a0)
	sd	s1,  (PCB_S + 1 * 8)(a0)
	sd	s2,  (PCB_S + 2 * 8)(a0)
	sd	s3,  (PCB_S + 3 * 8)(a0)
	sd	s4,  (PCB_S + 4 * 8)(a0)
	sd	s5,  (PCB_S + 5 * 8)(a0)
	sd	s6,  (PCB_S + 6 * 8)(a0)
	sd	s7,  (PCB_S + 7 * 8)(a0)
	sd	s8,  (PCB_S + 8 * 8)(a0)
	sd	s9,  (PCB_S + 9 * 8)(a0)
	sd	s10, (PCB_S + 10 * 8)(a0)
	sd	s11, (PCB_S + 11 * 8)(a0)

	/* FPU on? (sstatus.FS != Off) */
	csrr	t0, sstatus
	li	t1, SSTATUS_FS_MASK
	and	t0, t0, t1
	beqz	t0, 1f
	fsd	fs0,  (PCB_FS + 0 * 8)(a0)
	fsd	fs1,  (PCB_FS + 1 * 8)(a0)
	fsd	fs2,  (PCB_FS + 2 * 8)(a0)
	fsd	fs3,  (PCB_FS + 3 * 8)(a0)
	fsd	fs4,  (PCB_FS + 4 * 8)(a0)
	fsd	fs5,  (PCB_FS + 5 * 8)(a0)
	fsd	fs6,  (PCB_FS + 6 * 8)(a0)
	fsd	fs7,  (PCB_FS + 7 * 8)(a0)
	fsd	fs8,  (PCB_FS + 8 * 8)(a0)
	fsd	fs9,  (PCB_FS + 9 * 8)(a0)
	fsd	fs10, (PCB_FS + 10 * 8)(a0)
	fsd	fs11, (PCB_FS + 11 * 8)(a0)
	frcsr	t0
	sd	t0, (PCB_FCSR)(a0)
1:
	mv	a0, a1			/* new context, then fall into cpu_throw */
	/* FALLTHROUGH */
	.size	cpu_switch,.-cpu_switch

/*
 * void cpu_throw(struct pcb *new /a0/)
 *
 * Resume *new without saving anything (the caller has exited).
 */
	.globl	cpu_throw
	.type	cpu_throw,@function
cpu_throw:
	ld	ra, (PCB_RA)(a0)
	ld	sp, (PCB_SP)(a0)

	ld	s0,  (PCB_S + 0 * 8)(a0)
	ld	s1,  (PCB_S + 1 * 8)(a0)
	ld	s2,  (PCB_S + 2 * 8)(a0)
	ld	s3,  (PCB_S + 3 * 8)(a0)
	ld	s4,  (PCB_S + 4 * 8)(a0)
	ld	s5,  (PCB_S + 5 * 8)(a0)
	ld	s6,  (PCB_S + 6 * 8)(a0)
	ld	s7,  (PCB_S + 7 * 8)(a0)
	ld	s8,  (PCB_S + 8 * 8)(a0)
	ld	s9,  (PCB_S + 9 * 8)(a0)
	ld	s10, (PCB_S + 10 * 8)(a0)
	ld	s11, (PCB_S + 11 * 8)(a0)

	csrr	t0, sstatus
	li	t1, SSTATUS_FS_MASK
	and	t0, t0, t1
	beqz	t0, 1f
	fld	fs0,  (PCB_FS + 0 * 8)(a0)
	fld	fs1,  (PCB_FS + 1 * 8)(a0)
	fld	fs2,  (PCB_FS + 2 * 8)(a0)
	fld	fs3,  (PCB_FS + 3 * 8)(a0)
	fld	fs4,  (PCB_FS + 4 * 8)(a0)
	fld	fs5,  (PCB_FS + 5 * 8)(a0)
	fld	fs6,  (PCB_FS + 6 * 8)(a0)
	fld	fs7,  (PCB_FS + 7 * 8)(a0)
	fld	fs8,  (PCB_FS + 8 * 8)(a0)
	fld	fs9,  (PCB_FS + 9 * 8)(a0)
	fld	fs10, (PCB_FS + 10 * 8)(a0)
	fld	fs11, (PCB_FS + 11 * 8)(a0)
	ld	t0, (PCB_FCSR)(a0)
	fscsr	t0
1:
	ret
	.size	cpu_throw,.-cpu_throw

/*
 * First code a new thread runs (cpu_thread_setup() puts its address in the
 * saved ra).  s0 = entry function, s1 = its argument.  Entry functions never
 * return; if one does, park the hart.
 */
	.globl	fork_trampoline
	.type	fork_trampoline,@function
	.p2align 2
fork_trampoline:
	mv	a0, s1
	mv	t0, s0
	li	s0, 0			/* frame-pointer chain ends here */
	jalr	t0
1:	csrci	sstatus, 0x2
	wfi
	j	1b
	.size	fork_trampoline,.-fork_trampoline

	.section .note.GNU-stack,"",@progbits
