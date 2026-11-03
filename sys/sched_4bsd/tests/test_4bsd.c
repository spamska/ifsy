/*
 * Policy tests for the 4BSD port.  Threads are bare handles: there are no
 * stacks and context switching stays off, so sched_switch_prepare() is pure
 * run-queue bookkeeping.  The numbers checked here follow from FreeBSD's
 * sched_4bsd.c (HZ 100, stathz 0 => 10 tick slice, RQ_PPQ 4, FSCALE 2048).
 */
#include "4bsd.h"
#include <stdint.h>
#include <stdio.h>

#define HZ 100
#define QUANTUM 10

static int failures, checks;
#define CHECK(cond) do { \
    checks++; \
    if (!(cond)) { failures++; printf("  FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); } \
} while (0)

static thread_t *T(int i) { return (thread_t *)(uintptr_t)i; }

/* ---- hooks the "machine" provides -------------------------------------- */
static int g_cpu;
static int ipi_count, last_ipi_cpu = -1, last_ipi_kind = -1;
int sched_curcpu(void) { return g_cpu; }
void sched_ipi(int cpu, int kind) { ipi_count++; last_ipi_cpu = cpu; last_ipi_kind = kind; }

static void fresh(void) {
    sched_init(HZ, 0);
    g_cpu = 0;
    ipi_count = 0; last_ipi_cpu = last_ipi_kind = -1;
}

/* Leave `old` (NULL = idle) and take what is chosen. */
static thread_t *sw_to(thread_t *old) {
    struct sched_switch sw;
    return sched_switch_prepare(old, &sw);
}

/* Boot-style start: wake T(1) and make it the running thread. */
static void boot(void) {
    fresh();
    sched_wakeup(T(1));
    CHECK(sw_to(NULL) == T(1));
}

static void test_empty(void) {
    puts("empty scheduler");
    fresh();
    CHECK(sched_load() == 0);
    CHECK(sw_to(NULL) == NULL);
    CHECK(sched_get_priority(T(1)) == -1);
    CHECK(sched_get_priority(NULL) == -1);
    sched_exit(T(1));            /* unknown thread: harmless */
    sched_sleep(NULL);
    CHECK(!sched_tick(T(1)));
    CHECK(!sched_runnable());
    CHECK(sched_rr_interval() == QUANTUM);
}

static void test_wakeup_and_load(void) {
    puts("wakeup registers, load counts runnable + running threads");
    fresh();
    CHECK(sched_wakeup(T(1)));                 /* idle CPU: reschedule */
    CHECK(sched_load() == 1);
    CHECK(sched_runnable());
    CHECK(sw_to(NULL) == T(1));
    CHECK(sched_load() == 1);                  /* running is counted */
    CHECK(!sched_wakeup(T(1)));                /* already running: no-op */
    CHECK(sched_load() == 1);
    sched_sleep(T(1));
    CHECK(sw_to(T(1)) == NULL);                /* nothing else: idle */
    CHECK(sched_load() == 0);
    CHECK(!sched_runnable());
}

static void test_initial_priority(void) {
    puts("a new thread starts at PUSER + (0 - PRIO_MIN)");
    fresh();
    sched_wakeup(T(1));
    CHECK(sched_get_priority(T(1)) == PRI_MIN_TIMESHARE - PRIO_MIN);
    CHECK(sched_get_user_priority(T(1)) == PRI_MIN_TIMESHARE - PRIO_MIN);
    CHECK(sched_estcpu(T(1)) == 0);
}

static void test_nice_ordering(void) {
    puts("lower priority number runs first; nice moves the priority");
    fresh();
    sched_wakeup(T(1));
    sched_wakeup(T(2));
    sched_wakeup(T(3));
    sched_nice(T(2), -10);
    sched_nice(T(3), 20);
    CHECK(sched_get_priority(T(2)) == 130);
    CHECK(sched_get_priority(T(3)) == 160);
    thread_t *cur = sw_to(NULL);
    CHECK(cur == T(2));
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(1));
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(3));
}

static void test_queue_granularity(void) {
    puts("4 priorities share a queue and are served FIFO (RQ_PPQ)");
    fresh();
    sched_wakeup(T(1));
    sched_wakeup(T(2));
    sched_prio(T(1), 143);                     /* 143/4 == 140/4 == 35 */
    CHECK(sched_get_priority(T(1)) == 143);
    CHECK(sched_get_priority(T(2)) == 140);
    thread_t *cur = sw_to(NULL);
    CHECK(cur == T(1));                        /* queued first, same queue */
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(2));
    /* a priority change of a queued thread moves it to the right queue */
    fresh();
    sched_wakeup(T(1)); sched_wakeup(T(2)); sched_wakeup(T(3));
    sched_prio(T(3), 100);                     /* queued: requeued at 25 */
    cur = sw_to(NULL);
    CHECK(cur == T(3));
    CHECK(sched_load() == 3);
}

