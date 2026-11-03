/*
 * SPDX-License-Identifier: BSD-3-Clause
 *
 * Copyright (c) 1982, 1986, 1990, 1991, 1993
 *     The Regents of the University of California.  All rights reserved.
 * (c) UNIX System Laboratories, Inc.
 * All or some portions of this file are derived from material licensed
 * to the University of California by American Telephone and Telegraph
 * Co. or Unix System Laboratories, Inc. and are reproduced herein with
 * the permission of UNIX System Laboratories, Inc.
 *
 * This is a port of FreeBSD's sys/kern/sched_4bsd.c and the run queue code of
 * sys/kern/kern_switch.c to Ifsy.  Policy and arithmetic are FreeBSD's; the
 * plumbing (thread table, switch halves, CPU hooks) is Ifsy's.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 * 3. Neither the name of the University nor the names of its contributors
 *    may be used to endorse or promote products derived from this software
 *    without specific prior written permission.
 *
 * THIS SOFTWARE IS PROVIDED BY THE REGENTS AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE REGENTS OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */

#include "4bsd.h"
#include "pcb.h"

#define MAX_THREADS     256
#define NOCPU           (-1)

/* sys/param.h: fixed point with FSHIFT fraction bits. */
#define FSHIFT          11
#define FSCALE          (1 << FSHIFT)
/* exp(-1/20) * FSCALE and the three load-average decay constants, computed
 * by the compiler in FreeBSD (0.95122942450071400909 * FSCALE etc.); spelled
 * out here so the code never needs the FPU. */
#define CCPU_INIT       1948
#define CCPU_SHIFT      11
static const uint32_t cexp[3] = { 1884, 2014, 2036 };

#define INVERSE_ESTCPU_WEIGHT   8   /* 1 / (priorities per estcpu level) */
#define NICE_WEIGHT             1   /* priorities per nice level         */
#define ESTCPULIM(e)                                                       \
    umin((e), (uint32_t)(INVERSE_ESTCPU_WEIGHT *                           \
        (PRI_MAX_TIMESHARE - PRI_MIN_TIMESHARE -                           \
         (PRIO_MAX - PRIO_MIN) * NICE_WEIGHT) +                            \
        INVERSE_ESTCPU_WEIGHT - 1))

#define TSF_AFFINITY    0x0001      /* has a non-"full" CPU set */

#define PREEMPTION                  /* sched_add() may preempt (not FULL_) */

typedef struct sched_thread sched_thread_t;

enum { TDS_FREE, TDS_CAN_RUN, TDS_RUNQ, TDS_RUNNING, TDS_SLEEPING };

struct sched_thread {
    struct pcb pcb;             /* MUST BE FIRST - CPU context lives here */
    thread_t *handle;           /* NULL => slot is free                   */
    thread_t *parent;           /* who gets our estcpu when we exit       */
    sched_thread_t *rq_next, *rq_prev;     /* run queue linkage           */
    int rqindex;                /* td_rqindex: queue we are on            */
    int state;                  /* TDS_*                                  */
    int priority;               /* td_priority: what the queues use       */
    int base_pri;               /* td_base_pri                            */
    int user_pri;               /* td_user_pri                            */
    int base_user_pri;          /* td_base_user_pri                       */
    int lend_user_pri;          /* td_lend_user_pri                       */
    int pri_class;              /* PRI_*                                  */
    int nice;                   /* p_nice                                 */
    int flags;                  /* TDF_*                                  */
    int pinned;                 /* td_pinned                              */
    int lastcpu, oncpu;
    uint64_t cpuset;            /* td_cpuset->cs_mask                     */
    /* struct td_sched */
    uint32_t ts_pctcpu;         /* %cpu during p_swtime                   */
    uint32_t ts_estcpu;         /* estimated cpu utilization              */
    int ts_cpticks;             /* ticks of cpu time                      */
    int ts_slptime;             /* seconds !RUNNING                       */
    int ts_slice;               /* remaining part of time slice           */
    int ts_flags;               /* TSF_*                                  */
    int ts_rqcpu;               /* that CPU's runq or NOCPU => global     */
    bool ctx_valid;             /* pcb holds a context cpu_switch can resume */
};

struct runq {
    uint64_t status;                        /* bit i: queue i not empty */
    sched_thread_t *head[RQ_NQS];
    sched_thread_t *tail[RQ_NQS];
};

static int timer_hz;
static int real_stat_hz = 100;
static int sched_slice = 10;            /* thread run time before rescheduling */
static int sched_tdcnt;                 /* total runnable threads in the system */
static uint32_t ccpu = CCPU_INIT;
static uint32_t ldavg[3];
static unsigned bg_calls;

static int ncpus = 1;
static bool smp_started;
static const int runq_fuzz = 1;
static const int forward_wakeup_enabled = 1;
static const int forward_wakeup_use_mask = 1;
static const int forward_wakeup_use_loop = 0;

static sched_thread_t thread_pool[MAX_THREADS];
static int pool_top;                    /* 1 + highest slot index in use */

