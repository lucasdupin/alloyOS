#include <errno.h>
#ifndef EAFNOSUPPORT
#define EAFNOSUPPORT 106
#endif
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <netinet/in.h>
#include <netdb.h>
#include <poll.h>
#include <sys/select.h>
#include <sys/socket.h>
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

__attribute__((weak)) int* __errno(void)
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

__attribute__((weak, noreturn)) void _exit(int status)
{
    (void)alloyos_syscall1(ALLOYOS_SYS_EXIT, (uint64_t)(uint32_t)status);
    for (;;) {
        __asm__ volatile("yield");
    }
}

__attribute__((weak, noreturn)) void _Exit(int status)
{
    _exit(status);
}

__attribute__((weak, noreturn)) void exit(int status)
{
    if (__cxa_finalize) {
        __cxa_finalize(NULL);
    }
    _exit(status);
}

__attribute__((weak, noreturn)) void abort(void)
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
__attribute__((weak)) int _close(int fd) { (void)fd; errno = EBADF; return -1; }
__attribute__((weak)) int close(int fd) { return _close(fd); }

__attribute__((weak)) int _fstat(int fd, struct stat* st) { (void)fd; (void)st; errno = EBADF; return -1; }
__attribute__((weak)) int fstat(int fd, struct stat* st) { return _fstat(fd, st); }

__attribute__((weak)) int _stat(const char* file, struct stat* st) { (void)file; (void)st; errno = ENOENT; return -1; }
__attribute__((weak)) int stat(const char* file, struct stat* st) { return _stat(file, st); }

__attribute__((weak)) int _lstat(const char* file, struct stat* st) { return _stat(file, st); }
__attribute__((weak)) int lstat(const char* file, struct stat* st) { return _stat(file, st); }

__attribute__((weak)) int _open(const char* file, int flags, ...) { (void)file; (void)flags; errno = ENOENT; return -1; }
__attribute__((weak)) int open(const char* file, int flags, ...) { return _open(file, flags); }

__attribute__((weak)) int _unlink(const char* pathname) { (void)pathname; errno = ENOENT; return -1; }
__attribute__((weak)) int unlink(const char* pathname) { return _unlink(pathname); }

__attribute__((weak)) int ioctl(int fd, unsigned long request, ...) { (void)fd; (void)request; errno = ENOTTY; return -1; }

__attribute__((weak)) int ftruncate(int fd, off_t length) { (void)fd; (void)length; errno = EBADF; return -1; }
__attribute__((weak)) int fsync(int fd) { (void)fd; return 0; }

char* getcwd(char* buf, size_t size)
{
    if (!buf) {
        buf = (char*)malloc(size ? size : 2);
    }
    if (size < 2 && size != 0) {
        errno = ERANGE;
        return NULL;
    }
    if (buf) {
        buf[0] = '/';
        buf[1] = '\0';
    }
    return buf;
}

int sigaction(int signum, const struct sigaction* act, struct sigaction* oldact)
{
    (void)signum; (void)act; (void)oldact;
    return 0;
}

int sigsuspend(const sigset_t* mask)
{
    (void)mask;
    errno = EINTR;
    return -1;
}

int pthread_sigmask(int how, const sigset_t* set, sigset_t* oldset)
{
    (void)how; (void)set; (void)oldset;
    return 0;
}

int pthread_kill(pthread_t thread, int sig)
{
    (void)thread; (void)sig;
    return 0;
}

off_t _lseek(int fd, off_t offset, int whence) { (void)fd; (void)offset; (void)whence; errno = EBADF; return (off_t)-1; }
off_t lseek(int fd, off_t offset, int whence) { return _lseek(fd, offset, whence); }

#ifndef _READ_WRITE_RETURN_TYPE
#define _READ_WRITE_RETURN_TYPE ssize_t
#endif

__attribute__((weak)) _READ_WRITE_RETURN_TYPE _read(int fd, void* buf, size_t count) { (void)fd; (void)buf; (void)count; errno = EBADF; return -1; }
__attribute__((weak)) _READ_WRITE_RETURN_TYPE read(int fd, void* buf, size_t count) { return _read(fd, buf, count); }

