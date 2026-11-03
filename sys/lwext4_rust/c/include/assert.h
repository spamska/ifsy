/* Freestanding <assert.h> for lwext4: a failed assertion is fatal and is
 * reported through the Rust side (src/ulibc.rs), which panics. */
#ifndef LWEXT4_SHIM_ASSERT_H
#define LWEXT4_SHIM_ASSERT_H

__attribute__((noreturn)) void ext4_rs_assert_fail(const char *file, int line, const char *expr);

#define assert(e) ((e) ? (void)0 : ext4_rs_assert_fail(__FILE__, __LINE__, #e))

#endif
