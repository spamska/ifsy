/* Helpers shared by the Ifsy commands (lib/util.c) and Plan 9's ARGBEGIN. */
#ifndef _IFSY_H_
#define _IFSY_H_

#define	ARGBEGIN	for(argv0 = *argv, argv++, argc--;\
			    argv[0] && argv[0][0] == '-' && argv[0][1];\
			    argc--, argv++){\
				char *_args, *_argt;\
				Rune _argc;\
				_args = &argv[0][1];\
				if(_args[0] == '-' && _args[1] == 0){\
					argc--; argv++; break;\
				}\
				_argc = 0;\
				while(*_args && (_args += chartorune(&_argc, _args)))\
				switch(_argc)
#define	ARGEND		SET(_argt); USED(_argt); USED(_argc); USED(_args); }\
			USED(argv); USED(argc);
#define	ARGF()		(_argt = _args, _args = "",\
				(*_argt ? _argt : argv[1] ? (argc--, *++argv) : 0))
#define	EARGF(x)	(_argt = _args, _args = "",\
				(*_argt ? _argt : argv[1] ? (argc--, *++argv) : ((x), abort(), (char*)0)))
#define	ARGC()		_argc

extern char *argv0;

/* Scheduler clock rate: /dev/ps reports CPU time in ticks. */
#define	TICKHZ	100

char*	slurp(char *path, long *np);		/* whole file, NUL-terminated, malloc'd; nil on error */
vlong	wallsecs(void);				/* seconds since 1970 (since boot without an RTC) */
vlong	wallnsec(void);				/* nanoseconds since 1970 (UTC) */
vlong	uptimens(void);				/* nanoseconds since boot */
int	osinfo(char *sys, char *rel, char *ver, char *mach, int n);	/* /dev/osinfo */
char*	sysname(void);				/* static buffer */
void	fmtdate(vlong secs, char *buf, int n);	/* "YYYY-MM-DD HH:MM:SS" (UTC) */
void	civil(vlong secs, int *y, int *mo, int *d, int *h, int *mi, int *s, int *wday);
char*	humansize(uvlong bytes, char *buf, int n);	/* 1023B, 1.5K, 12M, ... */
int	globmatch(char *pat, char *s, int fold);	/* * ? [a-z] [!x] \x */
int	psread(int (*each)(int pid, int ppid, char *state, vlong ticks, char *name, void*), void *arg);
int	fsinfo(vlong *bsize, vlong *blocks, vlong *bfree, vlong *inodes, vlong *ifree, char *label, int n);
int	memstat(vlong *total, vlong *freebytes);	/* /dev/swap */
int	consize(int *cols, int *rows);			/* /dev/consinfo */
int	fsctl(char *op, char *a, char *b);		/* /dev/fsctl: rename, link, symlink */
long	readlink(char *path, char *buf, long n);	/* target of a symbolic link, -1 if not one */
int	dispinfo(vlong v[10]);				/* /dev/display: cols rows w h cellw cellh defcols defrows maxcols maxrows */
int	diskinfo(vlong *size, vlong *max, vlong *freemem);	/* /dev/diskinfo */
int	diskinfo2(vlong *size, vlong *max, vlong *freemem, int *ondisk);

/*
 * Users (lib/9sys/usr.c): /adm/users is "name:uid:home:shell", /adm/shadow is
 * "name:salt:hash".  The process's own user name is getuser() in libc.h.
 */
typedef struct Usr Usr;
struct Usr {
	char	name[32];
	int	uid;
	char	home[128];
	char	shell[128];
};
int	usrlookup(char *name, Usr *u);		/* 0 if /adm/users has the user */
int	usrvalidname(char *name);		/* letters, digits and _-. only */
int	usrnextuid(void);
int	usradd(char *name, int uid, char *home, char *shell);
int	usrhaspw(char *name);
int	usrsetpw(char *name, char *pw);		/* "" removes the password */
int	usrcheck(char *name, char *pw);		/* 0 if pw is right for name */
void	usrhash(char *salt, char *pw, char *hex65);
void	sha256(uchar *p, long n, uchar out[32]);

#endif