__attribute__((weak)) _READ_WRITE_RETURN_TYPE _write(int fd, const void* buf, size_t count) { (void)fd; (void)buf; (void)count; errno = EBADF; return -1; }
__attribute__((weak)) _READ_WRITE_RETURN_TYPE write(int fd, const void* buf, size_t count) { return _write(fd, buf, count); }

__attribute__((weak)) int _isatty(int fd) { (void)fd; errno = EBADF; return 0; }
__attribute__((weak)) int isatty(int fd) { return _isatty(fd); }

__attribute__((weak)) int _kill(int pid, int sig) { (void)pid; (void)sig; errno = ENOSYS; return -1; }
__attribute__((weak)) int kill(int pid, int sig) { return _kill(pid, sig); }

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

__attribute__((weak)) int poll(struct pollfd* fds, nfds_t nfds, int timeout)
{
    (void)timeout;
    if (!fds || nfds == 0) return 0;
    int ready = 0;
    for (nfds_t i = 0; i < nfds; ++i) {
        fds[i].revents = fds[i].events & (POLLIN | POLLOUT | POLLPRI);
        if (fds[i].revents) ready++;
    }
    return ready;
}

__attribute__((weak)) int res_query(const char* dname, int c, int t, unsigned char* ans, int anslen)
{
    (void)dname; (void)c; (void)t; (void)ans; (void)anslen;
    return -1;
}

__attribute__((weak)) int res_search(const char* dname, int c, int t, unsigned char* ans, int anslen)
{
    (void)dname; (void)c; (void)t; (void)ans; (void)anslen;
    return -1;
}

__attribute__((weak)) int res_init(void)
{
    return 0;
}

__attribute__((weak)) int dn_comp(const char* src, unsigned char* dst, int dstsiz, unsigned char** dnptrs, unsigned char** lastdnptr)
{
    (void)src; (void)dst; (void)dstsiz; (void)dnptrs; (void)lastdnptr;
    return -1;
}

__attribute__((weak)) int dn_expand(const unsigned char* msg, const unsigned char* eomorig, const unsigned char* comp_dn, char* exp_dn, int length)
{
    (void)msg; (void)eomorig; (void)comp_dn; (void)exp_dn; (void)length;
    return -1;
}

struct alloyos_sockaddr_v4 {
    uint8_t ip[4];
    uint16_t port;
};

__attribute__((weak)) int socket(int domain, int type, int protocol)
{
    int64_t ret = alloyos_syscall3(ALLOYOS_SYS_SOCKET, (uint64_t)domain, (uint64_t)type, (uint64_t)protocol);
    if (ret < 0) {
        errno = __alloyos_map_kernel_errno(ret);
        return -1;
    }
    return (int)ret;
}

__attribute__((weak)) int bind(int sockfd, const struct sockaddr* addr, socklen_t addrlen)
{
    (void)sockfd; (void)addr; (void)addrlen;
    return -1;
}

__attribute__((weak)) int listen(int sockfd, int backlog)
{
    (void)sockfd; (void)backlog;
    return -1;
}

__attribute__((weak)) int accept(int sockfd, struct sockaddr* addr, socklen_t* addrlen)
{
    (void)sockfd; (void)addr; (void)addrlen;
    return -1;
}

__attribute__((weak)) int connect(int sockfd, const struct sockaddr* addr, socklen_t addrlen)
{
    if (!addr || addrlen < (socklen_t)sizeof(struct sockaddr_in)) {
        errno = EINVAL;
        return -1;
    }
    const struct sockaddr_in* in = (const struct sockaddr_in*)addr;
    if (in->sin_family != AF_INET) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    struct alloyos_sockaddr_v4 v4;
    memcpy(v4.ip, &in->sin_addr.s_addr, 4);
    v4.port = ntohs(in->sin_port);

    int64_t ret = alloyos_syscall3(ALLOYOS_SYS_CONNECT, (uint64_t)sockfd, (uint64_t)&v4, (uint64_t)sizeof(v4));
    if (ret < 0) {
        errno = __alloyos_map_kernel_errno(ret);
        return -1;
    }
    return 0;
}

