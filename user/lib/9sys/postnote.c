#include <u.h>
#include <libc.h>

int
postnote(int group, int pid, char *note)
{
	char buf[ERRMAX+32];
	int f, n, r;

	if(group != PNPROC){
		werrstr("note groups are not supported");
		return -1;
	}
	f = open("/dev/postnote", OWRITE);
	if(f < 0)
		return -1;
	n = snprint(buf, sizeof buf, "%d %s", pid, note);
	r = write(f, buf, n);
	close(f);
	return r == n ? 0 : -1;
}