static struct runq runq_global;
static struct runq runq_pcpu[SCHED_MAXCPU];
static long runq_length[SCHED_MAXCPU];
static uint64_t idle_cpus_mask;
static sched_thread_t *curthread[SCHED_MAXCPU];   /* NULL => idle context */
static bool cpu_owepreempt[SCHED_MAXCPU];         /* td_owepreempt        */
static bool cpu_resched[SCHED_MAXCPU];            /* TDP_RESCHED / TDA_SCHED */

/* Per-CPU idle contexts (handle NULL).  Not in the pool: never queued. */
static struct pcb idle_pcb[SCHED_MAXCPU];
static bool idle_ctx_valid[SCHED_MAXCPU];
static bool ctx_switching;

/* ---- machine-dependent hooks: single-CPU defaults ------------------------ */

__attribute__((weak)) int sched_curcpu(void) { return 0; }
__attribute__((weak)) void sched_ipi(int cpu, int kind) { (void)cpu; (void)kind; }
__attribute__((weak)) bool sched_cpu_idle_wakeup(int cpu) { (void)cpu; return false; }

static inline int imin(int a, int b) { return a < b ? a : b; }
static inline int imax(int a, int b) { return a > b ? a : b; }
static inline uint32_t umin(uint32_t a, uint32_t b) { return a < b ? a : b; }
static inline uint64_t cpubit(int cpu) { return (uint64_t)1 << cpu; }

static int curcpu(void) {
    int c = sched_curcpu();
    return (c < 0 || c >= SCHED_MAXCPU) ? 0 : c;
}

static inline int td_prio_of(const sched_thread_t *td) {
    return td ? td->priority : PRI_MAX_IDLE;    /* the idle thread's priority */
}

/* ---- thread table ----------------------------------------------------- */

static sched_thread_t *get_sched_thread(thread_t *h) {
    if (!h) return NULL;
    for (int i = 0; i < pool_top; i++)
        if (thread_pool[i].handle == h)
            return &thread_pool[i];
    return NULL;
}

/*
 * Zero a PCB without depending on the C library: this file is built
 * freestanding and the PCB layout differs per architecture. The volatile
 * keeps the compiler from turning the loop back into a memset() call.
 */
static void pcb_clear(struct pcb *pcb) {
    volatile uint64_t *w = (volatile uint64_t *)pcb;
    for (size_t i = 0; i < sizeof(*pcb) / sizeof(uint64_t); i++) w[i] = 0;
}

/* ---- run queues (kern_switch.c) --------------------------------------- */

static void runq_init(struct runq *rq) {
    rq->status = 0;
    for (int i = 0; i < RQ_NQS; i++) rq->head[i] = rq->tail[i] = NULL;
}

static bool runq_not_empty(const struct runq *rq) { return rq->status != 0; }

/* Lowest set bit = best priority. */
static int runq_findbit(const struct runq *rq) {
    return rq->status ? __builtin_ctzll(rq->status) : -1;
}

static void runq_add(struct runq *rq, sched_thread_t *td, int flags) {
    int idx = RQ_PRI_TO_QUEUE_IDX(td->priority);

    td->rqindex = idx;
    rq->status |= (uint64_t)1 << idx;
    if (flags & SRQ_PREEMPTED) {            /* TAILQ_INSERT_HEAD */
        td->rq_prev = NULL;
        td->rq_next = rq->head[idx];
        if (td->rq_next) td->rq_next->rq_prev = td; else rq->tail[idx] = td;
        rq->head[idx] = td;
    } else {                                /* TAILQ_INSERT_TAIL */
        td->rq_next = NULL;
        td->rq_prev = rq->tail[idx];
        if (td->rq_prev) td->rq_prev->rq_next = td; else rq->head[idx] = td;
        rq->tail[idx] = td;
    }
}

static void runq_remove(struct runq *rq, sched_thread_t *td) {
    int idx = td->rqindex;

    if (td->rq_prev) td->rq_prev->rq_next = td->rq_next; else rq->head[idx] = td->rq_next;
    if (td->rq_next) td->rq_next->rq_prev = td->rq_prev; else rq->tail[idx] = td->rq_prev;
    td->rq_next = td->rq_prev = NULL;
    if (rq->head[idx] == NULL)
        rq->status &= ~((uint64_t)1 << idx);
}

static sched_thread_t *runq_choose(struct runq *rq) {
    int pri = runq_findbit(rq);
    return pri < 0 ? NULL : rq->head[pri];
}

/*
 * Like runq_choose(), but among the first `fuzz` threads of the best queue
 * prefer one that last ran on this CPU (cache warmth).
 */
static sched_thread_t *runq_choose_fuzz(struct runq *rq, int fuzz) {
    int pri = runq_findbit(rq);
    sched_thread_t *td;

    if (pri < 0) return NULL;
    td = rq->head[pri];
    if (fuzz > 1) {
        int count = fuzz, cpu = curcpu();
        for (sched_thread_t *td2 = td; count-- && td2; td2 = td2->rq_next) {
            if (td2->lastcpu == cpu) { td = td2; break; }
        }
    }
    return td;
}