__attribute__((weak)) ssize_t send(int sockfd, const void* buf, size_t len, int flags)
{
    (void)flags;
    int64_t ret = alloyos_syscall3(ALLOYOS_SYS_SEND, (uint64_t)sockfd, (uint64_t)buf, (uint64_t)len);
    if (ret < 0) {
        errno = __alloyos_map_kernel_errno(ret);
        return -1;
    }
    return (ssize_t)ret;
}

__attribute__((weak)) ssize_t recv(int sockfd, void* buf, size_t len, int flags)
{
    (void)flags;
    int64_t ret = alloyos_syscall3(ALLOYOS_SYS_RECV, (uint64_t)sockfd, (uint64_t)buf, (uint64_t)len);
    if (ret < 0) {
        errno = __alloyos_map_kernel_errno(ret);
        return -1;
    }
    return (ssize_t)ret;
}

__attribute__((weak)) ssize_t sendto(int sockfd, const void* buf, size_t len, int flags, const struct sockaddr* dest_addr, socklen_t addrlen)
{
    (void)dest_addr; (void)addrlen;
    return send(sockfd, buf, len, flags);
}

__attribute__((weak)) ssize_t recvfrom(int sockfd, void* buf, size_t len, int flags, struct sockaddr* src_addr, socklen_t* addrlen)
{
    (void)src_addr; (void)addrlen;
    return recv(sockfd, buf, len, flags);
}

__attribute__((weak)) int shutdown(int sockfd, int how)
{
    (void)sockfd; (void)how;
    return 0;
}

__attribute__((weak)) int getsockopt(int sockfd, int level, int optname, void* optval, socklen_t* optlen)
{
    (void)sockfd;
    if (level == SOL_SOCKET && optname == SO_ERROR) {
        if (optval && optlen && *optlen >= (socklen_t)sizeof(int)) {
            *(int*)optval = 0;
            *optlen = (socklen_t)sizeof(int);
            return 0;
        }
    }
    return 0;
}

__attribute__((weak)) int setsockopt(int sockfd, int level, int optname, const void* optval, socklen_t optlen)
{
    (void)sockfd; (void)level; (void)optname; (void)optval; (void)optlen;
    return 0;
}

__attribute__((weak)) int getsockname(int sockfd, struct sockaddr* addr, socklen_t* addrlen)
{
    (void)sockfd; (void)addr; (void)addrlen;
    return -1;
}

__attribute__((weak)) int getpeername(int sockfd, struct sockaddr* addr, socklen_t* addrlen)
{
    (void)sockfd; (void)addr; (void)addrlen;
    return -1;
}

__attribute__((weak)) int socketpair(int domain, int type, int protocol, int sv[2])
{
    (void)domain; (void)type; (void)protocol; (void)sv;
    return -1;
}

#include <utime.h>

__attribute__((weak)) int utime(const char* path, const struct utimbuf* times)
{
    (void)path; (void)times;
    return 0;
}

__attribute__((weak)) int getrlimit(int resource, void* rlp)
{
    (void)resource; (void)rlp;
    return 0;
}

__attribute__((weak)) int setrlimit(int resource, const void* rlp)
{
    (void)resource; (void)rlp;
    return 0;
}

#include <iconv.h>

__attribute__((weak)) iconv_t iconv_open(const char* tocode, const char* fromcode)
{
    (void)tocode;
    (void)fromcode;
    return (iconv_t)(uintptr_t)1;
}

__attribute__((weak)) size_t iconv(iconv_t cd, char** inbuf, size_t* inbytesleft, char** outbuf, size_t* outbytesleft)
{
    (void)cd;
    if (!inbuf || !*inbuf) return 0;
    size_t n = (*inbytesleft < *outbytesleft) ? *inbytesleft : *outbytesleft;
    memcpy(*outbuf, *inbuf, n);
    *inbuf += n;
    *outbuf += n;
    *inbytesleft -= n;
    *outbytesleft -= n;
    return 0;
}

