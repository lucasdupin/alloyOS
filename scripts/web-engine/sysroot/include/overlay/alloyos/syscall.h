#ifndef _ALLOYOS_SYSCALL_H
#define _ALLOYOS_SYSCALL_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Protected-process ABI version and syscall discriminants
 * (matches crates/alloyos-proto/src/lib.rs and crates/alloyos-proto/src/process.rs).
 */
#define ALLOYOS_ABI_VERSION          1ULL

/* Standard alloyOS SyscallNo discriminants */
#define ALLOYOS_SYS_YIELD            24ULL
#define ALLOYOS_SYS_GETPID           39ULL
#define ALLOYOS_SYS_SOCKET           41ULL
#define ALLOYOS_SYS_CONNECT          42ULL
#define ALLOYOS_SYS_SEND             44ULL
#define ALLOYOS_SYS_RECV             45ULL
#define ALLOYOS_SYS_EXIT             60ULL
#define ALLOYOS_SYS_CLOCK_GETTIME    228ULL
#define ALLOYOS_SYS_GETRANDOM        318ULL
/* Fallback Linux-AArch64 number only used if 318 returns -ENOSYS in a mock */
#define ALLOYOS_SYS_GETRANDOM_ALT    278ULL

/* Protected process ABI calls (400..413) */
#define ALLOYOS_SYS_IPC_SEND         400ULL
#define ALLOYOS_SYS_IPC_RECEIVE      401ULL
#define ALLOYOS_SYS_QUERY_ABI        402ULL
#define ALLOYOS_SYS_PANIC_REPORT     403ULL
#define ALLOYOS_SYS_MEMORY_RESERVE   404ULL
#define ALLOYOS_SYS_MEMORY_COMMIT    405ULL
#define ALLOYOS_SYS_MEMORY_DECOMMIT  406ULL
#define ALLOYOS_SYS_MEMORY_RELEASE   407ULL
#define ALLOYOS_SYS_THREAD_CREATE    408ULL
#define ALLOYOS_SYS_THREAD_EXIT      409ULL
#define ALLOYOS_SYS_THREAD_JOIN      410ULL
#define ALLOYOS_SYS_WORD_WAIT        411ULL
#define ALLOYOS_SYS_WORD_WAKE        412ULL
#define ALLOYOS_SYS_GETTID           413ULL

#define ALLOYOS_MEMORY_READ          1ULL
#define ALLOYOS_MEMORY_READ_WRITE    3ULL
#define ALLOYOS_PAGE_SIZE            4096UL
#define ALLOYOS_THREAD_DESC_BYTES    48UL

static inline int64_t alloyos_syscall0(uint64_t number)
{
    register uint64_t x0 __asm__("x0") = 0;
    register uint64_t x1 __asm__("x1") = 0;
    register uint64_t x2 __asm__("x2") = 0;
    register uint64_t x8 __asm__("x8") = number;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x1), "r"(x2), "r"(x8)
                     : "memory", "cc");
    return (int64_t)x0;
}

static inline int64_t alloyos_syscall1(uint64_t number, uint64_t a0)
{
    register uint64_t x0 __asm__("x0") = a0;
    register uint64_t x1 __asm__("x1") = 0;
    register uint64_t x2 __asm__("x2") = 0;
    register uint64_t x8 __asm__("x8") = number;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x1), "r"(x2), "r"(x8)
                     : "memory", "cc");
    return (int64_t)x0;
}

static inline int64_t alloyos_syscall2(uint64_t number, uint64_t a0, uint64_t a1)
{
    register uint64_t x0 __asm__("x0") = a0;
    register uint64_t x1 __asm__("x1") = a1;
    register uint64_t x2 __asm__("x2") = 0;
    register uint64_t x8 __asm__("x8") = number;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x1), "r"(x2), "r"(x8)
                     : "memory", "cc");
    return (int64_t)x0;
}

static inline int64_t alloyos_syscall3(uint64_t number, uint64_t a0, uint64_t a1, uint64_t a2)
{
    register uint64_t x0 __asm__("x0") = a0;
    register uint64_t x1 __asm__("x1") = a1;
    register uint64_t x2 __asm__("x2") = a2;
    register uint64_t x8 __asm__("x8") = number;
    __asm__ volatile("svc #0"
                     : "+r"(x0)
                     : "r"(x1), "r"(x2), "r"(x8)
                     : "memory", "cc");
    return (int64_t)x0;
}

static inline uintptr_t alloyos_read_tpidr_el0(void)
{
    uint64_t tp = 0;
    __asm__ volatile("mrs %0, tpidr_el0" : "=r"(tp));
    return (uintptr_t)tp;
}

void __alloyos_runtime_init(uintptr_t heap_start, size_t heap_length);
int __alloyos_map_kernel_errno(int64_t negative_kernel_error);
void __alloyos_panic_report(const char* message, size_t length);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_SYSCALL_H */
