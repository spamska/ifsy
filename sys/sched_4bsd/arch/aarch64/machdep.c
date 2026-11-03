/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * aarch64 machine-dependent half of the scheduler: building the first
 * context of a new thread.  FreeBSD equivalent: cpu_fork()/cpu_copy_thread()
 * in sys/arm64/arm64/vm_machdep.c (x19 = function, x20 = argument, lr =
 * fork_trampoline).
 */
#include "4bsd.h"
#include "pcb.h"

extern void fork_trampoline(void);

void cpu_thread_setup(struct pcb *pcb, uintptr_t stack_top,
                      void (*entry)(void *), void *arg)
{
    stack_top &= ~(uintptr_t)15;

    for (int i = 0; i < 12; i++)
        pcb->pcb_x[i] = 0;
    for (int i = 0; i < 8; i++)
        pcb->pcb_d[i] = 0;
    pcb->pcb_x[PCB_X19 + 0] = (uint64_t)(uintptr_t)entry;   /* x19 */
    pcb->pcb_x[PCB_X19 + 1] = (uint64_t)(uintptr_t)arg;     /* x20 */
    pcb->pcb_x[PCB_FP] = 0;                                 /* x29 */
    pcb->pcb_x[PCB_LR] = (uint64_t)(uintptr_t)fork_trampoline; /* x30 */
    pcb->pcb_sp = stack_top;
    pcb->pcb_tpidr_el0 = 0;
    pcb->pcb_tpidrro_el0 = 0;
    pcb->pcb_fpcr = 0;
    pcb->pcb_fpsr = 0;
}
