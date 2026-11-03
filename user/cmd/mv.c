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

static int fflag, vflag;

static void
usage(void)
{
	fprint(2, "usage: mv [-fv] source... target\n");
	exits("usage");
}

static char*
base(char *p)
{
	char *q;

	q = strrchr(p, '/');
	return q != nil && q[1] != 0 ? q + 1 : p;
}

/* A name without its trailing slashes ("dir/" -> "dir"), in a fresh copy. */
static char*
trimmed(char *p)
{
	char *s;
	long n;

	s = strdup(p);
	if(s == nil)
		return nil;
	n = strlen(s);
	while(n > 1 && s[n-1] == '/')
		s[--n] = 0;
	return s;
}

static int
move(char *from, char *to)
{
	Dir *sd, *dd;
	char *f, tmp[16];
	int r;

	f = trimmed(from);
	if(f == nil){
		fprint(2, "mv: out of memory\n");
		return 1;
	}
	/* a link is moved as itself, whatever it points to */
	sd = dirstat(f);
	if(readlink(f, tmp, sizeof tmp) >= 0){
		free(sd);
		sd = nil;
	}else if(sd == nil){
		fprint(2, "mv: can't stat %s: %r\n", from);
		free(f);
		return 1;
	}
	dd = dirstat(to);
	if(dd != nil){
		if(sd != nil && dd->qid.path == sd->qid.path && dd->length == sd->length && !(dd->mode & DMDIR) == !(sd->mode & DMDIR)
		&& strcmp(f, to) == 0){
			fprint(2, "mv: %s and %s are the same file\n", from, to);
			r = 1;
			goto out;
		}
		if(dd->mode & DMDIR){
			fprint(2, "mv: can't replace directory %s\n", to);
			r = 1;
			goto out;
		}
		if(sd != nil && (sd->mode & DMDIR)){
			fprint(2, "mv: can't replace file %s with a directory\n", to);
			r = 1;
			goto out;
		}
		if(remove(to) < 0){
			fprint(2, "mv: can't replace %s: %r\n", to);
			r = 1;
			goto out;
		}
	}
	r = 0;
	if(fsctl("rename", f, to) < 0){
		fprint(2, "mv: can't move %s to %s: %r\n", from, to);
		r = 1;
	}else if(vflag)
		print("%s -> %s\n", from, to);
out:
	free(sd);
	free(dd);
	free(f);
	return r;
}

void
main(int argc, char **argv)
{
	int i, bad, todir;
	Dir *d;
	char *dst, *to;

	ARGBEGIN{
	case 'f':
		fflag = 1;
		break;
	case 'v':
		vflag = 1;
		break;
	default:
		usage();
	}ARGEND
	USED(fflag);
	if(argc < 2)
		usage();

	dst = argv[argc-1];
	d = dirstat(dst);
	todir = d != nil && (d->mode & DMDIR);
	free(d);
	if(argc > 2 && !todir){
		fprint(2, "mv: target %s is not a directory\n", dst);
		exits("usage");
	}
	bad = 0;
	for(i = 0; i < argc - 1; i++){
		if(todir)
			to = smprint("%s/%s", dst, base(argv[i]));
		else
			to = strdup(dst);
		if(to == nil){
			fprint(2, "mv: out of memory\n");
			exits("memory");
		}
		bad |= move(argv[i], to);
		free(to);
	}
	exits(bad ? "errors" : 0);
}
