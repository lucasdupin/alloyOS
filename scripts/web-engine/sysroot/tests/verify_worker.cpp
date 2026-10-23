#include "verify.h"
#include <errno.h>
#include <new>
#include <pthread.h>
#include <sched.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <time.h>

namespace {

alignas(64) thread_local int g_worker_tls_init = 19;
thread_local int g_worker_tls_zero = 0;

class MultiplyEvaluator final : public SysrootEvaluator {
    int factor_;
public:
    explicit MultiplyEvaluator(int factor) : factor_(factor) {}
    int compute(int a, int b) const override {
        return (a * b) * factor_;
    }
};

struct StaticGuardFixture {
    int value;
    StaticGuardFixture() : value(42) {}
};

int query_guarded_singleton() {
    static StaticGuardFixture instance;
    return instance.value;
}

pthread_once_t g_once_control = PTHREAD_ONCE_INIT;
int g_once_counter = 0;
pthread_key_t g_tls_key = 0;
int g_key_dtor_count = 0;

void once_init_fn(void) {
    g_once_counter += 1;
}

void key_dtor_fn(void* ptr) {
    if (ptr) {
        g_key_dtor_count += 1;
    }
}

struct WorkerContext {
    pthread_mutex_t mutex;
    pthread_cond_t cond;
    int ready;
    int tls_ok;
    int computed;
};

void* worker_entry(void* raw) {
    WorkerContext* ctx = static_cast<WorkerContext*>(raw);
    (void)pthread_once(&g_once_control, once_init_fn);

    bool initial_tls_valid = (g_worker_tls_init == 19 && g_worker_tls_zero == 0);
    g_worker_tls_init = 99;
    g_worker_tls_zero = 77;
    (void)pthread_setspecific(g_tls_key, reinterpret_cast<void*>(0x1234UL));
    (void)sched_yield();

    pthread_mutex_lock(&ctx->mutex);
    ctx->tls_ok = (initial_tls_valid &&
                   g_worker_tls_init == 99 &&
                   g_worker_tls_zero == 77 &&
                   pthread_getspecific(g_tls_key) == reinterpret_cast<void*>(0x1234UL)) ? 1 : 0;
    ctx->computed = query_guarded_singleton();
    ctx->ready = 1;
    pthread_cond_signal(&ctx->cond);
    pthread_mutex_unlock(&ctx->mutex);
    return reinterpret_cast<void*>(42UL);
}

} // namespace

SysrootEvaluator* create_sysroot_evaluator(int multiplier) {
    return new MultiplyEvaluator(multiplier);
}

int run_sysroot_worker_checks(void) {
    if (query_guarded_singleton() != 42) {
        return 10;
    }
    AlignedPayload* aligned = new AlignedPayload();
    if ((reinterpret_cast<uintptr_t>(aligned) & 63U) != 0U) {
        delete aligned;
        return 11;
    }
    aligned->values[0] = 0xCAFEBABEULL;
    delete aligned;

    int* arr = new int[8];
    for (int i = 0; i < 8; ++i) {
        arr[i] = i + 1;
    }
    delete[] arr;

    /* Verify mmap / mprotect / munmap and PROT_EXEC rejection */
    void* exec_attempt = mmap(nullptr, 4096, PROT_READ | PROT_EXEC,
                              MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (exec_attempt != MAP_FAILED || errno != ENOTSUP) {
        return 12;
    }
    void* file_attempt = mmap(nullptr, 4096, PROT_READ | PROT_WRITE,
                              MAP_PRIVATE, 3, 0);
    if (file_attempt != MAP_FAILED || errno != ENOTSUP) {
        return 13;
    }

    void* reserved = mmap(nullptr, 8192, PROT_NONE,
                          MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (reserved == MAP_FAILED) {
        return 14;
    }
    if (mprotect(reserved, 4096, PROT_READ | PROT_WRITE) != 0) {
        (void)munmap(reserved, 8192);
        return 15;
    }
    static_cast<uint32_t*>(reserved)[0] = 0x12345678U;
    if (mprotect(reserved, 4096, PROT_READ) != 0) {
        (void)munmap(reserved, 8192);
        return 16;
    }
    if (static_cast<const volatile uint32_t*>(reserved)[0] != 0x12345678U) {
        (void)munmap(reserved, 8192);
        return 17;
    }
    if (mprotect(reserved, 4096, PROT_NONE) != 0) {
        (void)munmap(reserved, 8192);
        return 18;
    }
    if (munmap(reserved, 8192) != 0) {
        return 19;
    }

    /* Verify clock_gettime(CLOCK_MONOTONIC) */
    struct timespec ts {};
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0 || ts.tv_sec < 0) {
        return 20;
    }

    /* Verify getrandom >256 chunking and getentropy */
    unsigned char entropy_buf[300];
    memset(entropy_buf, 0xA5, sizeof(entropy_buf));
    ssize_t gr = getrandom(entropy_buf, sizeof(entropy_buf), 0);
    if (gr < 0 && errno != EAGAIN) {
        return 21;
    }
    if (getentropy(entropy_buf, 300) != -1 || errno != EIO) {
        return 22;
    }

    /* Verify pthread_once, pthread_key, mutex/cond, and thread creation/join */
    if (pthread_key_create(&g_tls_key, key_dtor_fn) != 0) {
        return 23;
    }
    (void)pthread_once(&g_once_control, once_init_fn);
    if (g_once_counter != 1) {
        return 24;
    }

    WorkerContext ctx {};
    pthread_mutex_init(&ctx.mutex, nullptr);
    pthread_cond_init(&ctx.cond, nullptr);

    pthread_t worker {};
    int create_rc = pthread_create(&worker, nullptr, worker_entry, &ctx);
    if (create_rc == 0) {
        if (pthread_equal(worker, pthread_self())) {
            return 25;
        }
        pthread_mutex_lock(&ctx.mutex);
        while (!ctx.ready) {
            pthread_cond_wait(&ctx.cond, &ctx.mutex);
        }
        pthread_mutex_unlock(&ctx.mutex);

        void* retval = nullptr;
        if (pthread_join(worker, &retval) != 0 || retval != reinterpret_cast<void*>(42UL)) {
            return 26;
        }
        if (!ctx.tls_ok || ctx.computed != 42 || g_worker_tls_init != 19 ||
            g_worker_tls_zero != 0 || g_once_counter != 1 || g_key_dtor_count != 1) {
            return 27;
        }
    } else if (create_rc != EPERM) {
        return 28;
    }

    pthread_cond_destroy(&ctx.cond);
    pthread_mutex_destroy(&ctx.mutex);
    pthread_key_delete(g_tls_key);
    return 0;
}
