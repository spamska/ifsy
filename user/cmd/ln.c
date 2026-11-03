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

static int sflag, fflag, vflag, rflag;

static void
usage(void)
{
	fprint(2, "usage: ln [-sfv] target linkname\n       ln [-sfv] target... directory\n       ln -r link...      print where each symbolic link points\n");
	exits("usage");
}

static char*
base(char *p)
{
	char *q;

	q = strrchr(p, '/');
	return q != nil && q[1] != 0 ? q + 1 : p;
}

static int
mklink(char *target, char *name)
{
	Dir *d;

	if(fflag){
		/* replace a file or a link, never a directory (a link is looked at by name) */
		d = dirstat(name);
		if(d != nil && (d->mode & DMDIR)){
			free(d);
			fprint(2, "ln: %s is a directory\n", name);
			return 1;
		}
		free(d);
		remove(name);
	}
	if(fsctl(sflag ? "symlink" : "link", target, name) < 0){
		fprint(2, "ln: can't make %s -> %s: %r\n", name, target);
		return 1;
	}
	if(vflag)
		print("%s -> %s\n", name, target);
	return 0;
}

void
main(int argc, char **argv)
{
	int i, bad, todir;
	Dir *d;
	char *dst, *name;

	ARGBEGIN{
	case 's':
		sflag = 1;
		break;
	case 'f':
		fflag = 1;
		break;
	case 'v':
		vflag = 1;
		break;
	case 'r':
		rflag = 1;
		break;
	default:
		usage();
	}ARGEND
	if(rflag){
		char buf[1024];

		if(argc < 1 || sflag || fflag)
			usage();
		bad = 0;
		for(i = 0; i < argc; i++){
			if(readlink(argv[i], buf, sizeof buf) < 0){
				fprint(2, "ln: %s: not a symbolic link\n", argv[i]);
				bad = 1;
			}else
				print("%s\n", buf);
		}
		exits(bad ? "errors" : 0);
	}
	if(argc < 2)
		usage();

	dst = argv[argc-1];
	d = dirstat(dst);
	todir = d != nil && (d->mode & DMDIR);
	free(d);
	if(argc > 2 && !todir){
		fprint(2, "ln: %s is not a directory\n", dst);
		exits("usage");
	}
	bad = 0;
	for(i = 0; i < argc - 1; i++){
		if(todir)
			name = smprint("%s/%s", dst, base(argv[i]));
		else
			name = strdup(dst);
		if(name == nil){
			fprint(2, "ln: out of memory\n");
			exits("memory");
		}
		bad |= mklink(argv[i], name);
		free(name);
	}
	exits(bad ? "errors" : 0);
}
