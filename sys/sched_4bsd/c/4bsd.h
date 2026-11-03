#ifndef _4BSD_SCHED_H_
#define _4BSD_SCHED_H_

/*
 * The FreeBSD 4BSD scheduler (sys/kern/sched_4bsd.c + sys/kern/kern_switch.c
 * run queues), cut down to what Ifsy needs.
 *
 * Everything in here is a straight port: the same priority bands, the same
 * 64 run queues of 4 priorities each, the same estcpu / nice / load-average
 * arithmetic, the same schedcpu() once a second, the same preemption rules,
 * the same SMP policy (global run queue + per-CPU run queues for pinned and
 * bound threads, pickcpu, forward_wakeup, kick_other_cpu).
 *
 * What was dropped: KTR/SDT/DTrace/hwpmc/hwt hooks, sysctls, turnstiles
 * (lend/unlend are kept, the turnstile hook is not), the idle *thread*
 * (a NULL handle is the CPU's idle context), TDF_NOLOAD, swapping, cpusets
 * (a 64-bit mask per thread is kept) and the per-CPU idle statistics.
 *
 * Locking: like FreeBSD's global sched_lock, every function below must be
 * called with the caller's scheduler lock held and interrupts masked.
 */

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

typedef struct thread thread_t;     /* opaque handle; NULL = idle context */
struct pcb;

#define SCHED_MAXCPU   64

/* ---- sys/priority.h ---------------------------------------------------- */
#define PRI_MIN            0
#define PRI_MAX            255
#define PRI_MIN_ITHD       PRI_MIN
#define PRI_MAX_ITHD       47
#define PRI_MIN_REALTIME   48
#define PRI_MAX_REALTIME   79
#define PRI_MIN_KERN       80
#define PRI_MAX_KERN       119
#define PRI_MIN_TIMESHARE  120
#define PRI_MAX_TIMESHARE  223
#define PRI_MIN_IDLE       224
#define PRI_MAX_IDLE       PRI_MAX
#define PUSER              PRI_MIN_TIMESHARE

/* Scheduling classes (td_pri_class). */
#define PRI_ITHD           1
#define PRI_REALTIME       2
#define PRI_TIMESHARE      3
#define PRI_IDLE           4
#define PRI_FIFO_BIT       8
#define PRI_FIFO           (PRI_FIFO_BIT | PRI_REALTIME)
#define PRI_BASE(P)        ((P) & ~PRI_FIFO_BIT)

#define PRIO_MIN           (-20)
#define PRIO_MAX           20

/* ---- sys/runq.h -------------------------------------------------------- */
#define RQ_NQS             64
#define RQ_PPQ             4
#define RQ_PRI_TO_QUEUE_IDX(pri)  ((pri) / RQ_PPQ)

/* sched_add() flags (sys/sched.h). */
#define SRQ_BORING         0x0000
#define SRQ_YIELDING       0x0001
#define SRQ_OURSELF        0x0002
#define SRQ_INTR           0x0004
#define SRQ_PREEMPTED      0x0008

/* Thread flags. */
#define TDF_DIDRUN         0x0001   /* thread actually ran                  */
#define TDF_BOUND          0x0002   /* bound to one CPU                     */
#define TDF_SLICEEND       0x0004   /* time slice is over                   */
#define TDF_BORROWING      0x0008   /* priority lent by sched_lend_prio()   */

/* IPI kinds handed to sched_ipi(). */
#define SCHED_IPI_AST      0
#define SCHED_IPI_PREEMPT  1

/* ---- set-up ------------------------------------------------------------ */

/* sched_setup() + sched_initticks().  Resets every thread and run queue. */
void sched_init(int hz, int stathz);
/*
 * SMP: `n` CPUs are running and may be scheduled on (smp_started).  With
 * n <= 1 everything uses the global run queue, exactly as before SMP start.
 */
void sched_set_ncpus(int n);
int  sched_ncpus(void);

/*
 * Machine-dependent hooks; weak defaults for a single CPU live in 4bsd.c.
 * sched_curcpu(): PCPU_GET(cpuid).  sched_ipi(): ipi_cpu().
 * sched_cpu_idle_wakeup(): cpu_idle_wakeup(), true if it woke the CPU
 * without an IPI.
 */
int  sched_curcpu(void);
void sched_ipi(int cpu, int kind);
bool sched_cpu_idle_wakeup(int cpu);

/* ---- thread life ------------------------------------------------------- */

/* sched_fork(): `child` starts runnable (sched_add(SRQ_BORING)). */
bool sched_fork(thread_t *parent, thread_t *child);
/* sched_exit(): charge the child's estcpu to its parent and drop it. */
void sched_exit(thread_t *td);

/* ---- the clock --------------------------------------------------------- */