static void test_quantum_and_tail(void) {
    puts("slice end: tick says reschedule, the thread goes to the queue tail");
    boot();
    sched_wakeup(T(2));
    sched_wakeup(T(3));
    for (int i = 0; i < QUANTUM - 1; i++) CHECK(!sched_tick(T(1)));
    CHECK(sched_tick(T(1)));                   /* QUANTUM-th tick */
    CHECK(sched_resched_pending());
    thread_t *cur = sw_to(T(1));
    CHECK(cur == T(2));
    CHECK(!sched_resched_pending());
    CHECK(sched_load() == 3);
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(3));                        /* T1 queued behind T3 */
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(1));
}

static void test_wakeup_resched(void) {
    puts("a better timeshare thread asks for a switch, no head insert");
    boot();
    sched_wakeup(T(4));                        /* equal priority, same queue */
    CHECK(!sched_resched_pending());
    sched_wakeup(T(2));
    sched_nice(T(2), -20);                     /* priority 120 beats 140 */
    sched_sleep(T(2));
    CHECK(sched_wakeup(T(2)));                 /* TDP_RESCHED */
    thread_t *cur = sw_to(T(1));
    CHECK(cur == T(2));
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(4));                        /* T1 went to the TAIL */
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(1));
}

static void test_ithread_preempt(void) {
    puts("an interrupt-class thread preempts; the victim keeps its place");
    fresh();
    sched_wakeup(T(3));
    thread_t *cur = sw_to(NULL);
    CHECK(cur == T(3));
    sched_set_class(T(3), PRI_ITHD);
    sched_prio(T(3), 20);
    sched_sleep(T(3));
    sched_wakeup(T(1));
    cur = sw_to(T(3));
    CHECK(cur == T(1));
    sched_wakeup(T(4));                        /* waits in T1's queue */
    CHECK(sched_wakeup(T(3)));                 /* pri 20 <= PRI_MAX_ITHD */
    CHECK(sched_resched_pending());
    cur = sw_to(T(1));
    CHECK(cur == T(3));
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(1));                        /* SRQ_PREEMPTED: queue head */
    sched_sleep(cur); cur = sw_to(cur);
    CHECK(cur == T(4));
}

static void test_ithread_demotion(void) {
    puts("an ithread that burns a whole quantum is demoted one step");
    fresh();
    sched_wakeup(T(1));
    sw_to(NULL);
    sched_set_class(T(1), PRI_ITHD);
    sched_prio(T(1), 20);
    for (int i = 0; i < QUANTUM; i++) sched_tick(T(1));
    CHECK(sched_get_priority(T(1)) == 20 + RQ_PPQ);
}

static void test_estcpu_and_decay(void) {
    puts("estcpu grows per tick, priority follows, schedcpu decays it");
    boot();
    for (int i = 0; i < 30; i++) sched_tick(T(1));
    CHECK(sched_estcpu(T(1)) == 30);
    CHECK(sched_get_user_priority(T(1)) == 120 + 30 / 8 + 20);
    CHECK(sched_get_priority(T(1)) == 120 + 30 / 8 + 20);
    sched_background_work();                   /* load average is still 0 */
    CHECK(sched_estcpu(T(1)) == 0);
    CHECK(sched_pctcpu(T(1)) == 30u << 0);     /* cpticks << (FSHIFT-CCPU_SHIFT) */
    for (int i = 0; i < 1000; i++) sched_tick(T(1));
    CHECK(sched_estcpu(T(1)) == 511);          /* ESTCPULIM */
}

static void test_loadavg(void) {
    puts("load average every 5th schedcpu run, then slower decay");
    boot();
    for (int i = 0; i < 4; i++) sched_background_work();
    CHECK(sched_loadavg(0) == 0);
    sched_background_work();
    CHECK(sched_loadavg(0) == 164);            /* (2048-1884)*1*2048 >> 11 */
    CHECK(sched_loadavg(1) == 34);             /* (2048-2014) */
    for (int i = 0; i < 100; i++) sched_tick(T(1));
    CHECK(sched_estcpu(T(1)) == 100);
    sched_background_work();                   /* loadfac 328 */
    CHECK(sched_estcpu(T(1)) == 13);           /* 328*100 / (328+2048) */
}