static struct runq *ts_runq_ptr(const sched_thread_t *td) {
    return td->ts_rqcpu == NOCPU ? &runq_global : &runq_pcpu[td->ts_rqcpu];
}

#define THREAD_CAN_SCHED(td, cpu)   (((td)->cpuset >> (cpu)) & 1)

/* ---- load, priority arithmetic ---------------------------------------- */

static void sched_load_add(void) { sched_tdcnt++; }
static void sched_load_rem(void) { sched_tdcnt--; }

/* Digital decay: 90% of estcpu forgotten in 5 * loadav seconds. */
#define loadfactor(loadav)          (2 * (loadav))
static uint32_t decay_cpu(uint32_t loadfac, uint32_t cpu) {
    return (loadfac * cpu) / (loadfac + FSCALE);
}

/* Arrange to reschedule if `td` should run before what this CPU runs now. */
static void maybe_resched(sched_thread_t *td) {
    int cpu = curcpu();

    if (td->priority < td_prio_of(curthread[cpu]))
        cpu_resched[cpu] = true;
}

/*
 * `td` is about to go on a run queue.  Should it preempt this CPU's current
 * thread?  If so, flag a preemption (td_owepreempt) and say yes.
 */
static bool maybe_preempt(sched_thread_t *td) {
#ifdef PREEMPTION
    int cpu = curcpu();
    sched_thread_t *ctd = curthread[cpu];
    int cpri = td_prio_of(ctd), pri = td->priority;

    /*
     * Not if the current thread has an equal or better priority (this also
     * keeps a thread from preempting itself), or is on its way out anyway.
     */
    if (pri >= cpri || (ctd && ctd->state == TDS_SLEEPING))
        return false;
#ifndef FULL_PREEMPTION
    if (pri > PRI_MAX_ITHD && cpri < PRI_MIN_IDLE)
        return false;
#endif
    cpu_owepreempt[cpu] = true;
    return true;
#else
    (void)td;
    return false;
#endif
}

/* resetpriority(): the user priority of a timesharing thread. */
static void resetpriority(sched_thread_t *td) {
    uint32_t newpriority;

    if (td->pri_class != PRI_TIMESHARE)
        return;
    newpriority = PRI_MIN_TIMESHARE +
        td->ts_estcpu / INVERSE_ESTCPU_WEIGHT +
        NICE_WEIGHT * (uint32_t)(td->nice - PRIO_MIN);
    if (newpriority > PRI_MAX_TIMESHARE)
        newpriority = PRI_MAX_TIMESHARE;
    sched_user_prio(td->handle, (int)newpriority);
}

static void set_priority(sched_thread_t *td, int prio);
static void sched_rem(sched_thread_t *td);
static void sched_add(sched_thread_t *td, int flags);

/* resetpriority_thread(): follow a changed user priority. */
static void resetpriority_thread(sched_thread_t *td) {
    /* Only change threads with a time sharing user priority. */
    if (td->priority < PRI_MIN_TIMESHARE || td->priority > PRI_MAX_TIMESHARE)
        return;
    /* XXX the whole needresched thing is broken, but not silly. */
    maybe_resched(td);
    sched_prio(td->handle, td->user_pri);
}

/* Recalculate the priority of a thread after it has slept for a while. */
static void updatepri(sched_thread_t *td) {
    uint32_t loadfac = loadfactor(ldavg[0]);
    uint32_t newcpu;

    if ((uint32_t)td->ts_slptime > 5 * loadfac) {
        td->ts_estcpu = 0;
    } else {
        newcpu = td->ts_estcpu;
        /* schedcpu() performs one decay_cpu() on its own. */
        td->ts_slptime--;
        while (newcpu && --td->ts_slptime)
            newcpu = decay_cpu(loadfac, newcpu);
        td->ts_estcpu = newcpu;
    }
}

/*
 * Recompute priorities, every hz ticks.
 */
static void schedcpu(void) {
    uint32_t loadfac = loadfactor(ldavg[0]);

    for (int i = 0; i < pool_top; i++) {
        sched_thread_t *td = &thread_pool[i];
        int awake = 0;

        if (!td->handle) continue;
        if (td->state == TDS_RUNQ) {
            awake = 1;
            td->flags &= ~TDF_DIDRUN;
        } else if (td->state == TDS_RUNNING) {
            awake = 1;
            /* Do not clear TDF_DIDRUN */
        } else if (td->flags & TDF_DIDRUN) {
            awake = 1;
            td->flags &= ~TDF_DIDRUN;
        }

        /* ts_pctcpu is only for ps and ttyinfo(). */
        td->ts_pctcpu = (td->ts_pctcpu * ccpu) >> FSHIFT;
        if (td->ts_cpticks != 0) {
            td->ts_pctcpu += (real_stat_hz == 100)
                ? ((uint32_t)td->ts_cpticks) << (FSHIFT - CCPU_SHIFT)
                : 100 * (((uint32_t)td->ts_cpticks) << (FSHIFT - CCPU_SHIFT))
                      / (uint32_t)real_stat_hz;
            td->ts_cpticks = 0;
        }

        if (awake) {
            if (td->ts_slptime > 1) {
                /*
                 * Whoever woke us from the long sleep should have unwound
                 * slptime and reset our priority first; be safe.
                 */
                updatepri(td);
            }
            td->ts_slptime = 0;
        } else {
            td->ts_slptime++;
        }

        /* Idle the whole second: stop recalculating until it wakes up. */
        if (td->ts_slptime > 1)
            continue;
        td->ts_estcpu = decay_cpu(loadfac, td->ts_estcpu);
        resetpriority(td);
        resetpriority_thread(td);
    }
}

