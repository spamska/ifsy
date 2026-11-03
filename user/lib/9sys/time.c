#include <u.h>
#include <libc.h>
extern vlong wallsecs(void);	/* lib/util.c */

long
time(long *tp)
{
	vlong t;

	t = wallsecs();
	if(tp != nil)
		*tp = t;
	return t;
}
