#ifndef _SYS_RESOURCE_H_
#define _SYS_RESOURCE_H_

#ifdef __cplusplus
extern "C" {
#endif

#include <sys/time.h>

#define	RUSAGE_SELF	0		/* calling process */
#define	RUSAGE_CHILDREN	-1		/* terminated child processes */
#if __GNU_VISIBLE
#define	RUSAGE_THREAD	1
#endif

struct rusage {
  	struct timeval ru_utime;	/* user time used */
	struct timeval ru_stime;	/* system time used */
	long   ru_maxrss;		/* maximum resident set size */
	long   ru_ixrss;		/* integral shared memory size */
	long   ru_idrss;		/* integral unshared data size */
	long   ru_isrss;		/* integral unshared stack size */
	long   ru_minflt;		/* page reclaims */
	long   ru_majflt;		/* page faults */
	long   ru_nswap;		/* swaps */
	long   ru_inblock;		/* block input operations */
	long   ru_oublock;		/* block output operations */
	long   ru_msgsnd;		/* messages sent */
	long   ru_msgrcv;		/* messages received */
	long   ru_nsignals;		/* signals received */
	long   ru_nvcsw;		/* voluntary context switches */
	long   ru_nivcsw;		/* involuntary context switches */
};

typedef unsigned long rlim_t;
#define RLIM_INFINITY (~0UL)

struct rlimit {
    rlim_t rlim_cur;
    rlim_t rlim_max;
};

#define RLIMIT_CPU      0
#define RLIMIT_FSIZE    1
#define RLIMIT_DATA     2
#define RLIMIT_STACK    3
#define RLIMIT_CORE     4
#define RLIMIT_RSS      5
#define RLIMIT_NOFILE   7
#define RLIMIT_AS       9

int getrlimit(int resource, struct rlimit* rlp);
int setrlimit(int resource, const struct rlimit* rlp);
int	getrusage (int, struct rusage*);

#ifdef __cplusplus
}
#endif
#endif /* !_SYS_RESOURCE_H_ */

