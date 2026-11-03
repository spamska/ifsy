/* Freestanding <string.h> for lwext4. The functions are provided by
 * compiler_builtins (mem*) and by src/ulibc.rs (str*). */
#ifndef LWEXT4_SHIM_STRING_H
#define LWEXT4_SHIM_STRING_H
#include <stddef.h>

void *memcpy(void *restrict dst, const void *restrict src, size_t n);
void *memmove(void *dst, const void *src, size_t n);
void *memset(void *dst, int c, size_t n);
int memcmp(const void *a, const void *b, size_t n);

size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
int strncmp(const char *a, const char *b, size_t n);
char *strcpy(char *restrict dst, const char *restrict src);
char *strncpy(char *restrict dst, const char *restrict src, size_t n);

#endif
