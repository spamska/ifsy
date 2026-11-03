/* Plan 9 <u.h> for the Ifsy user-space port, built with gcc/clang (LP64,
 * System V calling convention). Everything linked into one image is compiled
 * the same way, so the Plan 9 compiler's own va_list and 32-bit `long` are
 * not needed. */
#ifndef _IFSY_U_H_
#define _IFSY_U_H_
#include <stdarg.h>
#define nil ((void*)0)
typedef unsigned short ushort;
typedef unsigned char uchar;
typedef unsigned long ulong;
typedef unsigned int uint;
typedef signed char schar;
typedef long long vlong;
typedef unsigned long long uvlong;
typedef long long intptr;
typedef unsigned long long uintptr;
typedef unsigned long long usize;
typedef uint Rune;
typedef unsigned char u8int;
typedef unsigned short u16int;
typedef unsigned int u32int;
typedef unsigned long long u64int;
typedef signed char s8int;
typedef signed short s16int;
typedef signed int s32int;
typedef signed long long s64int;
typedef unsigned int mpdigit;
typedef union FPdbleword FPdbleword;
union FPdbleword {
	double x;
	struct {
		uint lo;
		uint hi;
	};
};
typedef uintptr jmp_buf[32];
#define JMPBUFSP 0
#define JMPBUFPC 1
#define JMPBUFDPC 0
/* Plan 9 compilers know these; here they are plain macros. */
#define USED(x, ...) ((void)(x))
#define SET(x) ((x) = 0)
#endif
