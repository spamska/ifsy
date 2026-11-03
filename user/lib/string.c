/* Minimal libString subset (s_new, s_append, s_reset, s_free); see include/String.h. */
#include <u.h>
#include <libc.h>
#include <String.h>

String*
s_new(void)
{
	String *s;

	s = malloc(sizeof *s);
	if(s == nil)
		sysfatal("malloc: %r");
	s->base = malloc(32);
	if(s->base == nil)
		sysfatal("malloc: %r");
	s->end = s->base + 32;
	s->ptr = s->base;
	*s->ptr = 0;
	return s;
}

void
s_free(String *s)
{
	if(s == nil)
		return;
	free(s->base);
	free(s);
}

void
s_reset(String *s)
{
	s->ptr = s->base;
	*s->ptr = 0;
}

String*
s_append(String *s, char *a)
{
	long n, len, cap;
	char *p;

	n = strlen(a);
	len = s->ptr - s->base;
	cap = s->end - s->base;
	if(len + n + 1 > cap){
		while(len + n + 1 > cap)
			cap *= 2;
		p = realloc(s->base, cap);
		if(p == nil)
			sysfatal("malloc: %r");
		s->base = p;
		s->end = p + cap;
		s->ptr = p + len;
	}
	memmove(s->ptr, a, n + 1);
	s->ptr += n;
	return s;
}
