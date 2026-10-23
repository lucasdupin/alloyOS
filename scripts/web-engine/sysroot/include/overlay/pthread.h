#ifndef _PTHREAD_H
#define _PTHREAD_H

#include <stddef.h>
#include <stdint.h>
#include <sched.h>
#include <time.h>
#include <sys/_pthreadtypes.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PTHREAD_CREATE_JOINABLE       0
#define PTHREAD_CREATE_DETACHED       1

#define PTHREAD_MUTEX_NORMAL          0
#define PTHREAD_MUTEX_RECURSIVE       1
#define PTHREAD_MUTEX_ERRORCHECK      2
#define PTHREAD_MUTEX_DEFAULT         PTHREAD_MUTEX_NORMAL

#define PTHREAD_ONCE_INIT             { 0U }
#define PTHREAD_MUTEX_INITIALIZER     { 0U, PTHREAD_MUTEX_NORMAL, 0ULL, 0U, 0U }
#define PTHREAD_COND_INITIALIZER      { 0U, 1 }

#define PTHREAD_KEYS_MAX              64
#define PTHREAD_DESTRUCTOR_ITERATIONS 4
#define PTHREAD_STACK_MIN             16384UL

#ifndef _SYS__PTHREADTYPES_H_
typedef uintptr_t pthread_t;
typedef uint32_t pthread_key_t;

typedef struct {
    size_t stack_size;
    size_t guard_size;
    void* stack_addr;
    int detach_state;
} pthread_attr_t;

typedef struct {
    int type;
} pthread_mutexattr_t;

typedef struct {
    uint32_t state;
    int32_t type;
    uint64_t owner_tid;
    uint32_t recursion;
    uint32_t reserved;
} pthread_mutex_t;

typedef struct {
    int clock_id;
} pthread_condattr_t;

typedef struct {
    uint32_t seq;
    int32_t clock_id;
} pthread_cond_t;

typedef struct {
    uint32_t state;
} pthread_once_t;
#endif

/* Thread lifecycle */
int pthread_create(pthread_t* thread, const pthread_attr_t* attr,
                   void* (*start_routine)(void*), void* arg);
int pthread_join(pthread_t thread, void** retval);
int pthread_detach(pthread_t thread);
pthread_t pthread_self(void);
int pthread_equal(pthread_t t1, pthread_t t2);
__attribute__((noreturn)) void pthread_exit(void* retval);

/* Thread attributes */
int pthread_attr_init(pthread_attr_t* attr);
int pthread_attr_destroy(pthread_attr_t* attr);
int pthread_attr_setstacksize(pthread_attr_t* attr, size_t stacksize);
int pthread_attr_getstacksize(const pthread_attr_t* attr, size_t* stacksize);
int pthread_attr_setguardsize(pthread_attr_t* attr, size_t guardsize);
int pthread_attr_getguardsize(const pthread_attr_t* attr, size_t* guardsize);
int pthread_attr_setdetachstate(pthread_attr_t* attr, int detachstate);
int pthread_attr_getdetachstate(const pthread_attr_t* attr, int* detachstate);
int pthread_attr_getstack(const pthread_attr_t* attr, void** stackaddr, size_t* stacksize);
int pthread_getattr_np(pthread_t thread, pthread_attr_t* attr);
int pthread_attr_setstack(pthread_attr_t* attr, void* stackaddr, size_t stacksize);
int pthread_getschedparam(pthread_t thread, int* policy, struct sched_param* param);
int pthread_setschedparam(pthread_t thread, int policy, const struct sched_param* param);

/* Mutexes */
int pthread_mutexattr_init(pthread_mutexattr_t* attr);
int pthread_mutexattr_destroy(pthread_mutexattr_t* attr);
int pthread_mutexattr_settype(pthread_mutexattr_t* attr, int type);
int pthread_mutexattr_gettype(const pthread_mutexattr_t* attr, int* type);

int pthread_mutex_init(pthread_mutex_t* mutex, const pthread_mutexattr_t* attr);
int pthread_mutex_lock(pthread_mutex_t* mutex);
int pthread_mutex_trylock(pthread_mutex_t* mutex);
int pthread_mutex_unlock(pthread_mutex_t* mutex);
int pthread_mutex_destroy(pthread_mutex_t* mutex);

/* Condition variables */
int pthread_condattr_init(pthread_condattr_t* attr);
int pthread_condattr_destroy(pthread_condattr_t* attr);
int pthread_condattr_setclock(pthread_condattr_t* attr, int clock_id);
int pthread_condattr_getclock(const pthread_condattr_t* attr, int* clock_id);

int pthread_cond_init(pthread_cond_t* cond, const pthread_condattr_t* attr);
int pthread_cond_wait(pthread_cond_t* cond, pthread_mutex_t* mutex);
int pthread_cond_timedwait(pthread_cond_t* cond, pthread_mutex_t* mutex,
                           const struct timespec* abstime);
int pthread_cond_signal(pthread_cond_t* cond);
int pthread_cond_broadcast(pthread_cond_t* cond);
int pthread_cond_destroy(pthread_cond_t* cond);

/* One-time initialization */
int pthread_once(pthread_once_t* once_control, void (*init_routine)(void));

/* Thread-specific data keys */
int pthread_key_create(pthread_key_t* key, void (*destructor)(void*));
int pthread_key_delete(pthread_key_t key);
void* pthread_getspecific(pthread_key_t key);
int pthread_setspecific(pthread_key_t key, const void* value);

#ifdef __cplusplus
}
#endif

#endif /* _PTHREAD_H */
