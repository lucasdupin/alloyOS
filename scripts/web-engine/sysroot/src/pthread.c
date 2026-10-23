#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <alloyos/syscall.h>

extern const unsigned char __alloyos_tdata_start[] __attribute__((weak));
extern const unsigned char __alloyos_tdata_end[] __attribute__((weak));
extern const unsigned char __alloyos_tbss_start[] __attribute__((weak));
extern const unsigned char __alloyos_tbss_end[] __attribute__((weak));
extern const unsigned char __alloyos_tls_align[] __attribute__((weak));
extern _Thread_local uint64_t __alloyos_tls_anchor;

#define ALLOYOS_DEFAULT_STACK_SIZE (16UL * ALLOYOS_PAGE_SIZE) /* 64 KiB */
#define ALLOYOS_DEFAULT_GUARD_SIZE ALLOYOS_PAGE_SIZE          /* 4 KiB */
#define ALLOYOS_MAX_THREAD_SLOTS   16

struct alloyos_thread_record {
    uint32_t in_use;
    uint32_t detached;
    uint32_t done_word;
    uint32_t joined;
    uint64_t tid;
    void* (*start_routine)(void*);
    void* arg;
    void* retval;
    uintptr_t reservation_base;
    size_t reservation_size;
    uintptr_t stack_base;
    size_t stack_size;
    size_t guard_size;
    uintptr_t tls_base;
    size_t tls_size;
};

struct alloyos_tls_state {
    struct alloyos_thread_record* self;
    void* key_values[PTHREAD_KEYS_MAX];
    uint32_t key_generations[PTHREAD_KEYS_MAX];
};

static _Thread_local struct alloyos_tls_state g_tls_state;
static struct alloyos_thread_record g_main_thread_record;
static struct alloyos_thread_record g_thread_table[ALLOYOS_MAX_THREAD_SLOTS];
static uint32_t g_thread_table_lock = 0;

struct alloyos_key_Slot {
    uint32_t in_use;
    uint32_t generation;
    void (*destructor)(void*);
};

static struct alloyos_key_Slot g_keys[PTHREAD_KEYS_MAX];
static uint32_t g_key_lock = 0;