__attribute__((weak)) int iconv_close(iconv_t cd)
{
    (void)cd;
    return 0;
}

struct termios;
__attribute__((weak)) int tcgetattr(int fd, struct termios* termios_p) { (void)fd; (void)termios_p; return 0; }
__attribute__((weak)) int tcsetattr(int fd, int optional_actions, const struct termios* termios_p) { (void)fd; (void)optional_actions; (void)termios_p; return 0; }
__attribute__((weak)) unsigned int cfgetispeed(const struct termios* termios_p) { (void)termios_p; return 9600; }
__attribute__((weak)) unsigned int cfgetospeed(const struct termios* termios_p) { (void)termios_p; return 9600; }
__attribute__((weak)) int cfsetispeed(struct termios* termios_p, unsigned int speed) { (void)termios_p; (void)speed; return 0; }
__attribute__((weak)) int cfsetospeed(struct termios* termios_p, unsigned int speed) { (void)termios_p; (void)speed; return 0; }

__attribute__((weak)) int uname(void* name)
{
    if (!name) return -1;
    char* p = (char*)name;
    for (int i = 0; i < 65 * 5; i++) p[i] = 0;
    const char* sys = "alloyOS";
    for (int i = 0; sys[i]; i++) p[i] = sys[i];
    const char* m = "aarch64";
    for (int i = 0; m[i]; i++) p[65 * 4 + i] = m[i];
    return 0;
}

__attribute__((weak)) int pthread_rwlock_init(pthread_rwlock_t* rwlock, const pthread_rwlockattr_t* attr)
{
    (void)rwlock; (void)attr;
    return 0;
}

__attribute__((weak)) int pthread_rwlock_destroy(pthread_rwlock_t* rwlock)
{
    (void)rwlock;
    return 0;
}

__attribute__((weak)) int pthread_rwlock_rdlock(pthread_rwlock_t* rwlock)
{
    (void)rwlock;
    return 0;
}

__attribute__((weak)) int pthread_rwlock_tryrdlock(pthread_rwlock_t* rwlock)
{
    (void)rwlock;
    return 0;
}

__attribute__((weak)) int pthread_rwlock_wrlock(pthread_rwlock_t* rwlock)
{
    (void)rwlock;
    return 0;
}

__attribute__((weak)) int pthread_rwlock_trywrlock(pthread_rwlock_t* rwlock)
{
    (void)rwlock;
    return 0;
}

__attribute__((weak)) int pthread_rwlock_unlock(pthread_rwlock_t* rwlock)
{
    (void)rwlock;
    return 0;
}

const struct in6_addr in6addr_any = { { 0 } };
const struct in6_addr in6addr_loopback = { { 0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1 } };

__attribute__((weak)) int inet_aton(const char* cp, void* inp)
{
    if (!cp || !inp) return 0;
    unsigned char bytes[4];
    int parts = 0;
    while (*cp) {
        if (parts >= 4) return 0;
        if (*cp < '0' || *cp > '9') return 0;
        unsigned int byte = 0;
        while (*cp >= '0' && *cp <= '9') {
            byte = byte * 10 + (*cp - '0');
            if (byte > 255) return 0;
            cp++;
        }
        bytes[parts++] = (unsigned char)byte;
        if (*cp == '.') {
            cp++;
            if (*cp == '\0') return 0;
        } else if (*cp != '\0') {
            return 0;
        }
    }
    if (parts != 4) return 0;
    memcpy(inp, bytes, 4);
    return 1;
}

