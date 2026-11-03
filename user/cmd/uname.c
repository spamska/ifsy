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

#include <u.h>
#include <libc.h>
#include <ifsy.h>

static void
usage(void)
{
	fprint(2, "usage: uname [-asnrvmo]\n");
	exits("usage");
}

void
main(int argc, char **argv)
{
	int s, n, r, v, m, o, first;
	char sys[64], rel[64], ver[64], mach[64];

	s = n = r = v = m = o = 0;
	ARGBEGIN{
	case 'a':
		s = n = r = v = m = o = 1;
		break;
	case 's':
		s = 1;
		break;
	case 'n':
		n = 1;
		break;
	case 'r':
		r = 1;
		break;
	case 'v':
		v = 1;
		break;
	case 'm':
	case 'p':
	case 'i':
		m = 1;
		break;
	case 'o':
		o = 1;
		break;
	default:
		usage();
	}ARGEND
	if(argc != 0)
		usage();
	if(!(s || n || r || v || m || o))
		s = 1;

	osinfo(sys, rel, ver, mach, sizeof sys);
	first = 1;
#define OUT(flag, str) if(flag){ print("%s%s", first ? "" : " ", str); first = 0; }
	OUT(s, sys);
	OUT(n, sysname());
	OUT(r, rel);
	OUT(v, ver);
	OUT(m, mach);
	OUT(o, "Ifsy");
	print("\n");
	exits(0);
}
