#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>
#include <alloyos/syscall.h>

typedef void (*alloyos_init_fn)(void);
extern alloyos_init_fn __init_array_start[] __attribute__((weak));
extern alloyos_init_fn __init_array_end[] __attribute__((weak));
extern void __cxa_finalize(void* dso_handle) __attribute__((weak));
extern void __alloyos_heap_init(uintptr_t heap_start, size_t heap_length);
extern void __alloyos_pthread_init_main(void);
extern int main(int argc, char** argv) __attribute__((weak));
extern int _Z4mainiPPc(int argc, char** argv) __attribute__((weak));
extern int _Z4mainv(void) __attribute__((weak));

static uint32_t g_runtime_initialized = 0;

void __alloyos_runtime_init(uintptr_t heap_start, size_t heap_length)
{
    uint32_t expected = 0;
    if (!__atomic_compare_exchange_n(&g_runtime_initialized, &expected, 1U,
                                     0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
        return;
    }
    __alloyos_heap_init(heap_start, heap_length);
    __alloyos_pthread_init_main();
    if (__init_array_start && __init_array_end) {
        for (alloyos_init_fn* fn = __init_array_start; fn < __init_array_end; ++fn) {
            if (*fn) {
                (*fn)();
            }
        }
    }
    __atomic_store_n(&g_runtime_initialized, 2U, __ATOMIC_RELEASE);
}

__attribute__((weak, section(".text._start"), noreturn))
void _start(uint64_t heap_start, uint64_t heap_length)
{
    if (alloyos_syscall0(ALLOYOS_SYS_QUERY_ABI) != (int64_t)ALLOYOS_ABI_VERSION) {
        alloyos_syscall1(ALLOYOS_SYS_EXIT, 72ULL);
        for (;;) {
            __asm__ volatile("yield");
        }
    }
    __alloyos_runtime_init((uintptr_t)heap_start, (size_t)heap_length);
    int status = 73;
    static char prog_name[] = "alloyos-elf";
    char* argv[2] = { prog_name, NULL };
    if (main) {
        status = main(1, argv);
    } else if (_Z4mainiPPc) {
        status = _Z4mainiPPc(1, argv);
    } else if (_Z4mainv) {
        status = _Z4mainv();
    }
    if (__cxa_finalize) {
        __cxa_finalize(NULL);
    }
    _exit(status);
}
