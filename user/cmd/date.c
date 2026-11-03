#include <u.h>
#include <libc.h>
extern vlong wallnsec(void);	/* lib/util.c */

enum {
	Nsec = 1000*1000*1000,
};

void
usage(void)
{
	fprint(2, "usage: date [-itun] [-f fmt] [seconds]\n");
	exits("usage");
}

void
main(int argc, char *argv[])
{
	int nflg, uflg;
	char *fmt;
	vlong s, ns;
	Tzone *tz;
	Tm tm;

	nflg = 0;
	uflg = 0;
	tz = nil;
	fmt =  "WW MMM _D hh:mm:ss ZZZ YYYY";
	tmfmtinstall();

	ARGBEGIN{
	case 'n':	nflg = 1;				break;
	case 'u':	uflg = 1;				break;
	case 't':	fmt = "YYYY-MM-DDThh:mm:ssZZ";		break;
	case 'i':	fmt = "YYYY-MM-DD";			break;
	case 'f':	fmt = EARGF(usage());			break;
	default:	usage();
	}ARGEND

	s = 0;
	ns = 0;
	switch(argc) {
	case 0:
		ns = wallnsec();	/* Ifsy: nsec() is time since boot */
		s = ns/Nsec;
		ns = ns%Nsec;
		break;
	case 1:
		s = strtoll(argv[0], nil, 0);
		ns = 0;
		break;
	default:
		usage();
		break;
	}

	/*
	 * Ifsy: UTC is the default.  A different zone is used only when
	 * /adm/timezone/local exists and can be read; otherwise tz stays nil,
	 * which the date library treats as GMT.
	 */
	if(!uflg)
		tz = tzload("local");
	if(tmtimens(&tm, s, ns, tz) == nil)
		sysfatal("now: %r");

	if(nflg)
		print("%lld\n", tmnorm(&tm));
	else
		if(print("%τ\n", tmfmt(&tm, fmt)) == -1)
			sysfatal("%r");
	exits(0);
}
