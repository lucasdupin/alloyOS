#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <alloyos/syscall.h>

extern void __cxa_finalize(void* dso_handle) __attribute__((weak));
extern int __cxa_atexit(void (*destructor)(void*), void* arg, void* dso_handle) __attribute__((weak));

/* Anchor .tdata with 64-byte alignment so PT_TLS always has filesz > 0 and align >= 64 */
__attribute__((section(".tdata._alloyos_anchor"), aligned(64), used))
_Thread_local uint64_t __alloyos_tls_anchor = 0x414c4c4f594f5331ULL;

static _Thread_local int g_tls_errno = 0;

int* __errno(void)
{
    return &g_tls_errno;
}

int __alloyos_map_kernel_errno(int64_t negative_kernel_error)
{
    int code = (negative_kernel_error < 0) ? (int)(-negative_kernel_error) : (int)negative_kernel_error;
    switch (code) {
        case 1:   return EPERM;
        case 2:   return ENOENT;
        case 3:   return ESRCH;
        case 4:   return EINTR;
        case 5:   return EIO;
        case 9:   return EBADF;
        case 11:  return EAGAIN;
        case 12:  return ENOMEM;
        case 13:  return EACCES;
        case 14:  return EFAULT;
        case 16:  return EBUSY;
        case 17:  return EEXIST;
        case 22:  return EINVAL;
        case 28:  return ENOSPC;
        case 34:  return ERANGE;
        case 35:  return EDEADLK;
        case 38:  return ENOSYS;
        case 75:  return EOVERFLOW;
        case 90:
#ifdef EMSGSIZE
            return EMSGSIZE;
#else
            return EINVAL;
#endif
        case 95:
#ifdef ENOTSUP
            return ENOTSUP;
#else
            return EINVAL;
#endif
        case 110:
#ifdef ETIMEDOUT
            return ETIMEDOUT;
#else
            return EAGAIN;
#endif
        default:
            return code > 0 ? code : EINVAL;
    }
}

void __alloyos_panic_report(const char* message, size_t length)
{
    if (!message) {
        return;
    }
    if (length > 1024U) {
        length = 1024U;
    }
    (void)alloyos_syscall2(ALLOYOS_SYS_PANIC_REPORT, (uint64_t)(uintptr_t)message, (uint64_t)length);
}

__attribute__((noreturn)) void _exit(int status)
{
    (void)alloyos_syscall1(ALLOYOS_SYS_EXIT, (uint64_t)(uint32_t)status);
    for (;;) {
        __asm__ volatile("yield");
    }
}

__attribute__((noreturn)) void _Exit(int status)
{
    _exit(status);
}

__attribute__((noreturn)) void exit(int status)
{
    if (__cxa_finalize) {
        __cxa_finalize(NULL);
    }
    _exit(status);
}

__attribute__((noreturn)) void abort(void)
{
    static const char msg[] = "alloyOS userland abort()";
    __alloyos_panic_report(msg, sizeof(msg) - 1U);
    _exit(134);
}

static void atexit_thunk(void* fn_ptr)
{
    void (*fn)(void) = (void (*)(void))(uintptr_t)fn_ptr;
    if (fn) {
        fn();
    }
}

__attribute__((weak)) int atexit(void (*func)(void))
{
    if (!func || !__cxa_atexit) {
        return -1;
    }
    return __cxa_atexit(atexit_thunk, (void*)(uintptr_t)func, NULL);
}

/* Honest error-returning hooks for unavailable descriptor/signal services */
int _close(int fd) { (void)fd; errno = EBADF; return -1; }
int close(int fd) { return _close(fd); }

int _fstat(int fd, struct stat* st) { (void)fd; (void)st; errno = EBADF; return -1; }
int fstat(int fd, struct stat* st) { return _fstat(fd, st); }

off_t _lseek(int fd, off_t offset, int whence) { (void)fd; (void)offset; (void)whence; errno = EBADF; return (off_t)-1; }
off_t lseek(int fd, off_t offset, int whence) { return _lseek(fd, offset, whence); }

#ifndef _READ_WRITE_RETURN_TYPE
#define _READ_WRITE_RETURN_TYPE ssize_t
#endif

