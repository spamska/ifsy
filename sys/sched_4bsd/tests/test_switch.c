/*
 * Real context-switch test for the machine-dependent half (cpu_switch,
 * cpu_throw, fork_trampoline, cpu_thread_setup).  Runs on the host when the
 * host architecture has a port (x86_64, aarch64, riscv64): threads get
 * malloc'd stacks and really switch between each other.
 */
#include "4bsd.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static int checks, failures;
#define CHECK(c) do { checks++; if (!(c)) { failures++; \
    printf("  FAIL %s:%d: %s\n", __FILE__, __LINE__, #c); } } while (0)

#define T(n) ((thread_t *)(uintptr_t)(n))
#define STACK_SIZE (64 * 1024)
#define ROUNDS 5

static thread_t *cur;                 /* who is running (NULL = idle) */
static int order[64], order_len;
static int sums_ok, exited, idle_runs;

/* Give up the CPU to the next runnable thread (like sched.rs's yield_now). */
static void yield_now(void) {
    thread_t *me = cur;
    struct sched_switch sw;
    /* A running thread that relinquishes goes to the tail of its queue. */
    cur = sched_switch_prepare(me, &sw);
    sched_switch_run(&sw);
    /* back here when someone switches to us again */
}

static void __attribute__((noinline)) work(uint64_t *a, uint64_t *b, uint64_t *c, uint64_t id, uint32_t r) {
    *a = *a * 31 + r;
    *b ^= *a << 3;
    uint64_t x = *c;
    *c = ((x << 7) | (x >> 57)) ^ *b;
    (void)id;
}

static void worker(void *arg) {
    int id = (int)(intptr_t)arg;
    uint64_t a = id, b = 0x1111u * id, c = 3, d = 0;
    double x = 1.5 + id;
    for (uint32_t r = 0; r < ROUNDS; r++) {
        order[order_len++] = id;
        work(&a, &b, &c, id, r);
        d += c;
        x = x * 1.0625 + 0.5;
        yield_now();
    }
    uint64_t ea = id, eb = 0x1111u * id, ec = 3, ed = 0;
    double ex = 1.5 + id;
    for (uint32_t r = 0; r < ROUNDS; r++) {
        work(&ea, &eb, &ec, id, r);
        ed += ec;
        ex = ex * 1.0625 + 0.5;
    }
    if (a == ea && b == eb && c == ec && d == ed && x == ex) sums_ok++;
    exited++;
    /* exit: leave the table, then never come back (cpu_throw) */
    thread_t *me = cur;
    struct sched_switch sw;
    sched_exit(me);
    cur = sched_switch_prepare(me, &sw);
    sched_switch_run(&sw);
    puts("  FAIL: exited thread resumed");
    abort();
}

static void idle_main(void *arg) {
    (void)arg;
    for (;;) {
        idle_runs++;
        struct sched_switch sw;
        cur = sched_switch_prepare(NULL, &sw);
        sched_switch_run(&sw);
    }
}

static void *new_stack(void) {
    char *p = malloc(STACK_SIZE);
    if (!p) abort();
    return p + STACK_SIZE;
}

static void test_round_robin_and_exit(void) {
    puts("threads yield round-robin, keep registers, exit with cpu_throw");
    sched_init(100, 0);
    /* The boot thread is the one already running: choose it before switching
     * is on (it has no context of its own until it is first switched out). */
    sched_wakeup(T(1));
    struct sched_switch sw0;
    cur = sched_switch_prepare(NULL, &sw0);
    CHECK(cur == T(1));
    sched_set_context_switching(true);
    CHECK(sched_set_thread_context(NULL, new_stack(), idle_main, NULL));

    for (int id = 10; id <= 12; id++) {
        CHECK(sched_fork(T(1), T(id)));
        CHECK(sched_set_thread_context(T(id), new_stack(), worker, (void *)(intptr_t)id));
    }
    while (exited < 3 && order_len < 60) yield_now();
    CHECK(order_len == 3 * ROUNDS);
    int cnt[3] = {0, 0, 0}, adj = 0;
    for (int i = 0; i < order_len; i++) {
        cnt[order[i] - 10]++;
        if (i && order[i] == order[i - 1]) adj++;
    }
    CHECK(cnt[0] == ROUNDS && cnt[1] == ROUNDS && cnt[2] == ROUNDS);
    CHECK(adj == 0);
    CHECK(sums_ok == 3);
    for (int i = 0; i < 20; i++) yield_now();   /* let the last ones exit */
    CHECK(sched_load() == 1);                     /* only us, running */
    CHECK(cur == T(1));
}

static void test_idle_context(void) {
    puts("blocking with nothing else runnable switches to the idle context");
    thread_t *me = cur;
    struct sched_switch sw;
    sched_sleep(me);
    thread_t *next = sched_switch_prepare(me, &sw);
    CHECK(next == NULL);
    CHECK(sw.kind == SW_SWITCH);
    cur = next;
    int before = idle_runs;
    /* A wakeup arrives before the register switch happens; the idle context
     * then picks us up again. */
    sched_wakeup(me);
    sched_switch_run(&sw);
    CHECK(idle_runs == before + 1);
    CHECK(cur == me);
}

static void test_no_context_is_refused(void) {
    puts("a thread without a context is never switched to");
    struct sched_switch sw;
    sched_wakeup(T(50));                 /* registered, no context */
    thread_t *next = sched_switch_prepare(T(1), &sw);
    CHECK(next == T(50));
    CHECK(sw.kind == SW_NOCTX);
    sched_exit(T(50));
    /* get T1 (requeued by the prepare above) back, bookkeeping only */
    sched_set_context_switching(false);
    cur = sched_switch_prepare(T(50), &sw);
    CHECK(cur == T(1));
    sched_set_context_switching(true);
}

int main(void) {
    test_round_robin_and_exit();
    test_idle_context();
    test_no_context_is_refused();
    printf("\n%d checks, %d failed\n", checks, failures);
    return failures ? 1 : 0;
}
