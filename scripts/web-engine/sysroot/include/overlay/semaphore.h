#ifndef _ALLOYOS_SEMAPHORE_H
#define _ALLOYOS_SEMAPHORE_H

#include <pthread.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Process-local counting semaphore built on pthread mutex/condvar. */
typedef struct {
    pthread_mutex_t mutex;
    pthread_cond_t  cond;
    unsigned int    value;
} sem_t;

#define SEM_FAILED ((sem_t*)0)
#define SEM_VALUE_MAX 0x7fffffff

int sem_init(sem_t* sem, int pshared, unsigned int value);
int sem_destroy(sem_t* sem);
int sem_post(sem_t* sem);
int sem_wait(sem_t* sem);
int sem_trywait(sem_t* sem);
int sem_timedwait(sem_t* sem, const struct timespec* abs_timeout);
int sem_getvalue(sem_t* sem, int* sval);

#ifdef __cplusplus
}
#endif
#endif