_READ_WRITE_RETURN_TYPE _read(int fd, void* buf, size_t count) { (void)fd; (void)buf; (void)count; errno = EBADF; return -1; }
_READ_WRITE_RETURN_TYPE read(int fd, void* buf, size_t count) { return _read(fd, buf, count); }

_READ_WRITE_RETURN_TYPE _write(int fd, const void* buf, size_t count) { (void)fd; (void)buf; (void)count; errno = EBADF; return -1; }
_READ_WRITE_RETURN_TYPE write(int fd, const void* buf, size_t count) { return _write(fd, buf, count); }

int _isatty(int fd) { (void)fd; errno = EBADF; return 0; }
int isatty(int fd) { return _isatty(fd); }

int _kill(int pid, int sig) { (void)pid; (void)sig; errno = ENOSYS; return -1; }
int kill(int pid, int sig) { return _kill(pid, sig); }

int _getpid(void)
{
    return (int)alloyos_syscall0(ALLOYOS_SYS_GETPID);
}

pid_t getpid(void)
{
    return (pid_t)_getpid();
}

long sysconf(int name)
{
    switch (name) {
        case _SC_PAGESIZE:
            return (long)ALLOYOS_PAGE_SIZE;
        case _SC_NPROCESSORS_CONF:
        case _SC_NPROCESSORS_ONLN:
            return 1L;
        default:
            errno = EINVAL;
            return -1L;
    }
}

/* Thread-safe bounded heap (_sbrk + mmap-backed allocator) */
static uint32_t g_heap_lock = 0;
static uintptr_t g_heap_begin = 0;
static uintptr_t g_heap_end = 0;
static uintptr_t g_heap_current = 0;

