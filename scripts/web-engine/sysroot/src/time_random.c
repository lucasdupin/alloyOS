#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/random.h>
#include <sys/time.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>
#include <alloyos/syscall.h>

struct alloyos_kernel_timespec {
    int64_t tv_sec;
    int64_t tv_nsec;
};

int clock_gettime(clockid_t clock_id, struct timespec* tp)
{
    if (!tp) {
        errno = EFAULT;
        return -1;
    }
    struct alloyos_kernel_timespec kts = { 0, 0 };
    int64_t rc = alloyos_syscall2(ALLOYOS_SYS_CLOCK_GETTIME,
                                  (uint64_t)(uint32_t)clock_id,
                                  (uint64_t)(uintptr_t)&kts);
    if (rc < 0) {
        errno = __alloyos_map_kernel_errno(rc);
        return -1;
    }
    if (kts.tv_sec < 0 || kts.tv_nsec < 0 || kts.tv_nsec >= 1000000000LL) {
        errno = EINVAL;
        return -1;
    }
    tp->tv_sec = (time_t)kts.tv_sec;
    tp->tv_nsec = (long)kts.tv_nsec;
    return 0;
}

int clock_getres(clockid_t clock_id, struct timespec* res)
{
    if (clock_id != CLOCK_MONOTONIC) {
        errno = EINVAL;
        return -1;
    }
    if (res) {
        res->tv_sec = 0;
        res->tv_nsec = 1;
    }
    return 0;
}

__attribute__((weak)) int gettimeofday(struct timeval* tv, void* tz)
{
    (void)tv;
    (void)tz;
    /* Protected ABI deliberately exposes only CLOCK_MONOTONIC, not wall time */
    errno = ENOSYS;
    return -1;
}

__attribute__((weak)) int _gettimeofday(struct timeval* tv, void* tz)
{
    return gettimeofday(tv, tz);
}

int nanosleep(const struct timespec* req, struct timespec* rem)
{
    if (!req || req->tv_sec < 0 || req->tv_nsec < 0 || req->tv_nsec >= 1000000000L) {
        errno = EINVAL;
        return -1;
    }
    if (rem) {
        rem->tv_sec = 0;
        rem->tv_nsec = 0;
    }
    uint64_t delta_ns = (uint64_t)req->tv_sec * 1000000000ULL + (uint64_t)req->tv_nsec;
    if (delta_ns == 0ULL) {
        return 0;
    }
    struct timespec now_ts;
    if (clock_gettime(CLOCK_MONOTONIC, &now_ts) != 0) {
        return -1;
    }
    uint64_t start_ns = (uint64_t)now_ts.tv_sec * 1000000000ULL + (uint64_t)now_ts.tv_nsec;
    uint64_t deadline_ns = start_ns + delta_ns;

    /* Sleep via WORD_WAIT when native threads are enabled; otherwise poll monotonic clock */
    uint32_t wait_word __attribute__((aligned(4))) = 0U;
    int64_t wait_rc = alloyos_syscall3(ALLOYOS_SYS_WORD_WAIT,
                                       (uint64_t)(uintptr_t)&wait_word,
                                       0ULL,
                                       deadline_ns);
    if (wait_rc == -110 || wait_rc == 0) {
        return 0;
    }
    while (clock_gettime(CLOCK_MONOTONIC, &now_ts) == 0) {
        uint64_t cur_ns = (uint64_t)now_ts.tv_sec * 1000000000ULL + (uint64_t)now_ts.tv_nsec;
        if (cur_ns >= deadline_ns) {
            break;
        }
        (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
    }
    return 0;
}

int usleep(useconds_t usec)
{
    struct timespec req;
    req.tv_sec = (time_t)(usec / 1000000U);
    req.tv_nsec = (long)((usec % 1000000U) * 1000U);
    return nanosleep(&req, NULL);
}

unsigned int sleep(unsigned int seconds)
{
    struct timespec req;
    req.tv_sec = (time_t)seconds;
    req.tv_nsec = 0L;
    (void)nanosleep(&req, NULL);
    return 0U;
}

static int64_t raw_getrandom_chunk(void* dst, size_t chunk_len, unsigned int flags)
{
    int64_t rc = alloyos_syscall3(ALLOYOS_SYS_GETRANDOM,
                                  (uint64_t)(uintptr_t)dst,
                                  (uint64_t)chunk_len,
                                  (uint64_t)flags);
    if (rc == -38) {
        rc = alloyos_syscall3(ALLOYOS_SYS_GETRANDOM_ALT,
                              (uint64_t)(uintptr_t)dst,
                              (uint64_t)chunk_len,
                              (uint64_t)flags);
    }
    return rc;
}

ssize_t getrandom(void* buf, size_t buflen, unsigned int flags)
{
    if (!buf && buflen > 0U) {
        errno = EFAULT;
        return -1;
    }
    if (flags != 0U) {
        errno = EINVAL;
        return -1;
    }
    if (buflen == 0U) {
        int64_t rc = raw_getrandom_chunk(buf, 0U, 0U);
        if (rc < 0) {
            errno = __alloyos_map_kernel_errno(rc);
            return -1;
        }
        return 0;
    }

    unsigned char* out = (unsigned char*)buf;
    size_t offset = 0U;
    while (offset < buflen) {
        size_t remaining = buflen - offset;
        size_t chunk = remaining > 256U ? 256U : remaining;
        int64_t rc = raw_getrandom_chunk(out + offset, chunk, 0U);
        if (rc < 0 || (size_t)rc != chunk) {
            /* Fail closed: wipe any partial output and propagate error (e.g. EAGAIN) */
            if (offset > 0U) {
                memset(out, 0, offset);
            }
            errno = (rc < 0) ? __alloyos_map_kernel_errno(rc) : EIO;
            return -1;
        }
        offset += chunk;
    }
    return (ssize_t)buflen;
}

int getentropy(void* buf, size_t buflen)
{
    if (!buf && buflen > 0U) {
        errno = EFAULT;
        return -1;
    }
    if (buflen > 256U) {
        errno = EIO;
        return -1;
    }
    ssize_t got = getrandom(buf, buflen, 0U);
    if (got < 0 || (size_t)got != buflen) {
        return -1;
    }
    return 0;
}
