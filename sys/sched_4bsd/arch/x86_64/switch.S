/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * x86_64 context switch, derived from FreeBSD sys/amd64/amd64/cpu_switch.S
 * (cpu_switch / cpu_throw / the ret-through-the-saved-rip trick), reduced to
 * kernel threads on one CPU.  AT&T syntax, System V ABI.
 */
#include "pcb.h"

	.text

/*
 * void cpu_switch(struct pcb *old /%rdi/, struct pcb *new /%rsi/)
 *
 * Save the callee-saved registers, %rsp and the return address of the
 * calling thread in *old, then resume *new.  When some other thread later
 * switches back to *old it appears to return from this call.
 *
 * The resume address is saved from (%rsp) and PCB_RSP is %rsp as it is
 * *now*, i.e. pointing AT the return address.  Restoring puts the saved rip
 * back into (%rsp) and `ret`s, which pops it, leaving %rsp exactly where the
 * thread's own `ret` would have left it.  For a fresh thread that same store
 * is what plants the address of fork_trampoline.
 */
	.globl	cpu_switch
	.type	cpu_switch,@function
	.p2align 4
cpu_switch:
	movq	(%rsp),%rax
	movq	%r15,PCB_R15(%rdi)
	movq	%r14,PCB_R14(%rdi)
	movq	%r13,PCB_R13(%rdi)
	movq	%r12,PCB_R12(%rdi)
	movq	%rbp,PCB_RBP(%rdi)
	movq	%rsp,PCB_RSP(%rdi)
	movq	%rbx,PCB_RBX(%rdi)
	movq	%rax,PCB_RIP(%rdi)
	movq	%rsi,%rdi		/* new context, then fall into cpu_throw */
	/* FALLTHROUGH */
	.size	cpu_switch,.-cpu_switch

/*
 * void cpu_throw(struct pcb *new /%rdi/)
 *
 * Resume *new without saving anything.  Used when the calling thread has
 * exited: its stack is still in use up to the `movq ...,%rsp` below and is
 * never touched again afterwards.
 */
	.globl	cpu_throw
	.type	cpu_throw,@function
cpu_throw:
	movq	PCB_R15(%rdi),%r15
	movq	PCB_R14(%rdi),%r14
	movq	PCB_R13(%rdi),%r13
	movq	PCB_R12(%rdi),%r12
	movq	PCB_RBP(%rdi),%rbp
	movq	PCB_RSP(%rdi),%rsp
	movq	PCB_RBX(%rdi),%rbx
	movq	PCB_RIP(%rdi),%rax
	movq	%rax,(%rsp)
	ret
	.size	cpu_throw,.-cpu_throw

/*
 * First code a new thread runs (cpu_thread_setup() points pcb_rip here).
 * %rbx = entry function, %r12 = its argument; both came out of the PCB.
 * Entry functions never return; if one does, stop rather than fall off the
 * end of the stack.
 */
	.globl	fork_trampoline
	.type	fork_trampoline,@function
	.p2align 4
fork_trampoline:
	movq	%r12,%rdi
	xorl	%ebp,%ebp		/* end of the frame-pointer chain */
	callq	*%rbx
1:	cli
	hlt
	jmp	1b
	.size	fork_trampoline,.-fork_trampoline

	.section .note.GNU-stack,"",@progbits