/* vmmeter.c loadav(): every 5 seconds. */
static void loadav(void) {
    int nrun = sched_tdcnt;

    for (int i = 0; i < 3; i++)
        ldavg[i] = (cexp[i] * ldavg[i] +
            (uint32_t)nrun * FSCALE * (FSCALE - cexp[i])) >> FSHIFT;
}

void sched_background_work(void) {
    schedcpu();
    if (++bg_calls % 5 == 0)
        loadav();
}

/* ---- initialisation -------------------------------------------------- */

void sched_init(int hz, int stathz) {
    timer_hz = hz;
    real_stat_hz = stathz ? stathz : hz;
    if (real_stat_hz <= 0) real_stat_hz = 100;
    sched_slice = imax(1, real_stat_hz / 10);       /* ~100ms */
    ccpu = CCPU_INIT;

    for (int i = 0; i < MAX_THREADS; i++) {
        thread_pool[i].handle = NULL;
        thread_pool[i].state = TDS_FREE;
    }
    pool_top = 0;
    sched_tdcnt = 0;
    ldavg[0] = ldavg[1] = ldavg[2] = 0;
    bg_calls = 0;
    ncpus = 1;
    smp_started = false;
    idle_cpus_mask = 0;

    runq_init(&runq_global);
    for (int c = 0; c < SCHED_MAXCPU; c++) {
        runq_init(&runq_pcpu[c]);
        runq_length[c] = 0;
        curthread[c] = NULL;
        cpu_owepreempt[c] = false;
        cpu_resched[c] = false;
    }
    /*
     * Deliberately NOT reset: idle_pcb / idle_ctx_valid. The idle contexts
     * own real stacks that survive a re-init of the run queues, and
     * ctx_switching is a property of the caller, not of the run queues.
     */
}

void sched_set_ncpus(int n) {
    ncpus = imax(1, imin(SCHED_MAXCPU, n));
    smp_started = ncpus > 1;
}

int sched_ncpus(void) { return ncpus; }

static sched_thread_t *add_thread(thread_t *h) {
    int slot = -1;
    for (int i = 0; i < MAX_THREADS; i++) {
        if (thread_pool[i].handle == NULL) { slot = i; break; }
    }
    if (slot < 0) return NULL;
    if (slot >= pool_top) pool_top = slot + 1;

    sched_thread_t *td = &thread_pool[slot];
    pcb_clear(&td->pcb);
    td->handle = h;
    td->parent = NULL;
    td->rq_next = td->rq_prev = NULL;
    td->rqindex = 0;
    td->state = TDS_CAN_RUN;
    td->pri_class = PRI_TIMESHARE;
    td->nice = 0;
    td->flags = 0;
    td->pinned = 0;
    td->lastcpu = td->oncpu = NOCPU;
    td->cpuset = ~(uint64_t)0;
    td->ts_pctcpu = 0;
    td->ts_estcpu = 0;
    td->ts_cpticks = 0;
    td->ts_slptime = 0;
    td->ts_slice = sched_slice;
    td->ts_flags = 0;
    td->ts_rqcpu = NOCPU;
    td->ctx_valid = false;
    td->lend_user_pri = PRI_MAX;
    td->user_pri = td->base_user_pri = PRI_MIN_TIMESHARE;
    resetpriority(td);
    td->priority = td->base_pri = td->user_pri;
    return td;
}

static void remove_thread(sched_thread_t *td) {
    td->handle = NULL;
    td->state = TDS_FREE;
    while (pool_top > 0 && thread_pool[pool_top - 1].handle == NULL)
        pool_top--;
}

/* ---- the clock -------------------------------------------------------- */

bool sched_tick(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    int cpu = curcpu();

    if (!td) return false;

    td->ts_cpticks++;
    td->ts_estcpu = ESTCPULIM(td->ts_estcpu + 1);
    if ((td->ts_estcpu % INVERSE_ESTCPU_WEIGHT) == 0) {
        resetpriority(td);
        resetpriority_thread(td);
    }

    /* Force a context switch if the thread used up a full time slice. */
    if (--td->ts_slice <= 0) {
        td->ts_slice = sched_slice;

        /* An ithread that uses a full quantum is demoted and preempted. */
        if (PRI_BASE(td->pri_class) == PRI_ITHD) {
            cpu_owepreempt[cpu] = true;
            if (td->base_pri + RQ_PPQ < PRI_MAX_ITHD)
                sched_prio(h, td->base_pri + RQ_PPQ);
        } else {
            td->flags |= TDF_SLICEEND;
            cpu_resched[cpu] = true;
        }
    }
    return (td->flags & TDF_SLICEEND) || cpu_owepreempt[cpu] || cpu_resched[cpu];
}