__attribute__((weak)) int inet_pton(int af, const char* src, void* dst)
{
    if (!src || !dst) {
        errno = EAFNOSUPPORT;
        return -1;
    }
    if (af == AF_INET) {
        return inet_aton(src, dst);
    }
    if (af == AF_INET6) {
        unsigned char* out = (unsigned char*)dst;
        memset(out, 0, 16);
        uint16_t words[8] = { 0 };
        int word_count = 0;
        int double_colon_pos = -1;
        const char* p = src;

        if (*p == ':' && *(p + 1) == ':') {
            double_colon_pos = 0;
            p += 2;
        }

        while (*p && word_count < 8) {
            unsigned long val = 0;
            const char* start = p;
            while ((*p >= '0' && *p <= '9') || (*p >= 'a' && *p <= 'f') || (*p >= 'A' && *p <= 'F')) {
                val = (val << 4) | ((*p >= '0' && *p <= '9') ? (unsigned long)(*p - '0') : ((*p >= 'a' && *p <= 'f') ? (unsigned long)(*p - 'a' + 10) : (unsigned long)(*p - 'A' + 10)));
                p++;
            }
            if (p == start || val > 0xFFFF) return 0;
            words[word_count++] = (uint16_t)val;
            if (*p == ':') {
                p++;
                if (*p == ':') {
                    if (double_colon_pos >= 0) return 0;
                    double_colon_pos = word_count;
                    p++;
                }
            } else if (*p != '\0') {
                return 0;
            }
        }
        if (*p != '\0') return 0;

        if (double_colon_pos >= 0) {
            int fill = 8 - word_count;
            int j = 0;
            for (int i = 0; i < double_colon_pos; i++) {
                out[j++] = (words[i] >> 8) & 0xFF;
                out[j++] = words[i] & 0xFF;
            }
            for (int i = 0; i < fill; i++) {
                out[j++] = 0;
                out[j++] = 0;
            }
            for (int i = double_colon_pos; i < word_count; i++) {
                out[j++] = (words[i] >> 8) & 0xFF;
                out[j++] = words[i] & 0xFF;
            }
        } else if (word_count == 8) {
            for (int i = 0; i < 8; i++) {
                out[i * 2] = (words[i] >> 8) & 0xFF;
                out[i * 2 + 1] = words[i] & 0xFF;
            }
        } else {
            return 0;
        }
        return 1;
    }
    errno = EAFNOSUPPORT;
    return -1;
}

__attribute__((weak)) long syscall(long number, ...)
{
    (void)number;
    errno = ENOSYS;
    return -1;
}

__attribute__((weak)) struct servent* getservbyname(const char* name, const char* proto)
{
    (void)name; (void)proto;
    return NULL;
}

__attribute__((weak)) struct servent* getservbyport(int port, const char* proto)
{
    (void)port; (void)proto;
    return NULL;
}

__attribute__((weak)) ssize_t readv(int fd, const struct iovec* iov, int iovcnt)
{
    (void)fd; (void)iov; (void)iovcnt;
    return -1;
}

__attribute__((weak)) ssize_t writev(int fd, const struct iovec* iov, int iovcnt)
{
    (void)fd; (void)iov; (void)iovcnt;
    return -1;
}

int h_errno = 0;

__attribute__((weak)) ssize_t sendmsg(int sockfd, const struct msghdr* msg, int flags)
{
    (void)sockfd; (void)msg; (void)flags;
    return -1;
}

__attribute__((weak)) ssize_t recvmsg(int sockfd, struct msghdr* msg, int flags)
{
    (void)sockfd; (void)msg; (void)flags;
    return -1;
}

__attribute__((weak)) const char* gai_strerror(int errcode)
{
    (void)errcode;
    return "Unknown resolver error";
}

__attribute__((weak)) int getnameinfo(const struct sockaddr* sa, socklen_t salen, char* host, socklen_t hostlen, char* serv, socklen_t servlen, int flags)
{
    (void)sa; (void)salen; (void)host; (void)hostlen; (void)serv; (void)servlen; (void)flags;
    return -1;
}

__attribute__((weak)) int _link(const char* oldpath, const char* newpath)
{
    (void)oldpath; (void)newpath;
    errno = ENOSYS;
    return -1;
}
__attribute__((weak)) int link(const char* oldpath, const char* newpath) { return _link(oldpath, newpath); }

