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

enum {
	Maxstring	= 128,
	Maxpath		= 256,
	Maxlines	= 8,
};

typedef struct DS DS;
struct DS {
	char	buf[Maxstring];
	char	*netdir;
	char	*proto;
	char	*rem;
	char	*local;
	char	*dir;
	int	*cfdp;
};

typedef struct Svc Svc;
struct Svc {
	char	*name;
	int	port;
};

static Svc svctab[] = {
	{"echo",	7},
	{"discard",	9},
	{"daytime",	13},
	{"ftp",		21},
	{"ssh",		22},
	{"telnet",	23},
	{"smtp",	25},
	{"time",	37},
	{"domain",	53},
	{"dns",		53},
	{"tftp",	69},
	{"gopher",	70},
	{"http",	80},
	{"www",		80},
	{"pop3",	110},
	{"ntp",		123},
	{"imap",	143},
	{"imap4",	143},
	{"https",	443},
	{"9fs",		564},
	{"9pfs",	564},
	{"rexexec",	512},
	{"irc",		6667},
	{nil,		0},
};

static char netmtpt[Maxpath] = "/net";

void
setnetmtpt(char *to, int n, char *net)
{
	if(net == nil)
		net = "/net";
	if(*net == '/')
		strncpy(to, net, n);
	else
		snprint(to, n, "/net.%s", net);
	to[n-1] = 0;
}

char*
netmkaddrbuf(char *linear, char *defnet, char *defsrv, char *buf, int len)
{
	char *cp;

	cp = strchr(linear, '!');
	if(cp == nil){
		if(defnet == nil)
			defnet = "net";
		if(defsrv != nil)
			snprint(buf, len, "%s!%s!%s", defnet, linear, defsrv);
		else
			snprint(buf, len, "%s!%s", defnet, linear);
		return buf;
	}
	cp = strchr(cp+1, '!');
	if(cp == nil){
		if(defsrv == nil)
			strncpy(buf, linear, len);
		else
			snprint(buf, len, "%s!%s", linear, defsrv);
		buf[len-1] = 0;
		return buf;
	}
	strncpy(buf, linear, len);
	buf[len-1] = 0;
	return buf;
}

char*
netmkaddr(char *linear, char *defnet, char *defsrv)
{
	static char addr[Maxstring];

	return netmkaddrbuf(linear, defnet, defsrv, addr, sizeof addr);
}

static void
dialparse(char *dest, DS *ds)
{
	char *p;

	strncpy(ds->buf, dest, sizeof(ds->buf)-1);
	ds->buf[sizeof(ds->buf)-1] = 0;
	ds->netdir = nil;
	ds->proto = ds->buf;
	ds->rem = nil;
	p = strchr(ds->buf, '!');
	if(p == nil){
		ds->proto = "net";
		ds->rem = ds->buf;
		return;
	}
	*p++ = 0;
	ds->rem = p;
	if(*ds->proto == '/'){
		p = strrchr(ds->proto, '/');
		if(p != ds->proto){
			*p++ = 0;
			ds->netdir = ds->proto;
			ds->proto = p;
		} else
			ds->proto++;
	}
	if(ds->netdir == nil)
		ds->netdir = netmtpt;
}

static int
isnumber(char *s)
{
	if(*s == 0)
		return 0;
	for(; *s; s++)
		if(*s < '0' || *s > '9')
			return 0;
	return 1;
}

static int
isip4(char *s)
{
	int dots, digits, v;

	dots = digits = v = 0;
	for(; *s; s++){
		if(*s == '.'){
			if(digits == 0 || ++dots > 3)
				return 0;
			digits = v = 0;
		} else if(*s >= '0' && *s <= '9'){
			v = v*10 + *s - '0';
			if(++digits > 3 || v > 255)
				return 0;
		} else
			return 0;
	}
	return dots == 3 && digits > 0;
}

static int
svcport(char *s)
{
	Svc *v;

	if(isnumber(s))
		return atoi(s);
	for(v = svctab; v->name != nil; v++)
		if(strcmp(v->name, s) == 0)
			return v->port;
	return -1;
}

