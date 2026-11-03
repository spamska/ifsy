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
#include <bio.h>
#include <ifsy.h>

static void
usage(void)
{
	fprint(2, "usage: head [-n lines] [file ...]\n");
	exits("usage");
}

Biobuf bout;

static void
head(Biobuf *b, long n)
{
	char *p;
	long i;

	for(i = 0; i < n; i++){
		p = Brdstr(b, '\n', 0);
		if(p == nil)
			break;
		Bwrite(&bout, p, strlen(p));
		free(p);
	}
}

void
main(int argc, char **argv)
{
	Biobuf in, *b;
	long n;
	int i, multi, status;

	n = 10;
	ARGBEGIN{
	case 'n':
		n = atol(EARGF(usage()));
		break;
	default:
		if(ARGC() >= '0' && ARGC() <= '9'){
			n = atol(&argv[0][1]);
			_args = "";
			break;
		}
		usage();
	}ARGEND
	Binit(&bout, 1, OWRITE);
	status = 0;
	if(argc == 0){
		Binit(&in, 0, OREAD);
		head(&in, n);
	} else {
		multi = argc > 1;
		for(i = 0; i < argc; i++){
			b = Bopen(argv[i], OREAD);
			if(b == nil){
				fprint(2, "head: can't open %s: %r\n", argv[i]);
				status = 1;
				continue;
			}
			if(multi)
				Bprint(&bout, "%s==> %s <==\n", i ? "\n" : "", argv[i]);
			head(b, n);
			Bterm(b);
		}
	}
	Bflush(&bout);
	exits(status ? "open" : nil);
}
