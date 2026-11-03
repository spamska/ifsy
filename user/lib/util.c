/* Small helpers shared by the Ifsy commands. */
#include <u.h>
#include <libc.h>
#include <ctype.h>
#include <ifsy.h>

extern vlong _nsec(void);

vlong
nsec(void)
{
	return _nsec();
}

char*
slurp(char *path, long *np)
{
	int fd;
	long n, tot, cap;
	char *b, *nb;

	fd = open(path, OREAD);
	if(fd < 0)
		return nil;
	cap = 1024;
	tot = 0;
	b = malloc(cap);
	if(b == nil){
		close(fd);
		return nil;
	}
	for(;;){
		if(tot + 512 > cap){
			cap *= 2;
			nb = realloc(b, cap);
			if(nb == nil){
				free(b);
				close(fd);
				return nil;
			}
			b = nb;
		}
		n = read(fd, b + tot, cap - tot - 1);
		if(n <= 0)
			break;
		tot += n;
	}
	close(fd);
	b[tot] = 0;
	if(np != nil)
		*np = tot;
	return b;
}

/* /dev/time: "<epoch secs> <ns since boot> <ticks> <hz>" */
static int
timefields(vlong *f)
{
	char *s, *fld[5];
	int i, n;

	s = slurp("/dev/time", nil);
	if(s == nil)
		return -1;
	n = tokenize(s, fld, 5);
	if(n < 4){
		free(s);
		return -1;
	}
	for(i = 0; i < 4; i++)
		f[i] = strtoll(fld[i], nil, 10);
	free(s);
	return 0;
}

vlong
wallsecs(void)
{
	vlong f[4];

	if(timefields(f) < 0)
		return nsec() / 1000000000LL;
	return f[0];
}

/*
 * Nanoseconds since 1970 (UTC), as 9front's nsec() would return them.  The
 * kernel's nsec() counts from boot, so the epoch seconds come from /dev/time
 * and the sub-second part from the boot clock.  Without a clock the
 * seconds are those since boot.
 */
vlong
wallnsec(void)
{
	vlong f[4];

	if(timefields(f) < 0)
		return nsec();
	return f[0]*1000000000LL + f[1]%1000000000LL;
}

vlong
uptimens(void)
{
	vlong f[4];

	if(timefields(f) < 0)
		return nsec();
	return f[1];
}

int
osinfo(char *sys, char *rel, char *ver, char *mach, int n)
{
	char *s, *fld[8];
	int k;

	strecpy(sys, sys + n, "ifsy");
	strecpy(rel, rel + n, "?");
	strecpy(ver, ver + n, "?");
	strecpy(mach, mach + n, "?");
	s = slurp("/dev/osinfo", nil);
	if(s == nil)
		return -1;
	k = tokenize(s, fld, 8);
	if(k >= 1)
		strecpy(sys, sys + n, fld[0]);
	if(k >= 2)
		strecpy(rel, rel + n, fld[1]);
	if(k >= 3)
		strecpy(ver, ver + n, fld[2]);
	if(k >= 4)
		strecpy(mach, mach + n, fld[3]);
	free(s);
	return 0;
}

char*
sysname(void)
{
	static char name[64];
	char *s, *p;

	s = slurp("/dev/sysname", nil);
	if(s == nil){
		strcpy(name, "ifsy");
		return name;
	}
	p = strchr(s, '\n');
	if(p != nil)
		*p = 0;
	strecpy(name, name + sizeof name, s);
	free(s);
	return name;
}

void
civil(vlong secs, int *y, int *mo, int *d, int *h, int *mi, int *s, int *wday)
{
	vlong days, rem, z, era, doe, yoe, doy, mp, yy;

	days = secs / 86400;
	rem = secs % 86400;
	if(rem < 0){
		rem += 86400;
		days--;
	}
	*h = rem / 3600;
	*mi = (rem % 3600) / 60;
	*s = rem % 60;
	*wday = (days + 4) % 7;
	if(*wday < 0)
		*wday += 7;
	z = days + 719468;
	era = (z >= 0 ? z : z - 146096) / 146097;
	doe = z - era * 146097;
	yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
	yy = yoe + era * 400;
	doy = doe - (365*yoe + yoe/4 - yoe/100);
	mp = (5*doy + 2) / 153;
	*d = doy - (153*mp + 2)/5 + 1;
	*mo = mp < 10 ? mp + 3 : mp - 9;
	*y = yy + (*mo <= 2);
}

