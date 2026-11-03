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

static int aflag, sflag, hflag, cflag, bflag, maxdepth = -1;
static vlong blk = 4096;
static int bad;

static void
usage(void)
{
	fprint(2, "usage: du [-ackhsb] [-d depth] [file...]\n");
	exits("usage");
}

static vlong
charge(Dir *d)
{
	vlong n;

	if(bflag)
		return d->length;
	n = (d->length + blk - 1) / blk * blk;
	if(d->mode & DMDIR)
		n += blk;
	return n;
}

static void
report(vlong bytes, char *path)
{
	char h[24];

	if(hflag)
		print("%s\t%s\n", humansize(bytes, h, sizeof h), path);
	else if(bflag)
		print("%lld\t%s\n", bytes, path);
	else
		print("%lld\t%s\n", (bytes + 1023) / 1024, path);
}

static vlong
walk(char *path, Dir *d, int depth, int top)
{
	vlong total;
	int fd;
	long n, i;
	Dir *e;
	char *child;

	total = charge(d);
	if(d->mode & DMDIR){
		if(depth > 64){
			fprint(2, "du: %s: nesting too deep\n", path);
			bad = 1;
			return total;
		}
		fd = open(path, OREAD);
		if(fd < 0){
			fprint(2, "du: can't open %s: %r\n", path);
			bad = 1;
			return total;
		}
		n = dirreadall(fd, &e);
		close(fd);
		if(n < 0){
			fprint(2, "du: can't read %s: %r\n", path);
			bad = 1;
			return total;
		}
		for(i = 0; i < n; i++){
			child = smprint("%s/%s", strcmp(path, "/") == 0 ? "" : path, e[i].name);
			if(child == nil){
				fprint(2, "du: out of memory\n");
				exits("memory");
			}
			total += walk(child, &e[i], depth + 1, 0);
			free(child);
		}
		free(e);
		if(!sflag && (maxdepth < 0 || depth <= maxdepth))
			report(total, path);
	}else if(aflag && !sflag && (maxdepth < 0 || depth <= maxdepth))
		report(total, path);
	if(top && sflag)
		report(total, path);
	return total;
}

void
main(int argc, char **argv)
{
	int i;
	vlong grand, bs, nb, bf, ni, nf, t;
	char label[32], *dd;
	Dir *d;

	ARGBEGIN{
	case 'a':
		aflag = 1;
		break;
	case 's':
		sflag = 1;
		break;
	case 'h':
		hflag = 1;
		break;
	case 'k':
		hflag = 0;
		break;
	case 'c':
		cflag = 1;
		break;
	case 'b':
		bflag = 1;
		break;
	case 'd':
		dd = EARGF(usage());
		maxdepth = atoi(dd);
		break;
	default:
		usage();
	}ARGEND

	if(fsinfo(&bs, &nb, &bf, &ni, &nf, label, sizeof label) == 0 && bs > 0)
		blk = bs;
	if(argc == 0){
		argv[0] = ".";
		argc = 1;
	}
	grand = 0;
	for(i = 0; i < argc; i++){
		d = dirstat(argv[i]);
		if(d == nil){
			fprint(2, "du: can't stat %s: %r\n", argv[i]);
			bad = 1;
			continue;
		}
		t = walk(argv[i], d, 0, 1);
		grand += t;
		free(d);
	}
	if(cflag)
		report(grand, "total");
	exits(bad ? "errors" : 0);
}