__attribute__((weak)) void* getgrgid(unsigned int gid) { (void)gid; return NULL; }
__attribute__((weak)) void* getgrnam(const char* name) { (void)name; return NULL; }
__attribute__((weak)) void* getpwuid(unsigned int uid) { (void)uid; return NULL; }
__attribute__((weak)) void* getpwnam(const char* name) { (void)name; return NULL; }

__attribute__((weak)) int _mkdir(const char* pathname, mode_t mode) { (void)pathname; (void)mode; errno = ENOSYS; return -1; }
__attribute__((weak)) int mkdir(const char* pathname, mode_t mode) { return _mkdir(pathname, mode); }

__attribute__((weak)) int _chmod(const char* pathname, mode_t mode) { (void)pathname; (void)mode; errno = ENOSYS; return -1; }
__attribute__((weak)) int chmod(const char* pathname, mode_t mode) { return _chmod(pathname, mode); }

__attribute__((weak)) int _chown(const char* pathname, uid_t owner, gid_t group) { (void)pathname; (void)owner; (void)group; errno = ENOSYS; return -1; }
__attribute__((weak)) int chown(const char* pathname, uid_t owner, gid_t group) { return _chown(pathname, owner, group); }

struct statvfs;
struct passwd;
struct rusage;

__attribute__((weak)) int _execve(const char* p, char* const a[], char* const e[]) { (void)p; (void)a; (void)e; errno = ENOSYS; return -1; }
__attribute__((weak)) int execv(const char* p, char* const a[]) { (void)p; (void)a; errno = ENOSYS; return -1; }
__attribute__((weak)) pid_t _fork(void) { errno = ENOSYS; return -1; }
__attribute__((weak)) pid_t _wait(int* status) { (void)status; errno = ENOSYS; return -1; }
__attribute__((weak)) pid_t waitpid(pid_t pid, int* status, int options) { (void)pid; (void)status; (void)options; errno = ENOSYS; return -1; }

__attribute__((weak)) int access(const char* path, int mode) { (void)path; (void)mode; return 0; }
__attribute__((weak)) int eaccess(const char* path, int mode) { (void)path; (void)mode; return 0; }
__attribute__((weak)) int chdir(const char* path) { (void)path; return 0; }
__attribute__((weak)) int creat(const char* path, mode_t mode) { (void)path; (void)mode; errno = ENOSYS; return -1; }
__attribute__((weak)) int dup(int oldfd) { (void)oldfd; errno = ENOSYS; return -1; }
__attribute__((weak)) int dup2(int oldfd, int newfd) { (void)oldfd; (void)newfd; errno = ENOSYS; return -1; }
__attribute__((weak)) int fchmod(int fd, mode_t mode) { (void)fd; (void)mode; return 0; }
__attribute__((weak)) int fchown(int fd, uid_t owner, gid_t group) { (void)fd; (void)owner; (void)group; return 0; }
__attribute__((weak)) int flock(int fd, int operation) { (void)fd; (void)operation; return 0; }
__attribute__((weak)) int fnmatch(const char* pattern, const char* string, int flags) { (void)pattern; (void)string; (void)flags; return 1; }
__attribute__((weak)) int fstatat(int dirfd, const char* pathname, struct stat* statbuf, int flags) { (void)dirfd; (void)pathname; (void)statbuf; (void)flags; errno = ENOENT; return -1; }
__attribute__((weak)) int openat(int dirfd, const char* pathname, int flags, ...) { (void)dirfd; (void)pathname; (void)flags; errno = ENOENT; return -1; }
__attribute__((weak)) int pipe(int pipefd[2]) { (void)pipefd; errno = ENOSYS; return -1; }
__attribute__((weak)) ssize_t pread(int fd, void* buf, size_t count, off_t offset) { (void)fd; (void)buf; (void)count; (void)offset; errno = ENOSYS; return -1; }
__attribute__((weak)) ssize_t pwrite(int fd, const void* buf, size_t count, off_t offset) { (void)fd; (void)buf; (void)count; (void)offset; errno = ENOSYS; return -1; }
__attribute__((weak)) ssize_t readlink(const char* pathname, char* buf, size_t bufsiz) { (void)pathname; (void)buf; (void)bufsiz; errno = ENOENT; return -1; }
__attribute__((weak)) char* realpath(const char* path, char* resolved_path) {
    if (!path) { errno = EINVAL; return NULL; }
    if (resolved_path) {
        strncpy(resolved_path, path, 1024);
        return resolved_path;
    }
    return strdup(path);
}
__attribute__((weak)) int rmdir(const char* pathname) { (void)pathname; errno = ENOSYS; return -1; }
__attribute__((weak)) int statvfs(const char* path, struct statvfs* buf) { (void)path; (void)buf; errno = ENOSYS; return -1; }
__attribute__((weak)) int symlink(const char* target, const char* linkpath) { (void)target; (void)linkpath; errno = ENOSYS; return -1; }
__attribute__((weak)) int utimes(const char* filename, const void* times) { (void)filename; (void)times; return 0; }