void
fmtdate(vlong secs, char *buf, int n)
{
	int y, mo, d, h, mi, s, wd;

	civil(secs, &y, &mo, &d, &h, &mi, &s, &wd);
	snprint(buf, n, "%04d-%02d-%02d %02d:%02d:%02d", y, mo, d, h, mi, s);
}

char*
humansize(uvlong b, char *buf, int n)
{
	static char unit[] = "KMGTP";
	uvlong rem;
	int u;

	if(b < 1024){
		snprint(buf, n, "%llud", b);
		return buf;
	}
	rem = 0;
	u = -1;
	while(b >= 1024 && u < 4){
		rem = b % 1024;
		b /= 1024;
		u++;
	}
	if(b < 10)
		snprint(buf, n, "%llud.%llud%c", b, rem * 10 / 1024, unit[u]);
	else
		snprint(buf, n, "%llud%c", b + (rem >= 512), unit[u]);
	return buf;
}

/* Shell-style pattern match. */
int
globmatch(char *p, char *s, int fold)
{
	int c, neg, ok, lo, hi, sc, first;

	for(;;){
		c = *p++;
		switch(c){
		case 0:
			return *s == 0;
		case '*':
			while(*p == '*')
				p++;
			if(*p == 0)
				return 1;
			for(; *s; s++)
				if(globmatch(p, s, fold))
					return 1;
			return 0;
		case '?':
			if(*s == 0)
				return 0;
			s++;
			continue;
		case '[':
			if(*s == 0)
				return 0;
			sc = (uchar)*s++;
			if(fold)
				sc = tolower(sc);
			neg = 0;
			if(*p == '!' || *p == '^'){
				neg = 1;
				p++;
			}
			ok = 0;
			first = 1;
			for(;;){
				if(*p == 0)
					return 0;	/* unterminated class */
				if(*p == ']' && !first)
					break;
				first = 0;
				lo = (uchar)*p++;
				hi = lo;
				if(*p == '-' && p[1] != ']' && p[1] != 0){
					hi = (uchar)p[1];
					p += 2;
				}
				if(fold){
					lo = tolower(lo);
					hi = tolower(hi);
				}
				if(sc >= lo && sc <= hi)
					ok = 1;
			}
			p++;
			if(ok == neg)
				return 0;
			continue;
		case '\\':
			if(*p)
				c = *p++;
			/* fall through */
		default:
			sc = (uchar)*s;
			if(sc == 0)
				return 0;
			if(fold ? tolower(sc) != tolower(c) : sc != c)
				return 0;
			s++;
			continue;
		}
	}
}

/* /dev/ps: "pid ppid state ticks name" per line. */
int
psread(int (*each)(int, int, char*, vlong, char*, void*), void *arg)
{
	char *s, *line, *nl, *f[6];
	int n, count;

	s = slurp("/dev/ps", nil);
	if(s == nil)
		return -1;
	count = 0;
	for(line = s; *line; line = nl){
		nl = strchr(line, '\n');
		if(nl != nil)
			*nl++ = 0;
		else
			nl = line + strlen(line);
		n = tokenize(line, f, 6);
		if(n < 5)
			continue;
		count++;
		if(each != nil && each(atoi(f[0]), atoi(f[1]), f[2], strtoll(f[3], nil, 10), f[4], arg) < 0)
			break;
	}
	free(s);
	return count;
}

int
fsinfo(vlong *bsize, vlong *blocks, vlong *bfree, vlong *inodes, vlong *ifree, char *label, int n)
{
	char *s, *f[10];
	int k;

	s = slurp("/dev/fsinfo", nil);
	if(s == nil)
		return -1;
	k = tokenize(s, f, 10);
	if(k < 8){
		free(s);
		return -1;
	}
	*bsize = strtoll(f[2], nil, 10);
	*blocks = strtoll(f[3], nil, 10);
	*bfree = strtoll(f[4], nil, 10);
	*inodes = strtoll(f[5], nil, 10);
	*ifree = strtoll(f[6], nil, 10);
	strecpy(label, label + n, f[7]);
	free(s);
	return 0;
}

