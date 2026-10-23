#ifndef _SCHED_H
#define _SCHED_H

#ifndef _SYS_SCHED_H_
#define _SYS_SCHED_H_
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define SCHED_OTHER 0
#define SCHED_FIFO  1
#define SCHED_RR    2

struct sched_param {
    int sched_priority;
};

int sched_yield(void);

#ifdef __cplusplus
}
#endif

#endif /* _SCHED_H */
