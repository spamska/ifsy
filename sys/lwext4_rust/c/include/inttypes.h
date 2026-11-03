/* Freestanding <inttypes.h> for lwext4 (LP64 targets only). */
#ifndef LWEXT4_SHIM_INTTYPES_H
#define LWEXT4_SHIM_INTTYPES_H
#include <stdint.h>
#define PRId64 "ld"
#define PRIu64 "lu"
#define PRIx64 "lx"
#define PRIX64 "lX"
#endif
