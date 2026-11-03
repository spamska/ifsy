#include <u.h>
#include <libc.h>
#include <fcall.h>

/*
 * Ifsy: the kernel's wait string is "pid utime stime rtime msg"; msg is
 * empty after exits(0) and may contain blanks ("sys: trap: ..."), so it is
 * parsed by hand instead of with tokenize, which would give up on both.
 */
Waitmsg*
wait(void)
{
	int n, l, i;
	char buf[512], *fld[4], *p, *msg;
	Waitmsg *w;

	n = await(buf, sizeof buf-1);
	if(n < 0)
		return nil;
	buf[n] = '\0';
	p = buf;
	for(i = 0; i < 4; i++){
		while(*p == ' ')
			p++;
		fld[i] = p;
		while(*p != '\0' && *p != ' ')
			p++;
		if(*p != '\0')
			*p++ = '\0';
	}
	if(fld[3][0] == '\0'){
		werrstr("couldn't parse wait message");
		return nil;
	}
	while(*p == ' ')
		p++;
	msg = p;
	l = strlen(msg);
	while(l > 0 && (msg[l-1] == '\n' || msg[l-1] == ' '))
		msg[--l] = '\0';
	l++;
	w = malloc(sizeof(Waitmsg)+l);
	if(w == nil)
		return nil;
	w->pid = atoi(fld[0]);
	w->time[0] = atoi(fld[1]);
	w->time[1] = atoi(fld[2]);
	w->time[2] = atoi(fld[3]);
	w->msg = (char*)&w[1];
	memmove(w->msg, msg, l);
	return w;
}
