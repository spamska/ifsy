/* Freestanding <stdlib.h> for lwext4. lwext4 is built with
 * CONFIG_USE_USER_MALLOC=1, so it only ever calls these four hooks; they are
 * implemented in src/ulibc.rs on top of the kernel's global allocator. */
#ifndef LWEXT4_SHIM_STDLIB_H
#define LWEXT4_SHIM_STDLIB_H
#include <stddef.h>

void *ext4_user_malloc(size_t size);
void *ext4_user_calloc(size_t nmemb, size_t size);
void *ext4_user_realloc(void *ptr, size_t size);
void ext4_user_free(void *ptr);

/* Used for directory index sorting; implemented in src/ulibc.rs. */
void qsort(void *base, size_t nmemb, size_t size,
           int (*compar)(const void *, const void *));

#endif