/* /dev/swap: "<bytes> memory\n<pagesize> pagesize\n<free>/<total> user\n" */
int
memstat(vlong *total, vlong *freebytes)
{
	char *s, *p;
	vlong pg, fr, tot;

	s = slurp("/dev/swap", nil);
	if(s == nil)
		return -1;
	pg = 4096;
	p = strstr(s, " pagesize");
	if(p != nil){
		while(p > s && p[-1] != '\n')
			p--;
		pg = strtoll(p, nil, 10);
	}
	p = strstr(s, " user");
	if(p == nil){
		free(s);
		return -1;
	}
	while(p > s && p[-1] != '\n')
		p--;
	fr = strtoll(p, &p, 10);
	tot = 0;
	if(*p == '/')
		tot = strtoll(p + 1, nil, 10);
	free(s);
	*total = tot * pg;
	*freebytes = fr * pg;
	return 0;
}

int
consize(int *cols, int *rows)
{
	char *s, *f[3];

	*cols = 80;
	*rows = 24;
	s = slurp("/dev/consinfo", nil);
	if(s == nil)
		return -1;
	if(tokenize(s, f, 3) >= 2){
		*cols = atoi(f[0]);
		*rows = atoi(f[1]);
	}
	free(s);
	return 0;
}

/* /dev/display: "cols rows width height cellw cellh defcols defrows maxcols maxrows" */
int
dispinfo(vlong v[10])
{
	char *s, *f[10];
	int i;

	s = slurp("/dev/display", nil);
	if(s == nil)
		return -1;
	if(tokenize(s, f, 10) < 10){
		free(s);
		return -1;
	}
	for(i = 0; i < 10; i++)
		v[i] = strtoll(f[i], nil, 10);
	free(s);
	return 0;
}

/* /dev/diskinfo: "<disk bytes> <largest size it can grow to> <free memory bytes>" */
/* /dev/diskinfo: "<disk bytes> <largest size it can grow to> <free memory bytes>" */
int
diskinfo2(vlong *size, vlong *max, vlong *freemem, int *ondisk)
{
	char *s, *f[4];
	int n;

	s = slurp("/dev/diskinfo", nil);
	if(s == nil)
		return -1;
	n = tokenize(s, f, 4);
	if(n < 3){
		free(s);
		return -1;
	}
	*size = strtoll(f[0], nil, 10);
	*max = strtoll(f[1], nil, 10);
	*freemem = strtoll(f[2], nil, 10);
	*ondisk = n > 3 && f[3][0] == '1';
	free(s);
	return 0;
}

int
diskinfo(vlong *size, vlong *max, vlong *freemem)
{
	int ondisk;

	return diskinfo2(size, max, freemem, &ondisk);
}

/* /dev/fsctl: `op`, then one argument per line. 0 on success, -1 with errstr set. */
int
fsctl(char *op, char *a, char *b)
{
	char *cmd;
	int fd, ok;
	long n;

	cmd = smprint("%s\n%s%s%s", op, a != nil ? a : "", b != nil ? "\n" : "", b != nil ? b : "");
	if(cmd == nil)
		return -1;
	fd = open("/dev/fsctl", ORDWR);
	if(fd < 0){
		free(cmd);
		return -1;
	}
	n = strlen(cmd);
	ok = write(fd, cmd, n) == n;
	close(fd);
	free(cmd);
	return ok ? 0 : -1;
}

/* The target stored in symbolic link `path` (not followed), copied to buf.
 * Returns its length, or -1 if path is not a link or does not exist. */
long
readlink(char *path, char *buf, long n)
{
	char *cmd;
	int fd;
	long k;

	cmd = smprint("readlink\n%s", path);
	if(cmd == nil)
		return -1;
	fd = open("/dev/fsctl", ORDWR);
	if(fd < 0){
		free(cmd);
		return -1;
	}
	k = -1;
	if(write(fd, cmd, strlen(cmd)) > 0){
		k = read(fd, buf, n - 1);
		if(k >= 0){
			if(k > 0 && buf[k-1] == '\n')
				k--;
			buf[k] = 0;
		}
	}
	close(fd);
	free(cmd);
	return k;
}

/* Plan 9's compilers make this a builtin; here it feeds malloc tags, which
 * are no-ops, so there is nothing worth returning. */
uintptr
getcallerpc(void *a)
{
	USED(a);
	return 0;
}