/* ---- priorities -------------------------------------------------------- */

/* sched_priority(): adjust the priority of a thread. */
static void set_priority(sched_thread_t *td, int prio) {
    if (td->priority == prio)
        return;
    td->priority = prio;
    if (td->state == TDS_RUNQ && td->rqindex != RQ_PRI_TO_QUEUE_IDX(prio)) {
        sched_rem(td);
        sched_add(td, SRQ_BORING);
    }
}

void sched_prio(thread_t *h, int prio) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;
    prio = imin(PRI_MAX, imax(PRI_MIN, prio));

    /* First, update the base priority. */
    td->base_pri = prio;

    /* A thread borrowing another's priority never has it lowered. */
    if ((td->flags & TDF_BORROWING) && td->priority < prio)
        return;

    /* Change the real priority. */
    set_priority(td, prio);
}

void sched_user_prio(thread_t *h, int prio) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;
    td->base_user_pri = prio;
    if (td->lend_user_pri <= prio)
        return;
    td->user_pri = prio;
}

void sched_lend_user_prio(thread_t *h, int prio) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;
    td->lend_user_pri = prio;
    td->user_pri = imin(prio, td->base_user_pri);
    if (td->priority > td->user_pri)
        sched_prio(h, td->user_pri);
    else if (td->priority != td->user_pri && td->state == TDS_RUNNING)
        cpu_resched[td->oncpu == NOCPU ? curcpu() : td->oncpu] = true;
}

void sched_lend_user_prio_cond(thread_t *h, int prio) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td || td->lend_user_pri == prio) return;
    sched_lend_user_prio(h, prio);
}

/* Lend `prio` to `td` (priority propagation). */
void sched_lend_prio(thread_t *h, int prio) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;
    td->flags |= TDF_BORROWING;
    set_priority(td, imin(PRI_MAX, imax(PRI_MIN, prio)));
}

/*
 * Restore a thread's priority when priority propagation is over.  `prio`
 * is the minimum priority the thread needs to satisfy other possible
 * lending requests.
 */
void sched_unlend_prio(thread_t *h, int prio) {
    sched_thread_t *td = get_sched_thread(h);
    int base_pri;

    if (!td) return;
    if (td->base_pri >= PRI_MIN_TIMESHARE && td->base_pri <= PRI_MAX_TIMESHARE)
        base_pri = td->user_pri;
    else
        base_pri = td->base_pri;
    if (prio >= base_pri) {
        td->flags &= ~TDF_BORROWING;
        sched_prio(h, base_pri);
    } else {
        sched_lend_prio(h, prio);
    }
}

void sched_userret_slowpath(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;
    td->priority = td->user_pri;
    td->base_pri = td->user_pri;
}

void sched_nice(thread_t *h, int nice) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;
    td->nice = imin(PRIO_MAX, imax(PRIO_MIN, nice));
    resetpriority(td);
    resetpriority_thread(td);
}

void sched_set_class(thread_t *h, int class_) {
    sched_thread_t *td = get_sched_thread(h);
    if (td) td->pri_class = class_;
}

/* ---- sleeping and waking -------------------------------------------- */

void sched_sleep(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return;

    if (td->state == TDS_RUNQ)
        sched_rem(td);              /* not a thread the CPU is running */
    td->ts_slptime = 0;
    td->state = TDS_SLEEPING;
}

bool sched_wakeup(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    int cpu = curcpu();

    if (!td) {
        td = add_thread(h);
        if (!td) return false;
    }
    if (td->state == TDS_RUNQ || td->state == TDS_RUNNING)
        return cpu_owepreempt[cpu] || cpu_resched[cpu];

    td->state = TDS_CAN_RUN;
    if (td->ts_slptime > 1) {
        updatepri(td);
        resetpriority(td);
    }
    td->ts_slptime = 0;
    td->ts_slice = sched_slice;

    /* An idle ithread gets its base priority back (td_base_ithread_pri). */
    sched_add(td, SRQ_BORING);
    return cpu_owepreempt[cpu] || cpu_resched[cpu];
}

/* ---- SMP: placing a thread on a CPU ----------------------------------- */

/* Cheapest legal CPU for a thread tied to a cpuset. */
static int sched_pickcpu(sched_thread_t *td) {
    int best, cpu;

    if (td->lastcpu != NOCPU && td->lastcpu < ncpus && THREAD_CAN_SCHED(td, td->lastcpu))
        best = td->lastcpu;
    else
        best = NOCPU;
    for (cpu = 0; cpu < ncpus; cpu++) {
        if (!THREAD_CAN_SCHED(td, cpu))
            continue;
        if (best == NOCPU)
            best = cpu;
        else if (runq_length[cpu] < runq_length[best])
            best = cpu;
    }
    return best == NOCPU ? 0 : best;        /* KASSERT(best != NOCPU) */
}

