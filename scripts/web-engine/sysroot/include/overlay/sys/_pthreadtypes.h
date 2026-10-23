#ifndef _SYS__PTHREADTYPES_H_
#define _SYS__PTHREADTYPES_H_

#include <stddef.h>
#include <stdint.h>
#include <sched.h>

#ifdef __cplusplus
extern "C" {
#endif

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

#ifdef __cplusplus
}
#endif

#endif /* _SYS__PTHREADTYPES_H_ */
