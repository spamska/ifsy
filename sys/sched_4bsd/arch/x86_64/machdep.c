/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * x86_64 machine-dependent half of the scheduler: building the first context
 * of a new thread.  FreeBSD equivalent: cpu_fork()/cpu_copy_thread() in
 * sys/amd64/amd64/vm_machdep.c, which also arrange for the first cpu_switch()
 * into a child to land in fork_trampoline().
 */
#include "4bsd.h"
#include "pcb.h"

extern void fork_trampoline(void);

void cpu_thread_setup(struct pcb *pcb, uintptr_t stack_top,
                      void (*entry)(void *), void *arg)
{
    uint64_t *top;

    stack_top &= ~(uintptr_t)15;
    top = (uint64_t *)stack_top;
    top[-1] = 0;                    /* sentinels: nothing to return into */
    top[-2] = 0;

    pcb->pcb_r15 = 0;
    pcb->pcb_r14 = 0;
    pcb->pcb_r13 = 0;
    pcb->pcb_r12 = (uint64_t)(uintptr_t)arg;     /* fork_trampoline: arg   */
    pcb->pcb_rbp = 0;
    pcb->pcb_rbx = (uint64_t)(uintptr_t)entry;   /* fork_trampoline: entry */
    /*
     * cpu_switch() stores pcb_rip at (pcb_rsp) and `ret`s, so pcb_rsp is the
     * slot the return address lives in and execution starts with
     * %rsp == pcb_rsp + 8.  fork_trampoline `call`s entry, which needs
     * %rsp 16-byte aligned at the call: (top - 24) + 8 == top - 16.
     */
    pcb->pcb_rsp = stack_top - 24;
    pcb->pcb_rip = (uint64_t)(uintptr_t)fork_trampoline;
}