/*
 * sched_clock() for one tick of `td` (the thread running on this CPU).
 * Returns true if this CPU should now reschedule (slice end, preemption
 * owed, ...).
 */
bool sched_tick(thread_t *td);
/* schedcpu() (call once a second) and, every 5th call, the load average. */
void sched_background_work(void);

/* ---- state changes ----------------------------------------------------- */

/* sched_wakeup(): make a sleeping (or not yet known) thread runnable.
 * Returns true if this CPU should reschedule. */
bool sched_wakeup(thread_t *td);
/* sched_sleep() + TD_SET_SLEEPING(): `td` (running) is about to block. */
void sched_sleep(thread_t *td);

void sched_nice(thread_t *td, int nice);
void sched_set_class(thread_t *td, int class_);
void sched_prio(thread_t *td, int prio);
void sched_user_prio(thread_t *td, int prio);
void sched_lend_prio(thread_t *td, int prio);
void sched_unlend_prio(thread_t *td, int prio);
void sched_lend_user_prio(thread_t *td, int prio);
void sched_lend_user_prio_cond(thread_t *td, int prio);
void sched_userret_slowpath(thread_t *td);

void sched_pin(thread_t *td);
void sched_unpin(thread_t *td);
/* sched_bind(): true if `td` (current) now has to leave this CPU. */
bool sched_bind(thread_t *td, int cpu);
void sched_unbind(thread_t *td);
bool sched_is_bound(thread_t *td);
/* The set of CPUs `td` may run on changed (td_cpuset). */
void sched_set_cpuset(thread_t *td, uint64_t mask);

/* ---- queries ----------------------------------------------------------- */

int      sched_get_priority(thread_t *td);       /* td_priority, -1 unknown */
int      sched_get_user_priority(thread_t *td);  /* td_user_pri, -1 unknown */
uint32_t sched_pctcpu(thread_t *td);             /* fixpt_t, FSHIFT = 11    */
uint32_t sched_estcpu(thread_t *td);
int      sched_load(void);                       /* sched_tdcnt             */
uint32_t sched_loadavg(int i);                   /* averunnable.ldavg[i]    */
int      sched_rr_interval(void);                /* in hz ticks             */
bool     sched_runnable(void);                   /* work for this CPU?      */
bool     sched_resched_pending(void);            /* this CPU owes a switch  */

/*
 * ---- context switching -------------------------------------------------
 *
 * A NULL thread handle stands for the CPU's idle context, which has a PCB of
 * its own per CPU (the scheduler never queues it).
 *
 * Context switching is OFF after sched_init(): sched_switch_prepare() then
 * only does the bookkeeping, which is what a caller that drives "threads"
 * without stacks (a simulation, the C unit tests) wants.  Turn it on with
 * sched_set_context_switching(true) once every thread that can be chosen
 * has a context (sched_set_thread_context(), or having been switched out
 * once).
 */

/* What sched_switch_prepare() decided. */
enum {
    SW_NONE    = 0,   /* nothing to do: same context, or switching is off  */
    SW_SWITCH  = 1,   /* save into ->old, resume ->new                     */
    SW_THROW   = 2,   /* ->old has exited: resume ->new, save nothing      */
    SW_NOCTX   = -1,  /* ->new has no context and cannot run: do NOT switch */
};

struct sched_switch {
    struct pcb *old;
    struct pcb *new;
    int         kind;
};

void sched_set_context_switching(bool on);
bool sched_context_switching(void);

/*
 * Give `td` (NULL = this CPU's idle context) a fresh context that starts in
 * entry(arg) on the stack whose top is `stack_top`.  entry must never
 * return.  Returns false if `td` is not a known thread.
 */
bool sched_set_thread_context(thread_t *td, void *stack_top,
                              void (*entry)(void *), void *arg);

/*
 * Two halves of mi_switch(), so a caller can keep its own lock out of the
 * switch itself.
 *
 * prepare() is sched_switch() + choosethread(): `old` (NULL = idle, a handle
 * that was sched_exit()ed = a thread that is gone) leaves this CPU; if it is
 * still running it goes back on a run queue, then the best thread is taken
 * off the queues and becomes this CPU's current thread.  Returns that thread
 * (NULL = idle) and fills *sw.  Call it under the scheduler lock.
 *
 * run() performs the register/stack switch (interrupts masked, lock NOT
 * held); it returns, in the old thread, only when something switches back.
 */
thread_t *sched_switch_prepare(thread_t *old, struct sched_switch *sw);
void sched_switch_run(const struct sched_switch *sw);

/* Machine-dependent, arch/<arch>/{switch.S,machdep.c}. */
void cpu_switch(struct pcb *old, struct pcb *new);
void cpu_throw(struct pcb *new);
void cpu_thread_setup(struct pcb *pcb, uintptr_t stack_top,
                      void (*entry)(void *), void *arg);

#endif