static int
localxlate(DS *ds, char *clone, int nclone, char *addr, int naddr)
{
	char rem[Maxstring], *f[4];
	char *proto;
	int n, port;

	proto = ds->proto;
	if(strcmp(proto, "net") == 0)
		proto = "tcp";
	if(strcmp(proto, "tcp") != 0 && strcmp(proto, "udp") != 0){
		werrstr("unknown network %s", ds->proto);
		return -1;
	}
	strncpy(rem, ds->rem, sizeof(rem)-1);
	rem[sizeof(rem)-1] = 0;
	n = getfields(rem, f, 4, 0, "!");
	if(n < 1 || n > 3){
		werrstr("bad dial string");
		return -1;
	}
	if(n == 1){
		f[1] = f[0];
		f[0] = "*";
	}
	if(strcmp(f[0], "*") != 0 && !isip4(f[0])){
		werrstr("can't translate address");
		return -1;
	}
	port = svcport(f[1]);
	if(port < 0 || port > 65535){
		werrstr("unknown service %s", f[1]);
		return -1;
	}
	snprint(clone, nclone, "%s/%s/clone", ds->netdir, proto);
	snprint(addr, naddr, "%s!%d", f[0], port);
	return 0;
}

static int
csquery(DS *ds, char lines[][Maxpath], int max)
{
	char buf[Maxpath];
	int fd, n, k;

	snprint(buf, sizeof buf, "%s/cs", ds->netdir);
	fd = open(buf, ORDWR);
	if(fd < 0)
		return -1;
	snprint(buf, sizeof buf, "%s!%s", ds->proto, ds->rem);
	if(write(fd, buf, strlen(buf)) < 0){
		close(fd);
		return -1;
	}
	n = 0;
	while(n < max){
		k = read(fd, lines[n], Maxpath-1);
		if(k <= 0)
			break;
		lines[n][k] = 0;
		n++;
	}
	close(fd);
	return n;
}

static int
splitline(char *line, char **clone, char **addr)
{
	char *p;

	p = strchr(line, '\n');
	if(p != nil)
		*p = 0;
	p = strchr(line, ' ');
	if(p == nil)
		return -1;
	*p++ = 0;
	*clone = line;
	*addr = p;
	return 0;
}

static void
localclone(DS *ds, char *clone, char *out, int n)
{
	char *p;

	if(*clone == '/'){
		p = strchr(clone+1, '/');
		if(p == nil)
			p = clone;
		snprint(out, n, "%s%s", ds->netdir, p);
	} else
		snprint(out, n, "%s/%s", ds->netdir, clone);
}

static int
opendir(DS *ds, char *clone, char *cname, int ncname, char *name, int nname)
{
	char *p;
	int cfd, n;

	localclone(ds, clone, cname, ncname);
	cfd = open(cname, ORDWR);
	if(cfd < 0)
		return -1;
	n = read(cfd, name, nname-1);
	if(n <= 0){
		close(cfd);
		return -1;
	}
	name[n] = 0;
	for(p = name; *p == ' '; p++)
		;
	snprint(name, nname, "%lud", strtoul(p, nil, 0));
	p = strrchr(cname, '/');
	*p = 0;
	return cfd;
}

static int
call(char *clone, char *dest, DS *ds)
{
	int fd, cfd;
	char cname[Maxpath], name[Maxpath], data[Maxpath], cmd[Maxpath];

	cfd = opendir(ds, clone, cname, sizeof cname, name, sizeof name);
	if(cfd < 0)
		return -1;
	if(ds->dir != nil)
		snprint(ds->dir, NETPATHLEN, "%s/%s", cname, name);
	snprint(data, sizeof data, "%s/%s/data", cname, name);
	if(ds->local != nil)
		snprint(cmd, sizeof cmd, "connect %s %s", dest, ds->local);
	else
		snprint(cmd, sizeof cmd, "connect %s", dest);
	if(write(cfd, cmd, strlen(cmd)) < 0){
		close(cfd);
		return -1;
	}
	fd = open(data, ORDWR);
	if(fd < 0){
		close(cfd);
		return -1;
	}
	if(ds->cfdp != nil)
		*ds->cfdp = cfd;
	else
		close(cfd);
	return fd;
}