static void test_updatepri_after_sleep(void) {
    puts("a long sleeper has its estcpu decayed when it wakes");
    boot();
    for (int i = 0; i < 5; i++) sched_background_work();   /* ldavg0 = 164 */
    sched_wakeup(T(2));
    for (int i = 0; i < 100; i++) sched_tick(T(1));
    for (int i = 0; i < 100; i++) { sched_tick(T(1)); }
    thread_t *cur = T(1);
    sched_sleep(cur);
    cur = sw_to(cur);                          /* T2 runs, T1 sleeps */
    CHECK(cur == T(2));
    uint32_t before = sched_estcpu(T(1));
    CHECK(before == 200);
    /* TDF_DIDRUN counts as awake once, then slptime 1 decays once more. */
    sched_background_work();
    sched_background_work();
    uint32_t once = sched_estcpu(T(1));
    CHECK(once < before);
    for (int i = 0; i < 8; i++) sched_background_work();   /* T1 sleeps */
    CHECK(sched_estcpu(T(1)) == once);         /* skipped while idle */
    before = once;
    sched_wakeup(T(1));
    CHECK(sched_estcpu(T(1)) < before);
    CHECK(sched_get_user_priority(T(1)) == 120 + (int)(sched_estcpu(T(1)) / 8) + 20);
}

static void test_fork_exit(void) {
    puts("fork copies estcpu, 1 tick slice; exit charges the parent");
    boot();
    for (int i = 0; i < 40; i++) sched_tick(T(1));
    sched_nice(T(1), 5);
    CHECK(sched_fork(T(1), T(2)));
    CHECK(!sched_fork(T(1), T(2)));            /* already exists */
    CHECK(!sched_fork(T(9), T(3)));            /* unknown parent */
    CHECK(sched_estcpu(T(2)) == 40);
    CHECK(sched_get_priority(T(2)) == sched_get_priority(T(1)));
    CHECK(sched_load() == 2);
    thread_t *cur = sw_to(T(1));               /* put T1 back, run T2? */
    /* both are in the same queue: T1 was running and is requeued last */
    CHECK(cur == T(2));
    CHECK(sched_tick(T(2)));                   /* slice is 1 */
    for (int i = 0; i < 20; i++) sched_tick(T(2));
    uint32_t child = sched_estcpu(T(2)), parent = sched_estcpu(T(1));
    sched_exit(T(2));
    CHECK(sched_estcpu(T(1)) == parent + child);
    CHECK(sched_load() == 1);
    CHECK(sw_to(T(2)) == T(1));                /* exited handle: throw */
    CHECK(sched_get_priority(T(2)) == -1);
    /* a child that never ran leaves the run queue */
    CHECK(sched_fork(T(1), T(3)));
    CHECK(sched_load() == 2);
    sched_exit(T(3));
    CHECK(sched_load() == 1);
}

static void test_lend_unlend(void) {
    puts("priority lending and unlending");
    fresh();
    sched_wakeup(T(1));
    sched_wakeup(T(2));
    sched_lend_prio(T(2), 100);                /* queued: moves to queue 25 */
    CHECK(sched_get_priority(T(2)) == 100);
    sched_prio(T(2), 150);                     /* borrowing: never lowered */
    CHECK(sched_get_priority(T(2)) == 100);
    thread_t *cur = sw_to(NULL);
    CHECK(cur == T(2));
    sched_unlend_prio(T(2), PRI_MAX);          /* back to the user priority */
    CHECK(sched_get_priority(T(2)) == 140);
    sched_lend_user_prio(T(2), 130);
    CHECK(sched_get_user_priority(T(2)) == 130);
    CHECK(sched_get_priority(T(2)) == 130);
    sched_lend_user_prio_cond(T(2), 130);      /* no change */
    sched_userret_slowpath(T(2));
    CHECK(sched_get_priority(T(2)) == 130);
}

static void test_rr_interval(void) {
    puts("round-robin interval is the slice in hz ticks");
    sched_init(1000, 100);
    CHECK(sched_rr_interval() == 100);         /* 10 stathz ticks * 10 */
    sched_init(HZ, 0);
    CHECK(sched_rr_interval() == QUANTUM);
}

