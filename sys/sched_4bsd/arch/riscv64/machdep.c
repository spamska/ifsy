/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * riscv64 machine-dependent half of the scheduler: building the first
 * context of a new thread.  FreeBSD equivalent: cpu_fork()/cpu_copy_thread()
 * in sys/riscv/riscv/vm_machdep.c (s0 = function, s1 = argument, ra =
 * fork_trampoline).
 */
#include "4bsd.h"
#include "pcb.h"

extern void fork_trampoline(void);

void cpu_thread_setup(struct pcb *pcb, uintptr_t stack_top,
                      void (*entry)(void *), void *arg)
{
    stack_top &= ~(uintptr_t)15;

    for (int i = 0; i < 12; i++) {
        pcb->pcb_s[i] = 0;
        pcb->pcb_fs[i] = 0;
    }
    pcb->pcb_s[0] = (uint64_t)(uintptr_t)entry;             /* s0 */
    pcb->pcb_s[1] = (uint64_t)(uintptr_t)arg;               /* s1 */
    pcb->pcb_ra = (uint64_t)(uintptr_t)fork_trampoline;
    pcb->pcb_sp = stack_top;
    pcb->pcb_fcsr = 0;
}