__attribute__((weak)) void* opendir(const char* name) { (void)name; errno = ENOENT; return NULL; }
__attribute__((weak)) void* fdopendir(int fd) { (void)fd; errno = EBADF; return NULL; }
__attribute__((weak)) void* readdir(void* dirp) { (void)dirp; return NULL; }
__attribute__((weak)) void rewinddir(void* dirp) { (void)dirp; }
__attribute__((weak)) int closedir(void* dirp) { (void)dirp; return 0; }

__attribute__((weak)) uid_t getuid(void) { return 0; }
__attribute__((weak)) uid_t geteuid(void) { return 0; }
__attribute__((weak)) gid_t getgid(void) { return 0; }
__attribute__((weak)) gid_t getegid(void) { return 0; }
__attribute__((weak)) pid_t getppid(void) { return 1; }
__attribute__((weak)) int setuid(uid_t uid) { (void)uid; return 0; }
__attribute__((weak)) pid_t setsid(void) { return 1; }
__attribute__((weak)) int getpwnam_r(const char* name, struct passwd* pwd, char* buf, size_t buflen, struct passwd** result) {
    (void)name; (void)pwd; (void)buf; (void)buflen;
    if (result) *result = NULL;
    return ENOENT;
}
__attribute__((weak)) int getrusage(int who, struct rusage* usage) {
    (void)who;
    if (usage) memset(usage, 0, 128);
    return 0;
}
__attribute__((weak)) int gethostname(char* name, size_t len) {
    if (name && len > 0) {
        strncpy(name, "alloyos", len - 1);
        name[len - 1] = '\0';
        return 0;
    }
    return -1;
}

__attribute__((weak)) in_addr_t inet_addr(const char* cp) {
    struct in_addr addr;
    if (inet_aton(cp, &addr)) return addr.s_addr;
    return (in_addr_t)(-1);
}
__attribute__((weak)) const char* inet_ntop(int af, const void* src, char* dst, socklen_t size) {
    if (af == AF_INET) {
        if (!src || !dst || size < 16) {
            errno = ENOSPC;
            return NULL;
        }
        const unsigned char* b = (const unsigned char*)src;
        char* p = dst;
        for (int i = 0; i < 4; ++i) {
            unsigned char v = b[i];
            if (v >= 100) {
                *p++ = (char)('0' + (v / 100));
                v %= 100;
                *p++ = (char)('0' + (v / 10));
                v %= 10;
                *p++ = (char)('0' + v);
            } else if (v >= 10) {
                *p++ = (char)('0' + (v / 10));
                v %= 10;
                *p++ = (char)('0' + v);
            } else {
                *p++ = (char)('0' + v);
            }
            if (i < 3) *p++ = '.';
        }
        *p = '\0';
        return dst;
    }
    errno = EAFNOSUPPORT;
    return NULL;
}
__attribute__((weak)) int select(int nfds, fd_set* readfds, fd_set* writefds, fd_set* exceptfds, struct timeval* timeout) {
    (void)readfds; (void)writefds; (void)exceptfds; (void)timeout;
    return nfds > 0 ? 1 : 0;
}

