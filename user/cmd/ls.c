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

static int lflag;

/* dir is where d was found (nil for a name given on the command line). A
 * directory listing reports symbolic links as plain entries, so look each one up. */
static void
show(Dir *d, char *dir)
{
	char *p, tgt[1024], arrow[1040];

	arrow[0] = 0;
	if(dir != nil && !(d->mode & DMDIR)){
		p = smprint("%s/%s", dir, d->name);
		if(p != nil && readlink(p, tgt, sizeof tgt) >= 0)
			snprint(arrow, sizeof arrow, " -> %s", tgt);
		free(p);
	}
	if(lflag)
		print("%M %10lld %s%s%s\n", d->mode, d->length, d->name, (d->mode & DMDIR) ? "/" : "", arrow);
	else
		print("%s%s%s\n", d->name, (d->mode & DMDIR) ? "/" : "", arrow);
}

static int
ls(char *path)
{
	Dir *d, *e;
	long n, i;
	int fd;

	d = dirstat(path);
	if(d == nil){
		fprint(2, "ls: can't stat %s: %r\n", path);
		return 1;
	}
	if(!(d->mode & DMDIR)){
		show(d, nil);
		free(d);
		return 0;
	}
	free(d);
	fd = open(path, OREAD);
	if(fd < 0){
		fprint(2, "ls: can't open %s: %r\n", path);
		return 1;
	}
	n = dirreadall(fd, &e);
	close(fd);
	if(n < 0){
		fprint(2, "ls: can't read %s: %r\n", path);
		return 1;
	}
	for(i = 0; i < n; i++)
		show(&e[i], path);
	free(e);
	return 0;
}

void
main(int argc, char **argv)
{
	int i, bad;

	bad = 0;
	argv++, argc--;
	if(argc > 0 && strcmp(argv[0], "-l") == 0){
		lflag = 1;
		argv++, argc--;
	}
	if(argc == 0)
		bad = ls(".");
	for(i = 0; i < argc; i++)
		bad |= ls(argv[i]);
	exits(bad ? "errors" : 0);
}
