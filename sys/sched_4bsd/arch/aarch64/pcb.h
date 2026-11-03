/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * aarch64 per-thread CPU context.
 *
 * Modelled on FreeBSD sys/arm64/include/pcb.h.  cpu_switch() is a plain
 * function call, so per AAPCS64 only x19-x30, sp and the low halves of
 * v8-v15 (d8-d15) have to survive it.  The Rust half of the kernel is built
 * for the hard-float aarch64-unknown-none target and may keep live values in
 * d8-d15 across the switch, so unlike FreeBSD (whose kernel is compiled
 * -mgeneral-regs-only and saves VFP state lazily) they are saved eagerly.
 * FPCR/FPSR are saved too so one thread's rounding mode / flags cannot leak
 * into another.
 *
 * tpidr_el0 / tpidrro_el0 are the user thread pointers; saved as in FreeBSD.
 */
#ifndef _PCB_H_
#define _PCB_H_

#define PCB_X        0            /* x19..x30 (12 slots)               */
#define PCB_X19      0            /* index of x19 within the array     */
#define PCB_FP       10           /* index of x29                      */
#define PCB_LR       11           /* index of x30                      */
#define PCB_SP       96
#define PCB_TPIDR    104
#define PCB_TPIDRRO  112
#define PCB_D        120          /* d8..d15 (8 slots)                 */
#define PCB_FPCR     184
#define PCB_FPSR     192
#define PCB_SIZE     200

#ifndef __ASSEMBLER__
#include <stdint.h>
#include <stddef.h>

struct pcb {
    uint64_t pcb_x[12];       /* x19-x28, x29 (fp), x30 (lr) */
    uint64_t pcb_sp;
    uint64_t pcb_tpidr_el0;
    uint64_t pcb_tpidrro_el0;
    uint64_t pcb_d[8];        /* d8-d15 */
    uint64_t pcb_fpcr;
    uint64_t pcb_fpsr;
};

_Static_assert(offsetof(struct pcb, pcb_x) == PCB_X, "PCB_X");
_Static_assert(offsetof(struct pcb, pcb_sp) == PCB_SP, "PCB_SP");
_Static_assert(offsetof(struct pcb, pcb_tpidr_el0) == PCB_TPIDR, "PCB_TPIDR");
_Static_assert(offsetof(struct pcb, pcb_tpidrro_el0) == PCB_TPIDRRO, "PCB_TPIDRRO");
_Static_assert(offsetof(struct pcb, pcb_d) == PCB_D, "PCB_D");
_Static_assert(offsetof(struct pcb, pcb_fpcr) == PCB_FPCR, "PCB_FPCR");
_Static_assert(offsetof(struct pcb, pcb_fpsr) == PCB_FPSR, "PCB_FPSR");
_Static_assert(sizeof(struct pcb) == PCB_SIZE, "PCB_SIZE");
#endif /* !__ASSEMBLER__ */

#endif /* !_PCB_H_ */
