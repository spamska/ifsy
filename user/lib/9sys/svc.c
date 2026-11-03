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

/*
 * Small pieces of 9front's libc that the service manager (init, listen)
 * needs and that Ifsy did not have yet: syslog, procsetname, ainc/adec,
 * getuser and procsetuser.
 */

#include <u.h>
#include <libc.h>

extern vlong	wallsecs(void);				/* lib/util.c */
extern void	fmtdate(vlong, char*, int);		/* lib/util.c */

/*
 * syslog(cons, logname, fmt, ...)
 *
 * 9front appends the line to /sys/log/logname on the file server.  Here
 * it is appended to /sys/log/logname on the root file system; if that
 * cannot be opened the line goes to standard error instead.  A non-zero
 * cons also copies it to standard error.
 */
void
syslog(int cons, char *logname, char *fmt, ...)
{
	char buf[1024], path[128], stamp[32], *p, *e;
	va_list arg;
	int fd, n, ok;

	fmtdate(wallsecs(), stamp, sizeof stamp);
	p = buf;
	e = buf + sizeof buf - 2;
	p = seprint(p, e, "%s %s: ", stamp, logname);
	va_start(arg, fmt);
	p = vseprint(p, e, fmt, arg);
	va_end(arg);
	if(p > buf && p[-1] == '\n')
		p--;
	*p++ = '\n';
	*p = 0;
	n = p - buf;

	ok = 0;
	snprint(path, sizeof path, "/sys/log/%s", logname);
	fd = open(path, OWRITE);
	if(fd < 0)
		fd = create(path, OWRITE, 0666);
	if(fd >= 0){
		seek(fd, 0, 2);
		ok = write(fd, buf, n) == n;
		close(fd);
	}
	if(!ok || cons)
		write(2, buf, n);
}

/*
 * Plan 9 shows the name in /proc/pid/args; Ifsy's /dev/ps takes it from
 * the program's path, so there is nothing to set.
 */
void
procsetname(char *fmt, ...)
{
	USED(fmt);
}

long
ainc(long *p)
{
	return __sync_add_and_fetch(p, 1);
}

long
adec(long *p)
{
	return __sync_sub_and_fetch(p, 1);
}

/*
 * The user name of the calling process: /dev/user, "none" if that cannot
 * be read.  The result is in a static buffer.
 */
char*
getuser(void)
{
	static char user[32];
	int fd, n;

	strcpy(user, "none");
	fd = open("/dev/user", OREAD);
	if(fd < 0)
		return user;
	n = read(fd, user, sizeof user - 1);
	close(fd);
	if(n <= 0){
		strcpy(user, "none");
		return user;
	}
	user[n] = 0;
	while(n > 0 && (user[n-1] == '\n' || user[n-1] == ' ' || user[n-1] == 0))
		user[--n] = 0;
	if(n == 0)
		strcpy(user, "none");
	return user;
}

/*
 * Change the calling process's user name (and that of the processes it
 * starts afterwards).  Only a process running as root may do this; the
 * kernel refuses everybody else.  0 on success, -1 with errstr set.
 */
int
procsetuser(char *user)
{
	int fd, n;

	if(user == nil || *user == 0){
		werrstr("bad user name");
		return -1;
	}
	fd = open("/dev/userctl", OWRITE);
	if(fd < 0)
		return -1;
	n = strlen(user);
	if(write(fd, user, n) != n){
		close(fd);
		return -1;
	}
	close(fd);
	return 0;
}
