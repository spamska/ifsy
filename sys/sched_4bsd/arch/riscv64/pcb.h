/*
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * riscv64 per-thread CPU context.
 *
 * Modelled on FreeBSD sys/riscv/include/pcb.h.  cpu_switch() is a plain
 * function call, so per the psABI only ra, sp, s0-s11 and (with the D
 * extension, lp64d) fs0-fs11 plus fcsr have to survive it.
 *
 * gp and tp are NOT part of the context: in the kernel they are constants
 * (the global pointer, and the per-hart pointer) shared by every thread.  A
 * fresh thread starts with a zeroed PCB, so restoring them from it would
 * zero gp/tp and break the first gp-relative access.
 *
 * The FP half is only touched while sstatus.FS != Off; with the FPU off the
 * kernel cannot be using it and `fsd` would trap.
 */
#ifndef _PCB_H_
#define _PCB_H_

#define PCB_RA     0
#define PCB_SP     8
#define PCB_S      16             /* s0..s11  (12 slots) */
#define PCB_FS     112            /* fs0..fs11 (12 slots) */
#define PCB_FCSR   208
#define PCB_SIZE   216

#ifndef __ASSEMBLER__
#include <stdint.h>
#include <stddef.h>

struct pcb {
    uint64_t pcb_ra;
    uint64_t pcb_sp;
    uint64_t pcb_s[12];       /* s0-s11 */
    uint64_t pcb_fs[12];      /* fs0-fs11 (f8-f9, f18-f27) */
    uint64_t pcb_fcsr;
};

_Static_assert(offsetof(struct pcb, pcb_ra) == PCB_RA, "PCB_RA");
_Static_assert(offsetof(struct pcb, pcb_sp) == PCB_SP, "PCB_SP");
_Static_assert(offsetof(struct pcb, pcb_s) == PCB_S, "PCB_S");
_Static_assert(offsetof(struct pcb, pcb_fs) == PCB_FS, "PCB_FS");
_Static_assert(offsetof(struct pcb, pcb_fcsr) == PCB_FCSR, "PCB_FCSR");
_Static_assert(sizeof(struct pcb) == PCB_SIZE, "PCB_SIZE");
#endif /* !__ASSEMBLER__ */

#endif /* !_PCB_H_ */