/* Wake an idle CPU (or the one asked for) to take work. */
static bool forward_wakeup(int cpunum) {
    uint64_t dontuse, map, map2;
    int me = curcpu();

    if (!forward_wakeup_enabled ||
        (forward_wakeup_use_mask == 0 && forward_wakeup_use_loop == 0))
        return false;
    if (!smp_started)
        return false;

    /* Don't bother if we should be doing it ourself. */
    if ((idle_cpus_mask & cpubit(me)) && (cpunum == NOCPU || me == cpunum))
        return false;

    dontuse = cpubit(me);
    map2 = 0;
    if (forward_wakeup_use_loop) {
        for (int id = 0; id < ncpus; id++)
            if (!(dontuse & cpubit(id)) && curthread[id] == NULL)
                map2 |= cpubit(id);
    }

    if (forward_wakeup_use_mask) {
        map = idle_cpus_mask & ~dontuse;
        /* If they are both on, compare and use loop if different. */
        if (forward_wakeup_use_loop && map != map2)
            map = map2;
    } else {
        map = map2;
    }

    /* If we only allow a specific CPU, then mask off all the others. */
    if (cpunum != NOCPU)
        map = (map & cpubit(cpunum)) ? cpubit(cpunum) : 0;

    if (map) {
        uint64_t rest = map;
        for (int id = 0; id < ncpus; id++)
            if ((map & cpubit(id)) && sched_cpu_idle_wakeup(id))
                rest &= ~cpubit(id);
        for (int id = 0; id < ncpus; id++)
            if (rest & cpubit(id))
                sched_ipi(id, SCHED_IPI_AST);
        return true;
    }
    return false;
}

/* Poke another CPU that now has something better to run. */
static void kick_other_cpu(int pri, int cpuid) {
    int cpri;

    if (idle_cpus_mask & cpubit(cpuid)) {
        if (!sched_cpu_idle_wakeup(cpuid))
            sched_ipi(cpuid, SCHED_IPI_AST);
        return;
    }

    cpri = td_prio_of(curthread[cpuid]);
    if (pri >= cpri)
        return;

#ifdef PREEMPTION
#ifndef FULL_PREEMPTION
    if (pri <= PRI_MAX_ITHD)
#endif
    {
        cpu_owepreempt[cpuid] = true;
        sched_ipi(cpuid, SCHED_IPI_PREEMPT);
        return;
    }
#endif
    cpu_resched[cpuid] = true;
    sched_ipi(cpuid, SCHED_IPI_AST);
}

static void sched_add(sched_thread_t *td, int flags) {
    int cpu, cpuid;
    bool forwarded = false, single_cpu = false;

    td->state = TDS_RUNQ;

    /*
     * If SMP is started and the thread is pinned or otherwise limited to a
     * specific set of CPUs, queue it on a per-CPU run queue.  Otherwise on
     * the global one.  Before SMP start per-CPU state may not be set up.
     */
    if (smp_started && (td->pinned != 0 || (td->flags & TDF_BOUND) ||
        (td->ts_flags & TSF_AFFINITY))) {
        if (td->pinned != 0)
            cpu = td->lastcpu;
        else if (td->flags & TDF_BOUND)
            cpu = td->ts_rqcpu;             /* find CPU from bound runq */
        else
            cpu = sched_pickcpu(td);        /* a valid CPU for our cpuset */
        if (cpu < 0 || cpu >= ncpus) cpu = sched_pickcpu(td);
        td->ts_rqcpu = cpu;
        single_cpu = true;
    } else {
        cpu = NOCPU;
        td->ts_rqcpu = NOCPU;
    }

    sched_load_add();
    runq_add(ts_runq_ptr(td), td, flags);
    if (cpu != NOCPU)
        runq_length[cpu]++;

    cpuid = curcpu();
    if (single_cpu && cpu != cpuid) {
        kick_other_cpu(td->priority, cpu);
    } else {
        if (!single_cpu) {
            uint64_t tidlemsk = idle_cpus_mask & ~cpubit(cpuid);

            if (!(idle_cpus_mask & cpubit(cpuid)) &&
                (flags & SRQ_INTR) == 0 && tidlemsk != 0)
                forwarded = forward_wakeup(cpu);
        }
        if (!forwarded) {
            if (!maybe_preempt(td))
                maybe_resched(td);
        }
    }
}

static void sched_rem(sched_thread_t *td) {
    sched_load_rem();
    if (td->ts_rqcpu != NOCPU)
        runq_length[td->ts_rqcpu]--;
    runq_remove(ts_runq_ptr(td), td);
    td->state = TDS_CAN_RUN;
}

/* Select the thread this CPU runs next; NULL means run the idle context. */
static sched_thread_t *sched_choose(void) {
    sched_thread_t *td, *tdcpu = NULL;
    struct runq *rq = &runq_global;
    int cpu = curcpu();

    td = runq_choose_fuzz(&runq_global, runq_fuzz);
    if (smp_started || runq_not_empty(&runq_pcpu[cpu])) {
        tdcpu = runq_choose(&runq_pcpu[cpu]);
        if (td == NULL || (tdcpu != NULL && tdcpu->priority < td->priority)) {
            td = tdcpu;
            rq = &runq_pcpu[cpu];
        }
    }
    if (td) {
        if (rq != &runq_global)
            runq_length[cpu]--;
        runq_remove(rq, td);
        td->flags |= TDF_DIDRUN;
    }
    return td;
}