static void heap_lock_acquire(void)
{
    uint32_t expected = 0;
    while (!__atomic_compare_exchange_n(&g_heap_lock, &expected, 1U,
                                        1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        expected = 0;
        (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
    }
}

static void heap_lock_release(void)
{
    __atomic_store_n(&g_heap_lock, 0U, __ATOMIC_RELEASE);
}

void __alloyos_heap_init(uintptr_t heap_start, size_t heap_length)
{
    heap_lock_acquire();
    if (heap_start != 0 && heap_length != 0 && heap_length <= UINTPTR_MAX - heap_start) {
        uintptr_t aligned = (heap_start + 15U) & ~(uintptr_t)15U;
        if (aligned < heap_start + heap_length) {
            g_heap_begin = aligned;
            g_heap_current = aligned;
            g_heap_end = heap_start + heap_length;
        }
    }
    heap_lock_release();
}

static int ensure_fallback_heap_locked(size_t min_bytes)
{
    if (g_heap_end > g_heap_current && (g_heap_end - g_heap_current) >= min_bytes) {
        return 0;
    }
    if (g_heap_begin != 0) {
        return -1;
    }
    size_t reserve_len = 256U * ALLOYOS_PAGE_SIZE; /* 1 MiB fallback arena */
    if (min_bytes > reserve_len) {
        reserve_len = (min_bytes + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    }
    int64_t base = alloyos_syscall1(ALLOYOS_SYS_MEMORY_RESERVE, (uint64_t)reserve_len);
    if (base < 0) {
        return -1;
    }
    if (alloyos_syscall3(ALLOYOS_SYS_MEMORY_COMMIT, (uint64_t)base,
                         (uint64_t)reserve_len, ALLOYOS_MEMORY_READ_WRITE) < 0) {
        (void)alloyos_syscall2(ALLOYOS_SYS_MEMORY_RELEASE, (uint64_t)base, (uint64_t)reserve_len);
        return -1;
    }
    g_heap_begin = (uintptr_t)base;
    g_heap_current = (uintptr_t)base;
    g_heap_end = (uintptr_t)base + reserve_len;
    return 0;
}

void* _sbrk(ptrdiff_t increment)
{
    heap_lock_acquire();
    if (increment > 0 && ensure_fallback_heap_locked((size_t)increment) != 0) {
        heap_lock_release();
        errno = ENOMEM;
        return (void*)-1;
    }
    if ((increment >= 0 && (uintptr_t)increment > (g_heap_end - g_heap_current)) ||
        (increment < 0 && (uintptr_t)(-(increment + 1)) + 1U > (g_heap_current - g_heap_begin))) {
        heap_lock_release();
        errno = ENOMEM;
        return (void*)-1;
    }
    uintptr_t previous = g_heap_current;
    g_heap_current += (uintptr_t)increment;
    heap_lock_release();
    return (void*)previous;
}

void* sbrk(intptr_t increment)
{
    return _sbrk((ptrdiff_t)increment);
}

/* Allocator header supporting both _sbrk and mmap-backed allocations */
#define ALLOC_MAGIC_SBRK 0xA110C001UL
#define ALLOC_MAGIC_MMAP 0xA110C002UL

struct alloc_header {
    uintptr_t base_addr;
    size_t total_size;
    size_t user_size;
    uint64_t magic;
};

__attribute__((weak)) int posix_memalign(void** memptr, size_t alignment, size_t size)
{
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wpointer-bool-conversion"
    if (!memptr || alignment < sizeof(void*) || (alignment & (alignment - 1U)) != 0U) {
        return EINVAL;
    }
#pragma clang diagnostic pop
    if (size == 0U) {
        size = 1U;
    }
    size_t overhead = sizeof(struct alloc_header) + alignment;
    if (size > SIZE_MAX - overhead) {
        return ENOMEM;
    }
    size_t needed = (size + overhead + 15U) & ~(size_t)15U;

    /* Try _sbrk first for small/medium allocations if static heap has space */
    if (needed <= 65536U) {
        void* raw = _sbrk((ptrdiff_t)needed);
        if (raw != (void*)-1) {
            uintptr_t raw_addr = (uintptr_t)raw;
            uintptr_t user_addr = (raw_addr + sizeof(struct alloc_header) + alignment - 1U) & ~(uintptr_t)(alignment - 1U);
            struct alloc_header* hdr = ((struct alloc_header*)user_addr) - 1;
            hdr->base_addr = raw_addr;
            hdr->total_size = needed;
            hdr->user_size = size;
            hdr->magic = ALLOC_MAGIC_SBRK;
            *memptr = (void*)user_addr;
            return 0;
        }
    }

    /* Fall back to page-grained private anonymous mmap reservation */
    size_t page_len = (needed + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    void* mapped = mmap(NULL, page_len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mapped == MAP_FAILED) {
        return ENOMEM;
    }
    uintptr_t raw_addr = (uintptr_t)mapped;
    uintptr_t user_addr = (raw_addr + sizeof(struct alloc_header) + alignment - 1U) & ~(uintptr_t)(alignment - 1U);
    struct alloc_header* hdr = ((struct alloc_header*)user_addr) - 1;
    hdr->base_addr = raw_addr;
    hdr->total_size = page_len;
    hdr->user_size = size;
    hdr->magic = ALLOC_MAGIC_MMAP;
    *memptr = (void*)user_addr;
    return 0;
}

__attribute__((weak)) void* malloc(size_t size)
{
    void* ptr = NULL;
    int rc = posix_memalign(&ptr, 16U, size);
    if (rc != 0) {
        errno = rc;
        return NULL;
    }
    return ptr;
}

__attribute__((weak)) void* aligned_alloc(size_t alignment, size_t size)
{
    if (alignment < sizeof(void*)) {
        alignment = sizeof(void*);
    }
    void* ptr = NULL;
    int rc = posix_memalign(&ptr, alignment, size);
    if (rc != 0) {
        errno = rc;
        return NULL;
    }
    return ptr;
}

__attribute__((weak)) void free(void* ptr)
{
    if (!ptr) {
        return;
    }
    struct alloc_header* hdr = ((struct alloc_header*)ptr) - 1;
    if (hdr->magic == ALLOC_MAGIC_MMAP) {
        uintptr_t base = hdr->base_addr;
        size_t total = hdr->total_size;
        hdr->magic = 0;
        (void)munmap((void*)base, total);
    } else if (hdr->magic == ALLOC_MAGIC_SBRK) {
        hdr->magic = 0;
    }
}

__attribute__((weak)) void* calloc(size_t nmemb, size_t size)
{
    if (nmemb != 0U && size > SIZE_MAX / nmemb) {
        errno = ENOMEM;
        return NULL;
    }
    size_t total = nmemb * size;
    void* ptr = malloc(total);
    if (ptr) {
        memset(ptr, 0, total);
    }
    return ptr;
}

__attribute__((weak)) void* realloc(void* ptr, size_t size)
{
    if (!ptr) {
        return malloc(size);
    }
    if (size == 0U) {
        free(ptr);
        return NULL;
    }
    struct alloc_header* hdr = ((struct alloc_header*)ptr) - 1;
    if ((hdr->magic == ALLOC_MAGIC_SBRK || hdr->magic == ALLOC_MAGIC_MMAP) && size <= hdr->user_size) {
        hdr->user_size = size;
        return ptr;
    }
    void* next = malloc(size);
    if (!next) {
        return NULL;
    }
    size_t copy_bytes = hdr->user_size < size ? hdr->user_size : size;
    memcpy(next, ptr, copy_bytes);
    free(ptr);
    return next;
}

/* Weak freestanding memory and string primitives (yield to newlib libc.a when present) */
__attribute__((weak)) void* memcpy(void* dst, const void* src, size_t n)
{
    unsigned char* d = (unsigned char*)dst;
    const unsigned char* s = (const unsigned char*)src;
    for (size_t i = 0; i < n; ++i) {
        d[i] = s[i];
    }
    return dst;
}

__attribute__((weak)) void* memmove(void* dst, const void* src, size_t n)
{
    unsigned char* d = (unsigned char*)dst;
    const unsigned char* s = (const unsigned char*)src;
    if (d == s || n == 0U) {
        return dst;
    }
    if (d < s) {
        for (size_t i = 0; i < n; ++i) {
            d[i] = s[i];
        }
    } else {
        for (size_t i = n; i > 0U; --i) {
            d[i - 1U] = s[i - 1U];
        }
    }
    return dst;
}

__attribute__((weak)) void* memset(void* s, int c, size_t n)
{
    unsigned char* p = (unsigned char*)s;
    unsigned char v = (unsigned char)c;
    for (size_t i = 0; i < n; ++i) {
        p[i] = v;
    }
    return s;
}

__attribute__((weak)) int memcmp(const void* s1, const void* s2, size_t n)
{
    const unsigned char* a = (const unsigned char*)s1;
    const unsigned char* b = (const unsigned char*)s2;
    for (size_t i = 0; i < n; ++i) {
        if (a[i] != b[i]) {
            return (int)a[i] - (int)b[i];
        }
    }
    return 0;
}

__attribute__((weak)) void* memchr(const void* s, int c, size_t n)
{
    const unsigned char* p = (const unsigned char*)s;
    unsigned char v = (unsigned char)c;
    for (size_t i = 0; i < n; ++i) {
        if (p[i] == v) {
            return (void*)(p + i);
        }
    }
    return NULL;
}

__attribute__((weak)) size_t strlen(const char* s)
{
    size_t len = 0;
    while (s[len] != '\0') {
        ++len;
    }
    return len;
}

__attribute__((weak)) size_t strnlen(const char* s, size_t maxlen)
{
    size_t len = 0;
    while (len < maxlen && s[len] != '\0') {
        ++len;
    }
    return len;
}

__attribute__((weak)) int strcmp(const char* s1, const char* s2)
{
    while (*s1 != '\0' && *s1 == *s2) {
        ++s1;
        ++s2;
    }
    return (int)(unsigned char)*s1 - (int)(unsigned char)*s2;
}

__attribute__((weak)) int strncmp(const char* s1, const char* s2, size_t n)
{
    for (size_t i = 0; i < n; ++i) {
        unsigned char c1 = (unsigned char)s1[i];
        unsigned char c2 = (unsigned char)s2[i];
        if (c1 != c2 || c1 == '\0') {
            return (int)c1 - (int)c2;
        }
    }
    return 0;
}

__attribute__((weak)) char* strcpy(char* dst, const char* src)
{
    char* out = dst;
    while ((*dst++ = *src++) != '\0') {
    }
    return out;
}

__attribute__((weak)) char* strncpy(char* dst, const char* src, size_t n)
{
    size_t i = 0;
    for (; i < n && src[i] != '\0'; ++i) {
        dst[i] = src[i];
    }
    for (; i < n; ++i) {
        dst[i] = '\0';
    }
    return dst;
}
