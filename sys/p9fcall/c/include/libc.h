/*
 * Copyright (c) 2026 spamska and contributors
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
 * FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
 * COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
 * BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 * LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
 * AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
 * USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 * NOTE: This file is licensed under the BSD 2-Clause license. 
 * This does not imply that all components or parts of the Ifsy 
 * operating system are distributed under the same terms. Please consult 
 * the individual components or the root license documentation for details.
 */

#ifndef P9FCALL_LIBC_H
#define P9FCALL_LIBC_H

#include <u.h>

#define nelem(x) (sizeof(x)/sizeof((x)[0]))

extern void* memmove(void*, const void*, unsigned long);
extern unsigned long strlen(const char*);

typedef struct Fmt Fmt;

typedef
struct Qid
{
	uvlong	path;
	ulong	vers;
	uchar	type;
} Qid;

typedef
struct Dir {
	ushort	type;
	uint	dev;
	Qid	qid;
	ulong	mode;
	ulong	atime;
	ulong	mtime;
	vlong	length;
	char	*name;
	char	*uid;
	char	*gid;
	char	*muid;
} Dir;

#define	STATMAX	65535U
#define	DIRMAX	(sizeof(Dir)+STATMAX)
#define	ERRMAX	128

#define	MORDER	0x0003
#define	MREPL	0x0000
#define	MBEFORE	0x0001
#define	MAFTER	0x0002
#define	MCREATE	0x0004
#define	MCACHE	0x0010
#define	MMASK	0x0017

#define	OREAD	0
#define	OWRITE	1
#define	ORDWR	2
#define	OEXEC	3
#define	OTRUNC	16
#define	OCEXEC	32
#define	ORCLOSE	64
#define	OEXCL	0x1000

#define QTDIR		0x80
#define QTAPPEND	0x40
#define QTEXCL		0x20
#define QTMOUNT		0x10
#define QTAUTH		0x08
#define QTTMP		0x04
#define QTFILE		0x00

#define DMDIR		0x80000000
#define DMAPPEND	0x40000000
#define DMEXCL		0x20000000
#define DMMOUNT		0x10000000
#define DMAUTH		0x08000000
#define DMTMP		0x04000000
#define DMREAD		0x4
#define DMWRITE		0x2
#define DMEXEC		0x1

#endif