/* ---- fork and exit -------------------------------------------------- */

bool sched_fork(thread_t *parent_h, thread_t *child_h) {
    sched_thread_t *parent = get_sched_thread(parent_h);
    sched_thread_t *child;

    if (!parent || !child_h || get_sched_thread(child_h)) return false;
    child = add_thread(child_h);
    if (!child) return false;

    /* td_startcopy: what a fork inherits. */
    child->parent = parent_h;
    child->nice = parent->nice;
    child->pri_class = parent->pri_class;
    child->priority = child->base_pri = parent->base_pri;
    child->user_pri = parent->user_pri;
    child->base_user_pri = parent->base_user_pri;
    child->lend_user_pri = parent->lend_user_pri;
    child->cpuset = parent->cpuset;
    child->lastcpu = child->oncpu = NOCPU;
    /* sched_fork_thread() */
    child->ts_estcpu = parent->ts_estcpu;
    child->ts_flags |= (parent->ts_flags & TSF_AFFINITY);
    child->ts_slice = 1;

    sched_add(child, SRQ_BORING);
    return true;
}

void sched_exit(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);

    if (!td) return;

    /* sched_exit_thread(parent, td): charge our cpu usage to the parent. */
    sched_thread_t *p = get_sched_thread(td->parent);
    if (p && p != td)
        p->ts_estcpu = ESTCPULIM(p->ts_estcpu + td->ts_estcpu);

    if (td->state == TDS_RUNQ)
        sched_rem(td);              /* never ran: leaves the run queue */
    else if (td->state == TDS_RUNNING)
        sched_load_rem();           /* it was counted while running */
    for (int c = 0; c < SCHED_MAXCPU; c++)
        if (curthread[c] == td) curthread[c] = NULL;
    remove_thread(td);
}

/* ---- binding, pinning, affinity --------------------------------------- */

void sched_pin(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    if (td) { td->pinned++; if (td->oncpu != NOCPU) td->lastcpu = td->oncpu; }
}

void sched_unpin(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    if (td && td->pinned > 0) td->pinned--;
}

bool sched_bind(thread_t *h, int cpu) {
    sched_thread_t *td = get_sched_thread(h);

    if (!td || cpu < 0 || cpu >= SCHED_MAXCPU) return false;
    td->flags |= TDF_BOUND;
    td->ts_rqcpu = cpu;
    if (curcpu() == cpu)
        return false;
    /* mi_switch(SW_VOL | SWT_BIND): the caller must now reschedule. */
    cpu_resched[curcpu()] = true;
    return true;
}

void sched_unbind(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    if (td) td->flags &= ~TDF_BOUND;
}

bool sched_is_bound(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    return td && (td->flags & TDF_BOUND);
}

void sched_set_cpuset(thread_t *h, uint64_t mask) {
    sched_thread_t *td = get_sched_thread(h);
    int cpu;

    if (!td) return;
    td->cpuset = mask;

    /* Set TSF_AFFINITY if there is at least one CPU this thread can't use. */
    td->ts_flags &= ~TSF_AFFINITY;
    for (cpu = 0; cpu < ncpus; cpu++) {
        if (!THREAD_CAN_SCHED(td, cpu)) {
            td->ts_flags |= TSF_AFFINITY;
            break;
        }
    }
    if (!(td->ts_flags & TSF_AFFINITY))
        return;                         /* can run anywhere: nothing to do */

    /* Pinned and bound threads are left alone. */
    if (td->pinned != 0 || (td->flags & TDF_BOUND))
        return;

    switch (td->state) {
    case TDS_RUNQ:
        /* On a per-CPU run queue that is in the set: nothing to do. */
        if (td->ts_rqcpu != NOCPU && THREAD_CAN_SCHED(td, td->ts_rqcpu))
            return;
        /* Put this thread on a valid per-CPU run queue. */
        sched_rem(td);
        sched_add(td, SRQ_BORING);
        break;
    case TDS_RUNNING:
        /* If our current CPU is in the set fine, else force a switch. */
        if (td->oncpu != NOCPU && THREAD_CAN_SCHED(td, td->oncpu))
            return;
        if (td->oncpu != NOCPU) {
            cpu_resched[td->oncpu] = true;
            if (td->oncpu != curcpu())
                sched_ipi(td->oncpu, SCHED_IPI_AST);
        }
        break;
    default:
        break;
    }
}

/* ---- queries ------------------------------------------------------------ */

int sched_get_priority(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    return td ? td->priority : -1;
}

int sched_get_user_priority(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    return td ? td->user_pri : -1;
}

uint32_t sched_pctcpu(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    return td ? td->ts_pctcpu : 0;
}

