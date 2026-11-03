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
	fprint(2, "usage: df [-hik] [file...]\n");
	exits("usage");
}

static void
num(int human, vlong bytes, char *out, int n)
{
	if(human)
		humansize(bytes, out, n);
	else
		snprint(out, n, "%lld", bytes / 1024);
}

void
main(int argc, char **argv)
{
	int hflag, iflag, i, pct;
	vlong bsize, blocks, bfree, inodes, ifree, size, used, avail;
	char label[32], a[24], b[24], c[24], dev[64];
	Dir *d;

	hflag = iflag = 0;
	ARGBEGIN{
	case 'h':
		hflag = 1;
		break;
	case 'i':
		iflag = 1;
		break;
	case 'k':
		hflag = 0;
		break;
	default:
		usage();
	}ARGEND

	for(i = 0; i < argc; i++){
		d = dirstat(argv[i]);
		if(d == nil){
			fprint(2, "df: can't stat %s: %r\n", argv[i]);
			exits("stat");
		}
		free(d);
	}
	if(fsinfo(&bsize, &blocks, &bfree, &inodes, &ifree, label, sizeof label) < 0){
		fprint(2, "df: can't read /dev/fsinfo: %r\n");
		exits("fsinfo");
	}
	snprint(dev, sizeof dev, "ext4:%s", strcmp(label, "-") == 0 ? "ramdisk" : label);

	if(iflag){
		used = inodes - ifree;
		pct = inodes > 0 ? (used * 100 + inodes - 1) / inodes : 0;
		print("%-16s %10s %10s %10s %5s  %s\n", "Filesystem", "Inodes", "IUsed", "IFree", "IUse%", "Mounted on");
		print("%-16s %10lld %10lld %10lld %4d%%  /\n", dev, inodes, used, ifree, pct);
		exits(0);
	}
	size = blocks * bsize;
	avail = bfree * bsize;
	used = size - avail;
	pct = size > 0 ? (used * 100 + size - 1) / size : 0;
	print("%-16s %10s %10s %10s %5s  %s\n", "Filesystem", hflag ? "Size" : "1K-blocks", "Used", "Avail", "Use%", "Mounted on");
	num(hflag, size, a, sizeof a);
	num(hflag, used, b, sizeof b);
	num(hflag, avail, c, sizeof c);
	print("%-16s %10s %10s %10s %4d%%  /\n", dev, a, b, c, pct);
	exits(0);
}