static void spin_lock(uint32_t* lock)
{
    uint32_t expected = 0;
    while (!__atomic_compare_exchange_n(lock, &expected, 1U,
                                        1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        expected = 0;
        (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
    }
}

static void spin_unlock(uint32_t* lock)
{
    __atomic_store_n(lock, 0U, __ATOMIC_RELEASE);
}

static uint64_t query_current_tid(void)
{
    int64_t tid = alloyos_syscall0(ALLOYOS_SYS_GETTID);
    if (tid > 0) {
        return (uint64_t)tid;
    }
    int64_t pid = alloyos_syscall0(ALLOYOS_SYS_GETPID);
    return pid > 0 ? (uint64_t)pid : 1ULL;
}

static int64_t wait_word_Wrapper(uint32_t* word, uint32_t expected, uint64_t deadline_ns)
{
    int64_t rc = alloyos_syscall3(ALLOYOS_SYS_WORD_WAIT,
                                  (uint64_t)(uintptr_t)word,
                                  (uint64_t)expected,
                                  deadline_ns);
    if (rc == -1) {
        /* If native threads are not enabled on this task, yield cooperatively */
        (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
        return 0;
    }
    return rc;
}

static void wake_word_Wrapper(uint32_t* word, uint32_t count)
{
    (void)alloyos_syscall3(ALLOYOS_SYS_WORD_WAKE,
                           (uint64_t)(uintptr_t)word,
                           (uint64_t)count,
                           0ULL);
}

void __alloyos_pthread_init_main(void)
{
    if (g_tls_state.self != NULL) {
        return;
    }
    g_main_thread_record.in_use = 1U;
    g_main_thread_record.detached = 0U;
    g_main_thread_record.done_word = 0U;
    g_main_thread_record.joined = 0U;
    g_main_thread_record.tid = query_current_tid();
    g_main_thread_record.tls_base = alloyos_read_tpidr_el0();
    g_tls_state.self = &g_main_thread_record;
}

static struct alloyos_thread_record* current_thread_record(void)
{
    if (g_tls_state.self == NULL) {
        __alloyos_pthread_init_main();
    }
    return g_tls_state.self;
}

int sched_yield(void)
{
    (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
    return 0;
}

pthread_t pthread_self(void)
{
    return (pthread_t)(uintptr_t)current_thread_record();
}

int pthread_equal(pthread_t t1, pthread_t t2)
{
    return t1 == t2 ? 1 : 0;
}

static void run_thread_key_destructors(void)
{
    for (int pass = 0; pass < PTHREAD_DESTRUCTOR_ITERATIONS; ++pass) {
        int any_called = 0;
        for (uint32_t k = 0; k < PTHREAD_KEYS_MAX; ++k) {
            void* val = g_tls_state.key_values[k];
            if (!val) {
                continue;
            }
            spin_lock(&g_key_lock);
            uint32_t in_use = g_keys[k].in_use;
            uint32_t gen = g_keys[k].generation;
            void (*dtor)(void*) = g_keys[k].destructor;
            spin_unlock(&g_key_lock);

            g_tls_state.key_values[k] = NULL;
            if (in_use && dtor && g_tls_state.key_generations[k] == gen) {
                any_called = 1;
                dtor(val);
            }
        }
        if (!any_called) {
            break;
        }
    }
}

__attribute__((noreturn)) void pthread_exit(void* retval)
{
    struct alloyos_thread_record* self = current_thread_record();
    run_thread_key_destructors();
    if (self && self != &g_main_thread_record) {
        self->retval = retval;
        __atomic_store_n(&self->done_word, 1U, __ATOMIC_RELEASE);
        wake_word_Wrapper(&self->done_word, 0x7fffffffU);
    }
    (void)alloyos_syscall1(ALLOYOS_SYS_THREAD_EXIT, 0ULL);
    for (;;) {
        __asm__ volatile("yield");
    }
}

static __attribute__((noreturn, used)) void alloyos_thread_trampoline(void* raw_arg)
{
    struct alloyos_thread_record* rec = (struct alloyos_thread_record*)raw_arg;
    memset(&g_tls_state, 0, sizeof(g_tls_state));
    g_tls_state.self = rec;
    if (__atomic_load_n(&rec->tid, __ATOMIC_ACQUIRE) == 0ULL) {
        __atomic_store_n(&rec->tid, query_current_tid(), __ATOMIC_RELEASE);
    }
    void* result = rec->start_routine(rec->arg);
    pthread_exit(result);
}

static void initialize_variant1_tls_block(uintptr_t tls_base, size_t tls_size)
{
    memset((void*)tls_base, 0, tls_size);
    uintptr_t align = (uintptr_t)__alloyos_tls_align;
    if (align < 64U || (align & (align - 1U)) != 0U || align > ALLOYOS_PAGE_SIZE) {
        align = 64U;
    }
    size_t tcb_offset = ((16U + align - 1U) / align) * align;
    if (__alloyos_tdata_start && __alloyos_tdata_end && __alloyos_tdata_end >= __alloyos_tdata_start) {
        size_t filesz = (size_t)(__alloyos_tdata_end - __alloyos_tdata_start);
        if (tcb_offset + filesz <= tls_size) {
            memcpy((void*)(tls_base + tcb_offset), __alloyos_tdata_start, filesz);
        }
    } else {
        /* Fallback when linked without __alloyos_tdata_start symbol */
        if (tcb_offset + sizeof(uint64_t) <= tls_size) {
            uint64_t anchor = 0x414c4c4f594f5331ULL;
            memcpy((void*)(tls_base + tcb_offset), &anchor, sizeof(anchor));
        }
    }
}

static size_t compute_tls_allocation_size(void)
{
    uintptr_t align = (uintptr_t)__alloyos_tls_align;
    if (align < 64U || (align & (align - 1U)) != 0U || align > ALLOYOS_PAGE_SIZE) {
        align = 64U;
    }
    size_t tcb_offset = ((16U + align - 1U) / align) * align;
    size_t memsz = sizeof(struct alloyos_tls_state) + 256U;
    if (__alloyos_tdata_start && __alloyos_tbss_end && __alloyos_tbss_end >= __alloyos_tdata_start) {
        size_t linker_memsz = (size_t)(__alloyos_tbss_end - __alloyos_tdata_start);
        if (linker_memsz > memsz) {
            memsz = linker_memsz;
        }
    }
    size_t total = tcb_offset + memsz;
    return (total + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
}

int pthread_create(pthread_t* thread, const pthread_attr_t* attr,
                   void* (*start_routine)(void*), void* arg)
{
    if (!thread || !start_routine) {
        return EINVAL;
    }
    (void)current_thread_record();

    size_t stack_size = ALLOYOS_DEFAULT_STACK_SIZE;
    size_t guard_size = ALLOYOS_DEFAULT_GUARD_SIZE;
    int detach_state = PTHREAD_CREATE_JOINABLE;
    if (attr) {
        if (attr->stack_size != 0U) {
            stack_size = attr->stack_size;
        }
        guard_size = attr->guard_size;
        detach_state = attr->detach_state;
    }
    if (stack_size < PTHREAD_STACK_MIN) {
        stack_size = PTHREAD_STACK_MIN;
    }
    stack_size = (stack_size + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    guard_size = (guard_size + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    size_t tls_size = compute_tls_allocation_size();

    if (stack_size > SIZE_MAX - guard_size || (stack_size + guard_size) > SIZE_MAX - tls_size) {
        return EINVAL;
    }
    size_t reservation_size = guard_size + stack_size + tls_size;

    spin_lock(&g_thread_table_lock);
    struct alloyos_thread_record* rec = NULL;
    for (size_t i = 0; i < ALLOYOS_MAX_THREAD_SLOTS; ++i) {
        if (!g_thread_table[i].in_use) {
            rec = &g_thread_table[i];
            memset(rec, 0, sizeof(*rec));
            rec->in_use = 1U;
            break;
        }
    }
    spin_unlock(&g_thread_table_lock);
    if (!rec) {
        return EAGAIN;
    }

    int64_t res_base = alloyos_syscall1(ALLOYOS_SYS_MEMORY_RESERVE, (uint64_t)reservation_size);
    if (res_base < 0) {
        spin_lock(&g_thread_table_lock);
        rec->in_use = 0U;
        spin_unlock(&g_thread_table_lock);
        return __alloyos_map_kernel_errno(res_base);
    }

    uintptr_t commit_base = (uintptr_t)res_base + guard_size;
    size_t commit_len = stack_size + tls_size;
    int64_t commit_rc = alloyos_syscall3(ALLOYOS_SYS_MEMORY_COMMIT,
                                         (uint64_t)commit_base,
                                         (uint64_t)commit_len,
                                         ALLOYOS_MEMORY_READ_WRITE);
    if (commit_rc < 0) {
        (void)alloyos_syscall2(ALLOYOS_SYS_MEMORY_RELEASE,
                               (uint64_t)res_base,
                               (uint64_t)reservation_size);
        spin_lock(&g_thread_table_lock);
        rec->in_use = 0U;
        spin_unlock(&g_thread_table_lock);
        return __alloyos_map_kernel_errno(commit_rc);
    }

    rec->detached = (detach_state == PTHREAD_CREATE_DETACHED) ? 1U : 0U;
    rec->done_word = 0U;
    rec->joined = 0U;
    rec->tid = 0ULL;
    rec->start_routine = start_routine;
    rec->arg = arg;
    rec->retval = NULL;
    rec->reservation_base = (uintptr_t)res_base;
    rec->reservation_size = reservation_size;
    rec->stack_base = commit_base;
    rec->stack_size = stack_size;
    rec->guard_size = guard_size;
    rec->tls_base = commit_base + stack_size;
    rec->tls_size = tls_size;

    initialize_variant1_tls_block(rec->tls_base, rec->tls_size);

    uint64_t desc[6];
    desc[0] = (uint64_t)(uintptr_t)&alloyos_thread_trampoline;
    desc[1] = (uint64_t)(uintptr_t)rec;
    desc[2] = (uint64_t)rec->stack_base;
    desc[3] = (uint64_t)rec->stack_size;
    desc[4] = (uint64_t)rec->tls_base;
    desc[5] = 0ULL;

    int64_t tid = alloyos_syscall3(ALLOYOS_SYS_THREAD_CREATE,
                                   (uint64_t)(uintptr_t)desc,
                                   ALLOYOS_THREAD_DESC_BYTES,
                                   0ULL);
    if (tid < 0) {
        (void)alloyos_syscall2(ALLOYOS_SYS_MEMORY_RELEASE,
                               (uint64_t)rec->reservation_base,
                               (uint64_t)rec->reservation_size);
        spin_lock(&g_thread_table_lock);
        rec->in_use = 0U;
        spin_unlock(&g_thread_table_lock);
        return __alloyos_map_kernel_errno(tid);
    }

    __atomic_store_n(&rec->tid, (uint64_t)tid, __ATOMIC_RELEASE);
    *thread = (pthread_t)(uintptr_t)rec;
    return 0;
}

int pthread_join(pthread_t thread, void** retval)
{
    if (!thread) {
        return EINVAL;
    }
    struct alloyos_thread_record* rec = (struct alloyos_thread_record*)(uintptr_t)thread;
    struct alloyos_thread_record* self = current_thread_record();
    if (rec == self) {
        return EDEADLK;
    }
    if (rec == &g_main_thread_record) {
        return EINVAL;
    }

    spin_lock(&g_thread_table_lock);
    if (!rec->in_use || rec->detached || rec->joined) {
        spin_unlock(&g_thread_table_lock);
        return EINVAL;
    }
    rec->joined = 1U;
    spin_unlock(&g_thread_table_lock);

    while (__atomic_load_n(&rec->done_word, __ATOMIC_ACQUIRE) == 0U) {
        (void)wait_word_Wrapper(&rec->done_word, 0U, 0ULL);
    }

    uint64_t child_tid = __atomic_load_n(&rec->tid, __ATOMIC_ACQUIRE);
    int32_t kernel_exit_code = 0;
    for (;;) {
        int64_t join_rc = alloyos_syscall3(ALLOYOS_SYS_THREAD_JOIN,
                                           child_tid,
                                           (uint64_t)(uintptr_t)&kernel_exit_code,
                                           0ULL);
        if (join_rc == 0) {
            break;
        }
        if (join_rc == -11) {
            /* Child entered THREAD_EXIT; yield so scheduler retires its context */
            (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
            continue;
        }
        return __alloyos_map_kernel_errno(join_rc);
    }

    if (retval) {
        *retval = rec->retval;
    }

    uintptr_t res_base = rec->reservation_base;
    size_t res_size = rec->reservation_size;

    spin_lock(&g_thread_table_lock);
    memset(rec, 0, sizeof(*rec));
    spin_unlock(&g_thread_table_lock);

    if (res_base != 0U && res_size != 0U) {
        (void)alloyos_syscall2(ALLOYOS_SYS_MEMORY_RELEASE,
                               (uint64_t)res_base,
                               (uint64_t)res_size);
    }
    return 0;
}

int pthread_detach(pthread_t thread)
{
    if (!thread) {
        return EINVAL;
    }
    struct alloyos_thread_record* rec = (struct alloyos_thread_record*)(uintptr_t)thread;
    if (rec == &g_main_thread_record) {
        return EINVAL;
    }
    spin_lock(&g_thread_table_lock);
    if (!rec->in_use || rec->detached || rec->joined) {
        spin_unlock(&g_thread_table_lock);
        return EINVAL;
    }
    rec->detached = 1U;
    spin_unlock(&g_thread_table_lock);
    return 0;
}

/* Thread attributes */
int pthread_attr_init(pthread_attr_t* attr)
{
    if (!attr) {
        return EINVAL;
    }
    attr->stack_size = ALLOYOS_DEFAULT_STACK_SIZE;
    attr->guard_size = ALLOYOS_DEFAULT_GUARD_SIZE;
    attr->stack_addr = NULL;
    attr->detach_state = PTHREAD_CREATE_JOINABLE;
    return 0;
}

int pthread_attr_destroy(pthread_attr_t* attr)
{
    if (!attr) {
        return EINVAL;
    }
    return 0;
}

int pthread_attr_setstacksize(pthread_attr_t* attr, size_t stacksize)
{
    if (!attr || stacksize < PTHREAD_STACK_MIN) {
        return EINVAL;
    }
    attr->stack_size = (stacksize + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    return 0;
}

int pthread_attr_getstacksize(const pthread_attr_t* attr, size_t* stacksize)
{
    if (!attr || !stacksize) {
        return EINVAL;
    }
    *stacksize = attr->stack_size;
    return 0;
}

int pthread_attr_setguardsize(pthread_attr_t* attr, size_t guardsize)
{
    if (!attr) {
        return EINVAL;
    }
    attr->guard_size = (guardsize + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    return 0;
}

int pthread_attr_getguardsize(const pthread_attr_t* attr, size_t* guardsize)
{
    if (!attr || !guardsize) {
        return EINVAL;
    }
    *guardsize = attr->guard_size;
    return 0;
}

int pthread_attr_setdetachstate(pthread_attr_t* attr, int detachstate)
{
    if (!attr || (detachstate != PTHREAD_CREATE_JOINABLE && detachstate != PTHREAD_CREATE_DETACHED)) {
        return EINVAL;
    }
    attr->detach_state = detachstate;
    return 0;
}

int pthread_attr_getdetachstate(const pthread_attr_t* attr, int* detachstate)
{
    if (!attr || !detachstate) {
        return EINVAL;
    }
    *detachstate = attr->detach_state;
    return 0;
}

int pthread_attr_getstack(const pthread_attr_t* attr, void** stackaddr, size_t* stacksize)
{
    if (!attr || !stackaddr || !stacksize) {
        return EINVAL;
    }
    *stackaddr = attr->stack_addr;
    *stacksize = attr->stack_size;
    return 0;
}

int pthread_getattr_np(pthread_t thread, pthread_attr_t* attr)
{
    if (!thread || !attr) {
        return EINVAL;
    }
    struct alloyos_thread_record* rec = (struct alloyos_thread_record*)(uintptr_t)thread;
    attr->stack_addr = (void*)rec->stack_base;
    attr->stack_size = rec->stack_size ? rec->stack_size : ALLOYOS_DEFAULT_STACK_SIZE;
    attr->guard_size = rec->guard_size;
    attr->detach_state = rec->detached ? PTHREAD_CREATE_DETACHED : PTHREAD_CREATE_JOINABLE;
    return 0;
}

/* Mutexes */
int pthread_mutexattr_init(pthread_mutexattr_t* attr)
{
    if (!attr) {
        return EINVAL;
    }
    attr->type = PTHREAD_MUTEX_NORMAL;
    return 0;
}

int pthread_mutexattr_destroy(pthread_mutexattr_t* attr)
{
    return attr ? 0 : EINVAL;
}

int pthread_mutexattr_settype(pthread_mutexattr_t* attr, int type)
{
    if (!attr || (type != PTHREAD_MUTEX_NORMAL &&
                  type != PTHREAD_MUTEX_RECURSIVE &&
                  type != PTHREAD_MUTEX_ERRORCHECK)) {
        return EINVAL;
    }
    attr->type = type;
    return 0;
}

int pthread_mutexattr_gettype(const pthread_mutexattr_t* attr, int* type)
{
    if (!attr || !type) {
        return EINVAL;
    }
    *type = attr->type;
    return 0;
}

int pthread_mutex_init(pthread_mutex_t* mutex, const pthread_mutexattr_t* attr)
{
    if (!mutex) {
        return EINVAL;
    }
    mutex->state = 0U;
    mutex->type = attr ? attr->type : PTHREAD_MUTEX_NORMAL;
    mutex->owner_tid = 0ULL;
    mutex->recursion = 0U;
    mutex->reserved = 0U;
    return 0;
}

int pthread_mutex_lock(pthread_mutex_t* mutex)
{
    if (!mutex) {
        return EINVAL;
    }
    uint64_t self_tid = current_thread_record()->tid;
    uint32_t expected = 0U;
    if (__atomic_compare_exchange_n(&mutex->state, &expected, 1U,
                                    0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        __atomic_store_n(&mutex->owner_tid, self_tid, __ATOMIC_RELAXED);
        mutex->recursion = 1U;
        return 0;
    }

    if (__atomic_load_n(&mutex->owner_tid, __ATOMIC_RELAXED) == self_tid) {
        if (mutex->type == PTHREAD_MUTEX_RECURSIVE) {
            if (mutex->recursion == UINT32_MAX) {
                return EAGAIN;
            }
            mutex->recursion += 1U;
            return 0;
        }
        if (mutex->type == PTHREAD_MUTEX_ERRORCHECK) {
            return EDEADLK;
        }
    }

    for (;;) {
        uint32_t prev = __atomic_exchange_n(&mutex->state, 2U, __ATOMIC_ACQUIRE);
        if (prev == 0U) {
            __atomic_store_n(&mutex->owner_tid, self_tid, __ATOMIC_RELAXED);
            mutex->recursion = 1U;
            return 0;
        }
        (void)wait_word_Wrapper(&mutex->state, 2U, 0ULL);
    }
}

int pthread_mutex_trylock(pthread_mutex_t* mutex)
{
    if (!mutex) {
        return EINVAL;
    }
    uint64_t self_tid = current_thread_record()->tid;
    uint32_t expected = 0U;
    if (__atomic_compare_exchange_n(&mutex->state, &expected, 1U,
                                    0, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        __atomic_store_n(&mutex->owner_tid, self_tid, __ATOMIC_RELAXED);
        mutex->recursion = 1U;
        return 0;
    }
    if (__atomic_load_n(&mutex->owner_tid, __ATOMIC_RELAXED) == self_tid &&
        mutex->type == PTHREAD_MUTEX_RECURSIVE) {
        if (mutex->recursion == UINT32_MAX) {
            return EAGAIN;
        }
        mutex->recursion += 1U;
        return 0;
    }
    return EBUSY;
}

int pthread_mutex_unlock(pthread_mutex_t* mutex)
{
    if (!mutex) {
        return EINVAL;
    }
    uint64_t self_tid = current_thread_record()->tid;
    uint64_t owner = __atomic_load_n(&mutex->owner_tid, __ATOMIC_RELAXED);
    if (owner != 0ULL && owner != self_tid) {
        return EPERM;
    }
    if (mutex->type == PTHREAD_MUTEX_RECURSIVE && mutex->recursion > 1U) {
        mutex->recursion -= 1U;
        return 0;
    }
    __atomic_store_n(&mutex->owner_tid, 0ULL, __ATOMIC_RELAXED);
    mutex->recursion = 0U;
    uint32_t prev = __atomic_exchange_n(&mutex->state, 0U, __ATOMIC_RELEASE);
    if (prev == 2U) {
        wake_word_Wrapper(&mutex->state, 1U);
    }
    return 0;
}

int pthread_mutex_destroy(pthread_mutex_t* mutex)
{
    if (!mutex) {
        return EINVAL;
    }
    if (__atomic_load_n(&mutex->state, __ATOMIC_RELAXED) != 0U) {
        return EBUSY;
    }
    return 0;
}

/* Condition variables */
int pthread_condattr_init(pthread_condattr_t* attr)
{
    if (!attr) {
        return EINVAL;
    }
    attr->clock_id = CLOCK_MONOTONIC;
    return 0;
}

int pthread_condattr_destroy(pthread_condattr_t* attr)
{
    return attr ? 0 : EINVAL;
}

int pthread_condattr_setclock(pthread_condattr_t* attr, int clock_id)
{
    if (!attr || clock_id != CLOCK_MONOTONIC) {
        return EINVAL;
    }
    attr->clock_id = clock_id;
    return 0;
}

int pthread_condattr_getclock(const pthread_condattr_t* attr, int* clock_id)
{
    if (!attr || !clock_id) {
        return EINVAL;
    }
    *clock_id = attr->clock_id;
    return 0;
}

int pthread_cond_init(pthread_cond_t* cond, const pthread_condattr_t* attr)
{
    if (!cond) {
        return EINVAL;
    }
    cond->seq = 0U;
    cond->clock_id = attr ? attr->clock_id : CLOCK_MONOTONIC;
    return 0;
}

int pthread_cond_timedwait(pthread_cond_t* cond, pthread_mutex_t* mutex,
                           const struct timespec* abstime)
{
    if (!cond || !mutex) {
        return EINVAL;
    }
    uint64_t deadline_ns = 0ULL;
    if (abstime) {
        if (abstime->tv_nsec < 0L || abstime->tv_nsec >= 1000000000L) {
            return EINVAL;
        }
        if (abstime->tv_sec < 0) {
            return ETIMEDOUT;
        }
        deadline_ns = (uint64_t)abstime->tv_sec * 1000000000ULL + (uint64_t)abstime->tv_nsec;
        if (deadline_ns == 0ULL) {
            deadline_ns = 1ULL;
        }
    }

    uint32_t seq = __atomic_load_n(&cond->seq, __ATOMIC_ACQUIRE);
    uint32_t saved_recursion = mutex->recursion > 0U ? mutex->recursion : 1U;
    mutex->recursion = 1U;
    int unlock_rc = pthread_mutex_unlock(mutex);
    if (unlock_rc != 0) {
        return unlock_rc;
    }

    int timed_out = 0;
    while (__atomic_load_n(&cond->seq, __ATOMIC_ACQUIRE) == seq) {
        int64_t wait_rc = wait_word_Wrapper(&cond->seq, seq, deadline_ns);
        if (wait_rc == -110) {
            timed_out = 1;
            break;
        }
        if (deadline_ns != 0ULL) {
            struct timespec now_ts;
            if (clock_gettime(CLOCK_MONOTONIC, &now_ts) == 0) {
                uint64_t now_ns = (uint64_t)now_ts.tv_sec * 1000000000ULL + (uint64_t)now_ts.tv_nsec;
                if (now_ns >= deadline_ns && __atomic_load_n(&cond->seq, __ATOMIC_ACQUIRE) == seq) {
                    timed_out = 1;
                    break;
                }
            }
        }
    }

    (void)pthread_mutex_lock(mutex);
    mutex->recursion = saved_recursion;

    if (timed_out && __atomic_load_n(&cond->seq, __ATOMIC_ACQUIRE) == seq) {
        return ETIMEDOUT;
    }
    return 0;
}

int pthread_cond_wait(pthread_cond_t* cond, pthread_mutex_t* mutex)
{
    return pthread_cond_timedwait(cond, mutex, NULL);
}

int pthread_cond_signal(pthread_cond_t* cond)
{
    if (!cond) {
        return EINVAL;
    }
    __atomic_fetch_add(&cond->seq, 1U, __ATOMIC_RELEASE);
    wake_word_Wrapper(&cond->seq, 1U);
    return 0;
}

int pthread_cond_broadcast(pthread_cond_t* cond)
{
    if (!cond) {
        return EINVAL;
    }
    __atomic_fetch_add(&cond->seq, 1U, __ATOMIC_RELEASE);
    wake_word_Wrapper(&cond->seq, 0x7fffffffU);
    return 0;
}

int pthread_cond_destroy(pthread_cond_t* cond)
{
    return cond ? 0 : EINVAL;
}

/* pthread_once */
int pthread_once(pthread_once_t* once_control, void (*init_routine)(void))
{
    if (!once_control || !init_routine) {
        return EINVAL;
    }
    uint32_t state = __atomic_load_n(&once_control->state, __ATOMIC_ACQUIRE);
    if (state == 2U) {
        return 0;
    }
    uint32_t expected = 0U;
    if (__atomic_compare_exchange_n(&once_control->state, &expected, 1U,
                                    0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
        init_routine();
        __atomic_store_n(&once_control->state, 2U, __ATOMIC_RELEASE);
        wake_word_Wrapper(&once_control->state, 0x7fffffffU);
        return 0;
    }
    while (__atomic_load_n(&once_control->state, __ATOMIC_ACQUIRE) != 2U) {
        (void)wait_word_Wrapper(&once_control->state, 1U, 0ULL);
    }
    return 0;
}

/* Thread-specific keys */
int pthread_key_create(pthread_key_t* key, void (*destructor)(void*))
{
    if (!key) {
        return EINVAL;
    }
    spin_lock(&g_key_lock);
    for (uint32_t i = 0; i < PTHREAD_KEYS_MAX; ++i) {
        if (!g_keys[i].in_use) {
            g_keys[i].in_use = 1U;
            g_keys[i].generation += 1U;
            g_keys[i].destructor = destructor;
            *key = i;
            spin_unlock(&g_key_lock);
            return 0;
        }
    }
    spin_unlock(&g_key_lock);
    return EAGAIN;
}

int pthread_key_delete(pthread_key_t key)
{
    if (key >= PTHREAD_KEYS_MAX) {
        return EINVAL;
    }
    spin_lock(&g_key_lock);
    if (!g_keys[key].in_use) {
        spin_unlock(&g_key_lock);
        return EINVAL;
    }
    g_keys[key].in_use = 0U;
    g_keys[key].generation += 1U;
    g_keys[key].destructor = NULL;
    spin_unlock(&g_key_lock);
    g_tls_state.key_values[key] = NULL;
    return 0;
}

void* pthread_getspecific(pthread_key_t key)
{
    if (key >= PTHREAD_KEYS_MAX) {
        return NULL;
    }
    (void)current_thread_record();
    spin_lock(&g_key_lock);
    uint32_t in_use = g_keys[key].in_use;
    uint32_t gen = g_keys[key].generation;
    spin_unlock(&g_key_lock);
    if (!in_use || g_tls_state.key_generations[key] != gen) {
        return NULL;
    }
    return g_tls_state.key_values[key];
}

int pthread_setspecific(pthread_key_t key, const void* value)
{
    if (key >= PTHREAD_KEYS_MAX) {
        return EINVAL;
    }
    (void)current_thread_record();
    spin_lock(&g_key_lock);
    if (!g_keys[key].in_use) {
        spin_unlock(&g_key_lock);
        return EINVAL;
    }
    uint32_t gen = g_keys[key].generation;
    spin_unlock(&g_key_lock);
    g_tls_state.key_values[key] = (void*)value;
    g_tls_state.key_generations[key] = gen;
    return 0;
}