int
dial(char *dest, char *local, char *dir, int *cfdp)
{
	DS ds;
	char lines[Maxlines][Maxpath], clone[Maxpath], addr[Maxpath], err[ERRMAX];
	char *c, *a;
	int i, n, fd;

	ds.local = local;
	ds.dir = dir;
	ds.cfdp = cfdp;
	dialparse(dest, &ds);
	werrstr("can't translate address");
	strcpy(err, "can't translate address");

	n = csquery(&ds, lines, Maxlines);
	for(i = 0; i < n; i++){
		if(splitline(lines[i], &c, &a) < 0)
			continue;
		fd = call(c, a, &ds);
		if(fd >= 0)
			return fd;
		rerrstr(err, sizeof err);
	}
	if(n > 0){
		werrstr("%s", err);
		return -1;
	}
	if(localxlate(&ds, clone, sizeof clone, addr, sizeof addr) < 0)
		return -1;
	return call(clone, addr, &ds);
}

int
announce(char *addr, char *dir)
{
	DS ds;
	char lines[Maxlines][Maxpath], clone[Maxpath], caddr[Maxpath];
	char cname[Maxpath], name[Maxpath], cmd[Maxpath];
	char *c, *a;
	int n, cfd;

	ds.local = nil;
	ds.dir = dir;
	ds.cfdp = nil;
	dialparse(addr, &ds);
	n = csquery(&ds, lines, 1);
	if(n > 0 && splitline(lines[0], &c, &a) == 0){
		strncpy(clone, c, sizeof(clone)-1);
		strncpy(caddr, a, sizeof(caddr)-1);
		clone[sizeof(clone)-1] = caddr[sizeof(caddr)-1] = 0;
	} else if(localxlate(&ds, clone, sizeof clone, caddr, sizeof caddr) < 0)
		return -1;
	cfd = opendir(&ds, clone, cname, sizeof cname, name, sizeof name);
	if(cfd < 0)
		return -1;
	if(dir != nil)
		snprint(dir, NETPATHLEN, "%s/%s", cname, name);
	snprint(cmd, sizeof cmd, "announce %s", caddr);
	if(write(cfd, cmd, strlen(cmd)) < 0){
		close(cfd);
		return -1;
	}
	return cfd;
}

int
listen(char *dir, char *newdir)
{
	char buf[Maxpath], *p;
	int ctl, n;

	snprint(buf, sizeof buf, "%s/listen", dir);
	ctl = open(buf, ORDWR);
	if(ctl < 0)
		return -1;
	n = read(ctl, buf, sizeof(buf)-1);
	if(n <= 0){
		close(ctl);
		return -1;
	}
	buf[n] = 0;
	for(p = buf; *p == ' '; p++)
		;
	n = strtoul(p, nil, 0);
	strncpy(newdir, dir, NETPATHLEN-12);
	newdir[NETPATHLEN-12] = 0;
	p = strrchr(newdir, '/');
	if(p == nil){
		close(ctl);
		werrstr("bad announce directory");
		return -1;
	}
	snprint(p+1, 12, "%d", n);
	return ctl;
}

int
accept(int ctl, char *dir)
{
	char buf[Maxpath], *num;

	if(ctl < 0)
		return -1;
	num = strrchr(dir, '/');
	num = num == nil ? dir : num+1;
	snprint(buf, sizeof buf, "accept %s", num);
	return write(ctl, buf, strlen(buf));
}

int
reject(int ctl, char *dir, char *cause)
{
	char buf[Maxpath], *num;

	if(ctl < 0)
		return -1;
	num = strrchr(dir, '/');
	num = num == nil ? dir : num+1;
	snprint(buf, sizeof buf, "reject %s %s", num, cause);
	return write(ctl, buf, strlen(buf));
}

int
hangup(int ctl)
{
	return write(ctl, "hangup", 6);
}
