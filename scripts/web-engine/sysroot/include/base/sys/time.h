#ifndef _ALLOYOS_SYS_TIME_H
#define _ALLOYOS_SYS_TIME_H

#include <sys/types.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

struct timeval {
    time_t tv_sec;
    suseconds_t tv_usec;
};

struct timezone {
    int tz_minuteswest;
    int tz_dsttime;
};

int gettimeofday(struct timeval* tv, void* tz);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_SYS_TIME_H */
