/* The small C runtime Ifsy adds to the borrowed 9front libc: process
 * start, a heap, and the few calls that need the kernel's own conventions. */
#include <u.h>
#include <libc.h>

char *argv0 __attribute__((weak));
extern int main(int, char**);

void
_main(int argc, char **argv)
{
	argv0 = argc > 0 ? argv[0] : "?";
	main(argc, argv);
	exits(0);
}

/* ---- heap: power-of-two size classes carved from the brk segment ---- */

typedef struct Hdr Hdr;
struct Hdr {
	ulong	cls;		/* size class: block holds 1<<cls bytes after the header */
	ulong	pad;
};
#define HDR	sizeof(Hdr)
#define NCLS	48

static Hdr *freelist[NCLS];
static uchar *arena, *arenaend;

void*
sbrk(usize n)
{
	uvlong cur, want, got;

	cur = (uvlong)segbrk(0, 0);
	if(n == 0)
		return (void*)cur;
	want = cur + n;
	got = (uvlong)segbrk(0, (void*)want);
	if(got < want)
		return (void*)-1;
	return (void*)cur;
}

static int
sizeclass(ulong n)
{
	int c;

	c = 4;
	while(((ulong)1 << c) < n)
		c++;
	return c;
}

void*
malloc(ulong n)
{
	int c;
	Hdr *h;
	ulong need;
	uchar *p;

	if(n == 0)
		n = 1;
	c = sizeclass(n);
	if(c >= NCLS)
		return nil;
	h = freelist[c];
	if(h != nil){
		freelist[c] = *(Hdr**)(h+1);
		return h+1;
	}
	need = HDR + ((ulong)1 << c);
	if(arena == nil || (ulong)(arenaend - arena) < need){
		ulong grab = need < 65536 ? 65536 : (need + 4095) & ~(ulong)4095;
		p = sbrk(grab);
		if(p == (uchar*)-1)
			return nil;
		if(arena != nil && p == arenaend){
			arenaend = p + grab;
		}else{
			arena = p;
			arenaend = p + grab;
		}
	}
	h = (Hdr*)arena;
	arena += need;
	h->cls = c;
	return h+1;
}

void*
mallocz(ulong n, int clr)
{
	void *p;

	p = malloc(n);
	if(p != nil && clr)
		memset(p, 0, n);
	return p;
}

void
free(void *v)
{
	Hdr *h;

	if(v == nil)
		return;
	h = (Hdr*)v - 1;
	*(Hdr**)v = freelist[h->cls];
	freelist[h->cls] = h;
}

void*
realloc(void *v, ulong n)
{
	Hdr *h;
	void *q;
	ulong have;

	if(v == nil)
		return malloc(n);
	h = (Hdr*)v - 1;
	have = (ulong)1 << h->cls;
	if(n <= have)
		return v;
	q = malloc(n);
	if(q == nil)
		return nil;
	memmove(q, v, have);
	free(v);
	return q;
}

void*	malloctopoolblock(void *v){ return v; }
void	setmalloctag(void *v, uintptr t){ USED(v); USED(t); }
void	setrealloctag(void *v, uintptr t){ USED(v); USED(t); }
uintptr	getmalloctag(void *v){ USED(v); return 0; }
uintptr	getrealloctag(void *v){ USED(v); return 0; }

/* ---- odds and ends ---- */

void
abort(void)
{
	exits("abort");
}

/* The kernel has no per-process page with the pid in it (Plan 9's Tos), so
 * the pid comes from /dev/pid. Not cached: a forked child must see its own. */
int
getpid(void)
{
	char b[16];
	int fd, n;

	fd = open("/dev/pid", OREAD);
	if(fd < 0)
		return 0;
	n = pread(fd, b, sizeof b - 1, 0);
	close(fd);
	if(n <= 0)
		return 0;
	b[n] = 0;
	return atoi(b);
}

int
getppid(void)
{
	return 0;
}

void
_exits_unused(void)
{
}

/* One CPU, no preemption inside a process: locks have nothing to protect. */
void	lock(Lock *l){ l->val = 1; }
void	unlock(Lock *l){ l->val = 0; }
int	canlock(Lock *l){ if(l->val) return 0; l->val = 1; return 1; }
