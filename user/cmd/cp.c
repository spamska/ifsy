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

static int rflag, fflag, pflag, vflag;

static void
usage(void)
{
	fprint(2, "usage: cp [-rfpv] source... target\n");
	exits("usage");
}

static char*
base(char *p)
{
	char *q;

	q = strrchr(p, '/');
	return q != nil && q[1] != 0 ? q + 1 : p;
}

/* Is `to` the directory `from` or somewhere beneath it? */
static int
inside(char *from, char *to)
{
	long n;

	n = strlen(from);
	while(n > 1 && from[n-1] == '/')
		n--;
	return strncmp(from, to, n) == 0 && (to[n] == 0 || to[n] == '/');
}

static void
settimes(char *to, Dir *sd)
{
	Dir nd;

	if(!pflag)
		return;
	nulldir(&nd);
	nd.mtime = sd->mtime;
	nd.atime = sd->atime;
	if(dirwstat(to, &nd) < 0)
		fprint(2, "cp: can't set time of %s: %r\n", to);
}

static int
copyfile(char *from, char *to, Dir *sd)
{
	int in, out;
	long n, m;
	char *buf;
	Dir *dd;

	dd = dirstat(to);
	if(dd != nil){
		if(dd->qid.path == sd->qid.path && !(dd->mode & DMDIR) && (dd->length == sd->length)){
			/* same path hash and same size: the same file */
			free(dd);
			fprint(2, "cp: %s and %s are the same file\n", from, to);
			return 1;
		}
		free(dd);
	}
	in = open(from, OREAD);
	if(in < 0){
		fprint(2, "cp: can't open %s: %r\n", from);
		return 1;
	}
	out = create(to, OWRITE, sd->mode & 0777);
	if(out < 0 && fflag){
		remove(to);
		out = create(to, OWRITE, sd->mode & 0777);
	}
	if(out < 0){
		fprint(2, "cp: can't create %s: %r\n", to);
		close(in);
		return 1;
	}
	buf = malloc(8192);
	if(buf == nil){
		fprint(2, "cp: out of memory\n");
		close(in);
		close(out);
		return 1;
	}
	while((n = read(in, buf, 8192)) > 0){
		m = write(out, buf, n);
		if(m != n){
			fprint(2, "cp: write error on %s: %r\n", to);
			n = -1;
			break;
		}
	}
	if(n < 0)
		fprint(2, "cp: error reading %s: %r\n", from);
	free(buf);
	close(in);
	close(out);
	if(n < 0)
		return 1;
	settimes(to, sd);
	if(vflag)
		print("%s -> %s\n", from, to);
	return 0;
}

static int copy(char *from, char *to, int depth);

static int
copydir(char *from, char *to, Dir *sd, int depth)
{
	int fd, bad;
	long n, i;
	Dir *e, *dd;
	char *a, *b;

	if(depth > 40){
		fprint(2, "cp: %s: directory nesting too deep\n", from);
		return 1;
	}
	if(inside(from, to)){
		fprint(2, "cp: can't copy %s into itself (%s)\n", from, to);
		return 1;
	}
	dd = dirstat(to);
	if(dd == nil){
		fd = create(to, OREAD, DMDIR | (sd->mode & 0777) | 0300);
		if(fd < 0){
			fprint(2, "cp: can't create directory %s: %r\n", to);
			return 1;
		}
		close(fd);
	}else{
		if(!(dd->mode & DMDIR)){
			free(dd);
			fprint(2, "cp: %s exists and is not a directory\n", to);
			return 1;
		}
		free(dd);
	}
	fd = open(from, OREAD);
	if(fd < 0){
		fprint(2, "cp: can't open %s: %r\n", from);
		return 1;
	}
	n = dirreadall(fd, &e);
	close(fd);
	if(n < 0){
		fprint(2, "cp: can't read %s: %r\n", from);
		return 1;
	}
	bad = 0;
	for(i = 0; i < n; i++){
		a = smprint("%s/%s", from, e[i].name);
		b = smprint("%s/%s", to, e[i].name);
		if(a == nil || b == nil){
			fprint(2, "cp: out of memory\n");
			bad = 1;
		}else
			bad |= copy(a, b, depth + 1);
		free(a);
		free(b);
	}
	free(e);
	settimes(to, sd);
	return bad;
}

static int
copy(char *from, char *to, int depth)
{
	Dir *sd;
	int r;

	sd = dirstat(from);
	if(sd == nil){
		fprint(2, "cp: can't stat %s: %r\n", from);
		return 1;
	}
	if(sd->mode & DMDIR){
		if(!rflag){
			fprint(2, "cp: %s is a directory (use -r)\n", from);
			free(sd);
			return 1;
		}
		r = copydir(from, to, sd, depth);
	}else
		r = copyfile(from, to, sd);
	free(sd);
	return r;
}

void
main(int argc, char **argv)
{
	int i, bad;
	Dir *d;
	char *dst, *to;
	int todir;

	ARGBEGIN{
	case 'r':
	case 'R':
		rflag = 1;
		break;
	case 'f':
		fflag = 1;
		break;
	case 'p':
		pflag = 1;
		break;
	case 'v':
		vflag = 1;
		break;
	default:
		usage();
	}ARGEND
	if(argc < 2)
		usage();

	dst = argv[argc-1];
	d = dirstat(dst);
	todir = d != nil && (d->mode & DMDIR);
	free(d);
	if(argc > 2 && !todir){
		fprint(2, "cp: target %s is not a directory\n", dst);
		exits("usage");
	}
	bad = 0;
	for(i = 0; i < argc - 1; i++){
		if(todir)
			to = smprint("%s/%s", dst, base(argv[i]));
		else
			to = strdup(dst);
		if(to == nil){
			fprint(2, "cp: out of memory\n");
			exits("memory");
		}
		bad |= copy(argv[i], to, 0);
		free(to);
	}
	exits(bad ? "errors" : 0);
}