uint32_t sched_estcpu(thread_t *h) {
    sched_thread_t *td = get_sched_thread(h);
    return td ? td->ts_estcpu : 0;
}

int sched_load(void) { return sched_tdcnt; }

uint32_t sched_loadavg(int i) { return (i >= 0 && i < 3) ? ldavg[i] : 0; }

int sched_rr_interval(void) {
    /* Convert sched_slice from stathz to hz. */
    return imax(1, (sched_slice * timer_hz + real_stat_hz / 2) / real_stat_hz);
}

bool sched_runnable(void) {
    return runq_not_empty(&runq_global) || runq_not_empty(&runq_pcpu[curcpu()]);
}

bool sched_resched_pending(void) {
    int cpu = curcpu();
    sched_thread_t *td = curthread[cpu];
    return cpu_owepreempt[cpu] || cpu_resched[cpu] ||
           (td && (td->flags & TDF_SLICEEND));
}

/* ---- context switching ---------------------------------------------- */

void sched_set_context_switching(bool on) { ctx_switching = on; }
bool sched_context_switching(void) { return ctx_switching; }

bool sched_set_thread_context(thread_t *h, void *stack_top,
                              void (*entry)(void *), void *arg) {
    if (!h) {
        int cpu = curcpu();
        cpu_thread_setup(&idle_pcb[cpu], (uintptr_t)stack_top, entry, arg);
        idle_ctx_valid[cpu] = true;
        return true;
    }
    sched_thread_t *td = get_sched_thread(h);
    if (!td) return false;
    cpu_thread_setup(&td->pcb, (uintptr_t)stack_top, entry, arg);
    td->ctx_valid = true;
    return true;
}

/* sched_switch() + choosethread() of sched_4bsd.c / kern_switch.c. */
thread_t *sched_switch_prepare(thread_t *old_h, struct sched_switch *sw) {
    int cpu = curcpu();
    sched_thread_t *old = get_sched_thread(old_h);
    sched_thread_t *new;
    bool preempted = false;

    sw->old = NULL;
    sw->new = NULL;
    sw->kind = SW_NONE;

    if (!old_h) {
        /* Leaving the idle context. */
        idle_cpus_mask &= ~cpubit(cpu);
        cpu_owepreempt[cpu] = false;
        cpu_resched[cpu] = false;
    } else if (old) {
        old->lastcpu = old->oncpu;
        preempted = !(old->flags & TDF_SLICEEND) && cpu_owepreempt[cpu];
        old->flags &= ~TDF_SLICEEND;
        cpu_resched[cpu] = false;
        cpu_owepreempt[cpu] = false;
        old->oncpu = NOCPU;

        /*
         * At the last moment, if this thread is still marked RUNNING, put
         * it back on a run queue: it has not been suspended or stopped.  A
         * preempted thread goes to the head of its queue, the rest to the
         * tail.
         */
        if (old->state == TDS_RUNNING)
            sched_add(old, SRQ_OURSELF | SRQ_YIELDING |
                (preempted ? SRQ_PREEMPTED : 0));
        sched_load_rem();
    } else {
        /* The thread has exited (sched_exit_thread() did its load_rem). */
        cpu_resched[cpu] = false;
        cpu_owepreempt[cpu] = false;
    }

    new = sched_choose();
    curthread[cpu] = new;
    if (new) {
        new->state = TDS_RUNNING;
        new->oncpu = cpu;
    } else {
        idle_cpus_mask |= cpubit(cpu);
    }

    if (!ctx_switching)
        return new ? new->handle : NULL;

    /* The context to resume. */
    struct pcb *new_pcb;
    bool new_ok;
    if (!new) {
        new_pcb = &idle_pcb[cpu];
        new_ok = idle_ctx_valid[cpu];
    } else {
        new_pcb = &new->pcb;
        new_ok = new->ctx_valid;
    }
    if (!new_ok) {
        sw->kind = SW_NOCTX;
        return new ? new->handle : NULL;
    }

    /*
     * The context being left. NULL is idle. A non-NULL handle that is no
     * longer in the table has exited: nothing to save, and its stack must
     * not be written to any more -> throw.
     */
    struct pcb *old_pcb;
    int kind = SW_SWITCH;
    if (!old_h)
        old_pcb = &idle_pcb[cpu];
    else if (old)
        old_pcb = &old->pcb;
    else {
        old_pcb = NULL;
        kind = SW_THROW;
    }

    if (old_pcb != new_pcb) {           /* else: same thread chosen again */
        if (kind == SW_SWITCH) {
            /* The save in cpu_switch() makes this context resumable. */
            if (!old_h) idle_ctx_valid[cpu] = true; else old->ctx_valid = true;
        }
        sw->old = old_pcb;
        sw->new = new_pcb;
        sw->kind = kind;
    }
    return new ? new->handle : NULL;
}

void sched_switch_run(const struct sched_switch *sw) {
    if (sw->kind == SW_SWITCH)
        cpu_switch(sw->old, sw->new);
    else if (sw->kind == SW_THROW)
        cpu_throw(sw->new);
}
