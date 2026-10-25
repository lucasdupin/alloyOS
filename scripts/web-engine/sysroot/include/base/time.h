#ifndef _ALLOYOS_TIME_H
#define _ALLOYOS_TIME_H

#include <stddef.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define CLOCK_REALTIME           0
#define CLOCK_MONOTONIC          1
#define CLOCK_PROCESS_CPUTIME_ID 2
#define CLOCK_THREAD_CPUTIME_ID  3
#define CLOCK_MONOTONIC_RAW      4
#define CLOCK_MONOTONIC_COARSE   6

#include <sys/_timespec.h>

int clock_gettime(clockid_t clock_id, struct timespec* tp);
int clock_getres(clockid_t clock_id, struct timespec* res);
int nanosleep(const struct timespec* req, struct timespec* rem);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_TIME_H */