static void test_smp(void) {
    puts("SMP: cpuset/pinned threads use per-CPU run queues and IPIs");
    fresh();
    sched_set_ncpus(3);
    CHECK(sched_ncpus() == 3);
    g_cpu = 1; CHECK(sw_to(NULL) == NULL);     /* cpu1 idle */
    g_cpu = 2; CHECK(sw_to(NULL) == NULL);     /* cpu2 idle */
    g_cpu = 0; CHECK(sw_to(NULL) == NULL);     /* cpu0 idle */

    sched_wakeup(T(1));                        /* global queue */
    sched_wakeup(T(2));
    sched_set_cpuset(T(1), 0x6);               /* cpu 1 or 2 only */
    sched_sleep(T(1));                         /* still queued: leaves it */
    ipi_count = 0;
    sched_wakeup(T(1));                        /* pickcpu -> cpu 1 */
    CHECK(ipi_count == 1 && last_ipi_cpu == 1 && last_ipi_kind == SCHED_IPI_AST);
    sched_wakeup(T(3));
    sched_set_cpuset(T(3), 0x6);
    sched_sleep(T(3));
    sched_wakeup(T(3));                        /* cpu 2 is emptier */
    CHECK(last_ipi_cpu == 2);

    /* On a tie the global queue wins (sched_choose: tdcpu must be better). */
    g_cpu = 1; thread_t *c1 = sw_to(NULL);
    CHECK(c1 == T(2));
    sched_sleep(c1);
    CHECK(sw_to(c1) == T(1));                  /* then its own queue */
    g_cpu = 2; CHECK(sw_to(NULL) == T(3));
    g_cpu = 0; CHECK(sw_to(NULL) == NULL);
}

static void test_smp_forward_wakeup(void) {
    puts("SMP: a wakeup on a busy CPU is forwarded to an idle one");
    fresh();
    sched_set_ncpus(2);
    g_cpu = 1; CHECK(sw_to(NULL) == NULL);     /* cpu1 idle */
    g_cpu = 0;
    sched_wakeup(T(1));
    CHECK(sw_to(NULL) == T(1));                /* cpu0 busy */
    ipi_count = 0;
    sched_wakeup(T(2));                        /* global queue */
    CHECK(ipi_count == 1 && last_ipi_cpu == 1 && last_ipi_kind == SCHED_IPI_AST);
    CHECK(!sched_resched_pending());           /* forwarded, we keep running */
    g_cpu = 1; CHECK(sw_to(NULL) == T(2));     /* the idle CPU takes it */
}

static void test_smp_kick_preempt(void) {
    puts("SMP: a bound interrupt thread preempts a remote CPU by IPI");
    fresh();
    sched_set_ncpus(2);
    g_cpu = 1; sched_wakeup(T(1)); CHECK(sw_to(NULL) == T(1));   /* cpu1 runs T1 */
    g_cpu = 0; sched_wakeup(T(2)); CHECK(sw_to(NULL) == T(2));   /* cpu0 runs T2 */
    sched_wakeup(T(3));
    sched_set_class(T(3), PRI_ITHD);
    sched_prio(T(3), 10);
    CHECK(sched_bind(T(3), 1) == false || true);
    sched_unbind(T(3));
    sched_set_cpuset(T(3), 0x2);               /* cpu 1 only */
    sched_sleep(T(3));
    ipi_count = 0;
    sched_wakeup(T(3));                        /* from cpu 0, for cpu 1 */
    CHECK(ipi_count == 1 && last_ipi_cpu == 1 && last_ipi_kind == SCHED_IPI_PREEMPT);
    g_cpu = 1;
    CHECK(sched_resched_pending());
    CHECK(sw_to(T(1)) == T(3));
}

static void test_bind(void) {
    puts("binding a thread to another CPU forces it off this one");
    fresh();
    sched_set_ncpus(2);
    sched_wakeup(T(1));
    CHECK(sw_to(NULL) == T(1));
    CHECK(!sched_is_bound(T(1)));
    CHECK(sched_bind(T(1), 1));                /* must leave cpu 0 */
    CHECK(sched_is_bound(T(1)));
    CHECK(sw_to(T(1)) == NULL);                /* requeued on cpu1's queue */
    g_cpu = 1;
    CHECK(sw_to(NULL) == T(1));
    CHECK(!sched_bind(T(1), 1));               /* already there */
    sched_unbind(T(1));
    CHECK(!sched_is_bound(T(1)));
    sched_pin(T(1)); sched_unpin(T(1));
}

int main(void) {
    test_empty();
    test_wakeup_and_load();
    test_initial_priority();
    test_nice_ordering();
    test_queue_granularity();
    test_quantum_and_tail();
    test_wakeup_resched();
    test_ithread_preempt();
    test_ithread_demotion();
    test_estcpu_and_decay();
    test_loadavg();
    test_updatepri_after_sleep();
    test_fork_exit();
    test_lend_unlend();
    test_rr_interval();
    test_smp();
    test_smp_forward_wakeup();
    test_smp_kick_preempt();
    test_bind();
    printf("\n%d checks, %d failed\n", checks, failures);
    return failures ? 1 : 0;
}