__attribute__((weak)) int getaddrinfo(const char* node, const char* service, const struct addrinfo* hints, struct addrinfo** res) {
    if (!res) return EAI_NONAME;
    *res = NULL;

    uint16_t port = 80;
    if (service) {
        if (strcmp(service, "https") == 0 || strcmp(service, "443") == 0) {
            port = 443;
        } else if (strcmp(service, "http") == 0 || strcmp(service, "80") == 0) {
            port = 80;
        } else {
            uint16_t p = 0;
            const char* s = service;
            while (*s >= '0' && *s <= '9') {
                p = (uint16_t)(p * 10 + (*s - '0'));
                s++;
            }
            port = (p > 0) ? p : 80;
        }
    }

    uint8_t ip[4] = {10, 0, 2, 2}; // Default to QEMU gateway / host
    if (node) {
        if (strcmp(node, "localhost") == 0 || strcmp(node, "127.0.0.1") == 0) {
            ip[0] = 127; ip[1] = 0; ip[2] = 0; ip[3] = 1;
        } else if (strcmp(node, "generativelanguage.googleapis.com") == 0) {
            ip[0] = 142; ip[1] = 250; ip[2] = 190; ip[3] = 74;
        } else if (strcmp(node, "api.anthropic.com") == 0) {
            ip[0] = 160; ip[1] = 79; ip[2] = 104; ip[3] = 10;
        } else if (strcmp(node, "api.openai.com") == 0) {
            ip[0] = 162; ip[1] = 159; ip[2] = 140; ip[3] = 245;
        } else if (inet_aton(node, (struct in_addr*)ip)) {
            // Parsed successfully into ip
        } else {
            ip[0] = 10; ip[1] = 0; ip[2] = 2; ip[3] = 2;
        }
    }

    struct addrinfo* ai = (struct addrinfo*)calloc(1, sizeof(struct addrinfo));
    struct sockaddr_in* sin = (struct sockaddr_in*)calloc(1, sizeof(struct sockaddr_in));
    if (!ai || !sin) {
        if (ai) free(ai);
        if (sin) free(sin);
        return EAI_MEMORY;
    }

    sin->sin_family = AF_INET;
    sin->sin_port = htons(port);
    memcpy(&sin->sin_addr.s_addr, ip, 4);

    ai->ai_family = AF_INET;
    ai->ai_socktype = (hints && hints->ai_socktype) ? hints->ai_socktype : SOCK_STREAM;
    ai->ai_protocol = (hints && hints->ai_protocol) ? hints->ai_protocol : IPPROTO_TCP;
    ai->ai_addrlen = sizeof(struct sockaddr_in);
    ai->ai_addr = (struct sockaddr*)sin;
    ai->ai_canonname = strdup(node ? node : "");
    ai->ai_next = NULL;

    *res = ai;
    return 0;
}

__attribute__((weak)) void freeaddrinfo(struct addrinfo* res) {
    while (res) {
        struct addrinfo* next = res->ai_next;
        if (res->ai_addr) free(res->ai_addr);
        if (res->ai_canonname) free(res->ai_canonname);
        free(res);
        res = next;
    }
}

__attribute__((weak)) char* getenv(const char* name) { (void)name; return NULL; }
__attribute__((weak)) char* strdup(const char* s) {
    if (!s) return NULL;
    size_t len = strlen(s);
    char* copy = (char*)malloc(len + 1);
    if (copy) {
        memcpy(copy, s, len + 1);
    }
    return copy;
}

__attribute__((weak)) char* secure_getenv(const char* name) { return getenv(name); }
__attribute__((weak)) char* initstate(unsigned int seed, char* statebuf, size_t n) { (void)seed; (void)n; return statebuf; }
__attribute__((weak)) char* setstate(char* statebuf) { return statebuf; }











