// Probe-only bounded heap hook. This is not the production libc adapter.
#include <errno.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>

extern "C" int64_t call(uint64_t, uint64_t, uint64_t, uint64_t);

extern "C" __attribute__((noreturn)) void _exit(int status)
{
    call(60, static_cast<uint64_t>(status), 0, 0);
    for (;;) asm volatile("yield");
}

// There are deliberately no descriptor or signal capabilities in this probe.
// Report honest failures if libc reaches those paths instead of faking success.
extern "C" int _close(int) { errno = EBADF; return -1; }
extern "C" int _fstat(int, struct stat*) { errno = EBADF; return -1; }
extern "C" off_t _lseek(int, off_t, int) { errno = EBADF; return -1; }
extern "C" ssize_t _read(int, void*, size_t) { errno = EBADF; return -1; }
extern "C" ssize_t _write(int, const void*, size_t) { errno = EBADF; return -1; }
extern "C" int _isatty(int) { errno = EBADF; return 0; }
extern "C" int _kill(int, int) { errno = ENOSYS; return -1; }
extern "C" int _getpid() { return static_cast<int>(call(39, 0, 0, 0)); }

static uintptr_t heap_begin;
static uintptr_t heap_end;
static uintptr_t heap_current;

extern "C" void* _sbrk(ptrdiff_t increment)
{
    if ((increment >= 0 && static_cast<uintptr_t>(increment) > heap_end - heap_current)
        || (increment < 0 && static_cast<uintptr_t>(-(increment + 1)) + 1 > heap_current - heap_begin)) {
        errno = ENOMEM;
        return reinterpret_cast<void*>(-1);
    }
    uintptr_t previous = heap_current;
    heap_current += increment;
    return reinterpret_cast<void*>(previous);
}

extern "C" int check_libc(uintptr_t heap, size_t length)
{
    if (!heap || !length || length > UINTPTR_MAX - heap)
        return 1;
    heap_begin = heap_current = heap;
    heap_end = heap + length;
    char buffer[16];
    memcpy(buffer, "abcdef", 7);
    memmove(buffer + 1, buffer, 6);
    if (memcmp(buffer, "aabcdef", 7))
        return 2;
    char* end = nullptr;
    double number = strtod("12.5xyz", &end);
    if (number != 12.5 || !end || strcmp(end, "xyz"))
        return 3;
    volatile double angle = 0.5;
    double sine = sin(angle);
    if (sine < 0.47942553860 || sine > 0.47942553861)
        return 4;
    return 0;
}
