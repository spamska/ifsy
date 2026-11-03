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
 * The user database for Ifsy's multi-user support.
 *
 *	/adm/users	name:uid:home:shell		one line per user
 *	/adm/shadow	name:salt:hash			one line per password
 *
 * A user without a line in /adm/shadow has no password and logs in by just
 * pressing return; a hash of "!" locks the account.  The hash is SHA-256
 * over salt and password, iterated Rounds times.  Only root can read or
 * write /adm/shadow (the kernel enforces this).
 */

#include <u.h>
#include <libc.h>
#include <ifsy.h>

extern vlong	wallnsec(void);

enum {
	Rounds = 2000,
	Maxline = 512,
};

/* ---- SHA-256 (FIPS 180-4) ---- */

typedef struct Sha Sha;
struct Sha {
	uint	h[8];
	uchar	buf[64];
	uvlong	len;
	int	nbuf;
};

static uint K[64] = {
	0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
	0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
	0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
	0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
	0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
	0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
	0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
	0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

#define ROR(x, n)	(((x) >> (n)) | ((x) << (32 - (n))))

static void
shablock(Sha *s, uchar *p)
{
	uint w[64], a, b, c, d, e, f, g, h, t1, t2;
	int i;

	for(i = 0; i < 16; i++)
		w[i] = (uint)p[4*i] << 24 | (uint)p[4*i+1] << 16 | (uint)p[4*i+2] << 8 | p[4*i+3];
	for(; i < 64; i++){
		t1 = ROR(w[i-15], 7) ^ ROR(w[i-15], 18) ^ (w[i-15] >> 3);
		t2 = ROR(w[i-2], 17) ^ ROR(w[i-2], 19) ^ (w[i-2] >> 10);
		w[i] = w[i-16] + t1 + w[i-7] + t2;
	}
	a = s->h[0]; b = s->h[1]; c = s->h[2]; d = s->h[3];
	e = s->h[4]; f = s->h[5]; g = s->h[6]; h = s->h[7];
	for(i = 0; i < 64; i++){
		t1 = h + (ROR(e, 6) ^ ROR(e, 11) ^ ROR(e, 25)) + ((e & f) ^ (~e & g)) + K[i] + w[i];
		t2 = (ROR(a, 2) ^ ROR(a, 13) ^ ROR(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
		h = g; g = f; f = e; e = d + t1;
		d = c; c = b; b = a; a = t1 + t2;
	}
	s->h[0] += a; s->h[1] += b; s->h[2] += c; s->h[3] += d;
	s->h[4] += e; s->h[5] += f; s->h[6] += g; s->h[7] += h;
}

static void
shainit(Sha *s)
{
	static uint iv[8] = {
		0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
		0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
	};

	memmove(s->h, iv, sizeof iv);
	s->len = 0;
	s->nbuf = 0;
}

static void
shaupdate(Sha *s, uchar *p, long n)
{
	s->len += n;
	while(n > 0){
		int k = 64 - s->nbuf;
		if(k > n)
			k = n;
		memmove(s->buf + s->nbuf, p, k);
		s->nbuf += k;
		p += k;
		n -= k;
		if(s->nbuf == 64){
			shablock(s, s->buf);
			s->nbuf = 0;
		}
	}
}

static void
shafinal(Sha *s, uchar out[32])
{
	uchar pad[72];
	uvlong bits;
	int i, n;

	bits = s->len * 8;
	n = (s->nbuf < 56) ? 56 - s->nbuf : 120 - s->nbuf;
	memset(pad, 0, sizeof pad);
	pad[0] = 0x80;
	for(i = 0; i < 8; i++)
		pad[n + i] = bits >> (56 - 8*i);
	shaupdate(s, pad, n + 8);
	for(i = 0; i < 8; i++){
		out[4*i] = s->h[i] >> 24;
		out[4*i+1] = s->h[i] >> 16;
		out[4*i+2] = s->h[i] >> 8;
		out[4*i+3] = s->h[i];
	}
}

void
sha256(uchar *p, long n, uchar out[32])
{
	Sha s;

	shainit(&s);
	shaupdate(&s, p, n);
	shafinal(&s, out);
}

static void
tohex(uchar *p, int n, char *out)
{
	static char hex[] = "0123456789abcdef";
	int i;

	for(i = 0; i < n; i++){
		out[2*i] = hex[p[i] >> 4];
		out[2*i+1] = hex[p[i] & 15];
	}
	out[2*n] = 0;
}

/* salted, iterated hash of a password, as 64 hex digits (out is 65 bytes) */
void
usrhash(char *salt, char *pw, char *out)
{
	Sha s;
	uchar h[32];
	int i;

	shainit(&s);
	shaupdate(&s, (uchar*)salt, strlen(salt));
	shaupdate(&s, (uchar*)":", 1);
	shaupdate(&s, (uchar*)pw, strlen(pw));
	shafinal(&s, h);
	for(i = 1; i < Rounds; i++){
		shainit(&s);
		shaupdate(&s, h, 32);
		shaupdate(&s, (uchar*)salt, strlen(salt));
		shaupdate(&s, (uchar*)pw, strlen(pw));
		shafinal(&s, h);
	}
	tohex(h, 32, out);
}

/* ---- /adm/users and /adm/shadow ---- */

static int
validname(char *s)
{
	int n;

	for(n = 0; s[n]; n++)
		if(!((s[n] >= 'a' && s[n] <= 'z') || (s[n] >= 'A' && s[n] <= 'Z') ||
		     (s[n] >= '0' && s[n] <= '9') || s[n] == '_' || s[n] == '-' || s[n] == '.'))
			return 0;
	return n > 0 && n < 28 && s[0] != '.' && s[0] != '-';
}

int
usrvalidname(char *s)
{
	return validname(s);
}

/*
 * Find the line for user `name` in file `path`.  Returns the whole file
 * (malloc'd, NUL-terminated) with *linep set to the start of the line and
 * *endp to just past its newline, or nil if there is no such line.
 */
static char*
findline(char *path, char *name, char **linep, char **endp)
{
	char *s, *p, *nl;
	int n;

	s = slurp(path, nil);
	if(s == nil)
		return nil;
	n = strlen(name);
	for(p = s; *p; p = nl){
		nl = strchr(p, '\n');
		nl = (nl == nil) ? p + strlen(p) : nl + 1;
		if(*p != '#' && strncmp(p, name, n) == 0 && p[n] == ':'){
			*linep = p;
			*endp = nl;
			return s;
		}
	}
	free(s);
	return nil;
}

int
usrlookup(char *name, Usr *u)
{
	char *s, *line, *end, buf[Maxline], *f[5];
	int n;

	if(!validname(name))
		return -1;
	s = findline("/adm/users", name, &line, &end);
	if(s == nil)
		return -1;
	n = end - line;
	if(n >= sizeof buf)
		n = sizeof buf - 1;
	memmove(buf, line, n);
	buf[n] = 0;
	free(s);
	if(n > 0 && buf[n-1] == '\n')
		buf[n-1] = 0;
	if(getfields(buf, f, 5, 0, ":") < 4)
		return -1;
	memset(u, 0, sizeof *u);
	strecpy(u->name, u->name + sizeof u->name, f[0]);
	u->uid = atoi(f[1]);
	strecpy(u->home, u->home + sizeof u->home, f[2]);
	strecpy(u->shell, u->shell + sizeof u->shell, f[3]);
	return 0;
}

int
usrnextuid(void)
{
	char *s, *p, *nl;
	int uid, max;

	max = 999;
	s = slurp("/adm/users", nil);
	if(s == nil)
		return 1000;
	for(p = s; *p; p = nl){
		nl = strchr(p, '\n');
		nl = (nl == nil) ? p + strlen(p) : nl + 1;
		if(*p == '#')
			continue;
		p = strchr(p, ':');
		if(p == nil)
			break;
		uid = atoi(p + 1);
		if(uid >= max && uid < 60000)
			max = uid;
	}
	free(s);
	return max + 1;
}

/* Replace the whole of `path` with `data`; 0 on success. */
static int
putfile(char *path, char *data, int mode)
{
	int fd, n;

	fd = open(path, OWRITE|OTRUNC);
	if(fd < 0)
		fd = create(path, OWRITE, mode);
	if(fd < 0)
		return -1;
	n = strlen(data);
	if(write(fd, data, n) != n){
		close(fd);
		return -1;
	}
	close(fd);
	return 0;
}

/*
 * Replace (or, with line == nil, delete) the line for `name` in `path`; a
 * missing line is appended.  Reads the whole file, so the files are small.
 */
static int
setline(char *path, char *name, char *line, int mode)
{
	char *s, *l, *e, *out;
	long n;
	int r;

	s = findline(path, name, &l, &e);
	if(s == nil){
		s = slurp(path, nil);
		if(s == nil)
			s = strdup("");
		l = e = s + strlen(s);
	}
	n = strlen(s) + (line ? strlen(line) : 0) + 4;
	out = malloc(n);
	if(out == nil){
		free(s);
		return -1;
	}
	n = l - s;
	memmove(out, s, n);
	if(l == e && n > 0 && out[n-1] != '\n')
		out[n++] = '\n';
	if(line)
		n += snprint(out + n, strlen(line) + 2, "%s\n", line);
	strcpy(out + n, e);
	r = putfile(path, out, mode);
	free(out);
	free(s);
	return r;
}

int
usradd(char *name, int uid, char *home, char *shell)
{
	Usr u;
	char line[Maxline];

	if(!validname(name)){
		werrstr("bad user name");
		return -1;
	}
	if(usrlookup(name, &u) == 0){
		werrstr("user exists");
		return -1;
	}
	snprint(line, sizeof line, "%s:%d:%s:%s", name, uid, home, shell);
	return setline("/adm/users", name, line, 0644);
}

/* Does `name` have a password (a line in /adm/shadow)? */
int
usrhaspw(char *name)
{
	char *s, *l, *e;

	s = findline("/adm/shadow", name, &l, &e);
	if(s == nil)
		return 0;
	free(s);
	return 1;
}

int
usrsetpw(char *name, char *pw)
{
	char salt[17], hash[65], line[Maxline];
	uchar seed[32], d[32];
	vlong t;
	int i;

	if(!validname(name)){
		werrstr("bad user name");
		return -1;
	}
	if(pw == nil || *pw == 0)
		return setline("/adm/shadow", name, nil, 0600);	/* no password */
	/*
	 * There is no random device yet; the salt only has to differ from one
	 * account to the next, so mix the clocks, the pid and the name.
	 */
	t = wallnsec() ^ (nsec() << 17) ^ ((vlong)getpid() << 40);
	for(i = 0; i < 8; i++)
		seed[i] = t >> (8*i);
	for(i = 0; i < 24 && name[i]; i++)
		seed[8+i] = name[i];
	sha256(seed, 32, d);
	tohex(d, 8, salt);
	usrhash(salt, pw, hash);
	snprint(line, sizeof line, "%s:%s:%s", name, salt, hash);
	return setline("/adm/shadow", name, line, 0600);
}

/* 0 if `pw` is right for `name`; -1 if not or if the account is locked. */
int
usrcheck(char *name, char *pw)
{
	char *s, *line, *end, buf[Maxline], *f[4], hash[65];
	int n, ok, i;

	if(!validname(name))
		return -1;
	s = findline("/adm/shadow", name, &line, &end);
	if(s == nil)
		return usrlookup(name, &(Usr){0}) == 0 && *pw == 0 ? 0 : -1;
	n = end - line;
	if(n >= sizeof buf)
		n = sizeof buf - 1;
	memmove(buf, line, n);
	buf[n] = 0;
	free(s);
	if(n > 0 && buf[n-1] == '\n')
		buf[n-1] = 0;
	if(getfields(buf, f, 4, 0, ":") < 3 || f[2][0] == '!')
		return -1;
	usrhash(f[1], pw, hash);
	/* compare without stopping at the first difference */
	ok = strlen(hash) == strlen(f[2]);
	for(i = 0; hash[i] && f[2][i]; i++)
		if(hash[i] != f[2][i])
			ok = 0;
	return ok ? 0 : -1;
}
