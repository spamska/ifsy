/* Minimal stand-in for 9front's libString: just what tar needs. */
#ifndef _STRING_H_
#define _STRING_H_
typedef struct String {
	char	*base, *end, *ptr;
} String;
extern String*	s_new(void);
extern void	s_free(String*);
extern String*	s_append(String*, char*);
extern void	s_reset(String*);
#define s_to_c(s)	((s)->base)
#endif
