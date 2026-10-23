#include <cxxabi.h>
#include <new>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <alloyos/syscall.h>

static void* alloc_or_abort(size_t size, size_t alignment, bool abort_on_failure) noexcept
{
    if (size == 0U) {
        size = 1U;
    }
    if (alignment < sizeof(void*)) {
        alignment = sizeof(void*);
    }
    void* ptr = nullptr;
    int rc = posix_memalign(&ptr, alignment, size);
    if (rc != 0 || !ptr) {
        if (abort_on_failure) {
            static const char msg[] = "alloyOS C++ operator new out of memory";
            __alloyos_panic_report(msg, sizeof(msg) - 1U);
            abort();
        }
        return nullptr;
    }
    return ptr;
}

void* operator new(std::size_t size)
{
    return alloc_or_abort(size, alignof(std::max_align_t), true);
}

void* operator new[](std::size_t size)
{
    return alloc_or_abort(size, alignof(std::max_align_t), true);
}

void* operator new(std::size_t size, const std::nothrow_t&) noexcept
{
    return alloc_or_abort(size, alignof(std::max_align_t), false);
}

void* operator new[](std::size_t size, const std::nothrow_t&) noexcept
{
    return alloc_or_abort(size, alignof(std::max_align_t), false);
}

void* operator new(std::size_t size, std::align_val_t alignment)
{
    return alloc_or_abort(size, static_cast<size_t>(alignment), true);
}

void* operator new[](std::size_t size, std::align_val_t alignment)
{
    return alloc_or_abort(size, static_cast<size_t>(alignment), true);
}

void* operator new(std::size_t size, std::align_val_t alignment, const std::nothrow_t&) noexcept
{
    return alloc_or_abort(size, static_cast<size_t>(alignment), false);
}

void* operator new[](std::size_t size, std::align_val_t alignment, const std::nothrow_t&) noexcept
{
    return alloc_or_abort(size, static_cast<size_t>(alignment), false);
}

void operator delete(void* ptr) noexcept
{
    free(ptr);
}

void operator delete[](void* ptr) noexcept
{
    free(ptr);
}

void operator delete(void* ptr, std::size_t) noexcept
{
    free(ptr);
}

void operator delete[](void* ptr, std::size_t) noexcept
{
    free(ptr);
}

void operator delete(void* ptr, const std::nothrow_t&) noexcept
{
    free(ptr);
}

void operator delete[](void* ptr, const std::nothrow_t&) noexcept
{
    free(ptr);
}

void operator delete(void* ptr, std::align_val_t) noexcept
{
    free(ptr);
}

void operator delete[](void* ptr, std::align_val_t) noexcept
{
    free(ptr);
}

void operator delete(void* ptr, std::size_t, std::align_val_t) noexcept
{
    free(ptr);
}

void operator delete[](void* ptr, std::size_t, std::align_val_t) noexcept
{
    free(ptr);
}

void operator delete(void* ptr, std::align_val_t, const std::nothrow_t&) noexcept
{
    free(ptr);
}

void operator delete[](void* ptr, std::align_val_t, const std::nothrow_t&) noexcept
{
    free(ptr);
}

extern "C" {

void* __dso_handle = &__dso_handle;

void __cxa_pure_virtual(void)
{
    static const char msg[] = "alloyOS __cxa_pure_virtual called";
    __alloyos_panic_report(msg, sizeof(msg) - 1U);
    abort();
}

void __cxa_deleted_virtual(void)
{
    static const char msg[] = "alloyOS __cxa_deleted_virtual called";
    __alloyos_panic_report(msg, sizeof(msg) - 1U);
    abort();
}

/* Itanium / ARM64 C++ ABI static initialization guards.
 * Bit 0 indicates initialization complete; 0x100 indicates in-progress.
 */
int __cxa_guard_acquire(uint64_t* guard_object)
{
    if (!guard_object) {
        return 0;
    }
    uint32_t* word = reinterpret_cast<uint32_t*>(guard_object);
    for (;;) {
        uint32_t state = __atomic_load_n(word, __ATOMIC_ACQUIRE);
        if ((state & 1U) != 0U) {
            return 0;
        }
        if (state == 0U) {
            uint32_t expected = 0U;
            if (__atomic_compare_exchange_n(word, &expected, 0x100U,
                                            0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) {
                return 1;
            }
            continue;
        }
        int64_t rc = alloyos_syscall3(ALLOYOS_SYS_WORD_WAIT,
                                      reinterpret_cast<uint64_t>(word),
                                      0x100ULL,
                                      0ULL);
        if (rc == -1) {
            (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
        }
    }
}

void __cxa_guard_release(uint64_t* guard_object)
{
    if (!guard_object) {
        return;
    }
    uint32_t* word = reinterpret_cast<uint32_t*>(guard_object);
    __atomic_store_n(word, 1U, __ATOMIC_RELEASE);
    (void)alloyos_syscall3(ALLOYOS_SYS_WORD_WAKE,
                           reinterpret_cast<uint64_t>(word),
                           0x7fffffffULL,
                           0ULL);
}

void __cxa_guard_abort(uint64_t* guard_object)
{
    if (!guard_object) {
        return;
    }
    uint32_t* word = reinterpret_cast<uint32_t*>(guard_object);
    __atomic_store_n(word, 0U, __ATOMIC_RELEASE);
    (void)alloyos_syscall3(ALLOYOS_SYS_WORD_WAKE,
                           reinterpret_cast<uint64_t>(word),
                           0x7fffffffULL,
                           0ULL);
}

#define ALLOYOS_MAX_ATEXIT_ENTRIES 128

struct atexit_entry {
    void (*destructor)(void*);
    void* arg;
    void* dso_handle;
    uint32_t active;
};

static atexit_entry g_atexit_table[ALLOYOS_MAX_ATEXIT_ENTRIES];
static size_t g_atexit_count = 0;
static uint32_t g_atexit_lock = 0;

static void atexit_lock_acquire(void)
{
    uint32_t expected = 0;
    while (!__atomic_compare_exchange_n(&g_atexit_lock, &expected, 1U,
                                        1, __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
        expected = 0;
        (void)alloyos_syscall0(ALLOYOS_SYS_YIELD);
    }
}

static void atexit_lock_release(void)
{
    __atomic_store_n(&g_atexit_lock, 0U, __ATOMIC_RELEASE);
}

int __cxa_atexit(void (*destructor)(void*), void* arg, void* dso_handle)
{
    if (!destructor) {
        return -1;
    }
    atexit_lock_acquire();
    if (g_atexit_count >= ALLOYOS_MAX_ATEXIT_ENTRIES) {
        atexit_lock_release();
        return -1;
    }
    size_t idx = g_atexit_count++;
    g_atexit_table[idx].destructor = destructor;
    g_atexit_table[idx].arg = arg;
    g_atexit_table[idx].dso_handle = dso_handle;
    g_atexit_table[idx].active = 1U;
    atexit_lock_release();
    return 0;
}

void __cxa_finalize(void* dso_handle)
{
    for (;;) {
        void (*dtor)(void*) = nullptr;
        void* arg = nullptr;
        atexit_lock_acquire();
        for (size_t i = g_atexit_count; i > 0U; --i) {
            atexit_entry& entry = g_atexit_table[i - 1U];
            if (entry.active && (dso_handle == nullptr || entry.dso_handle == dso_handle)) {
                entry.active = 0U;
                dtor = entry.destructor;
                arg = entry.arg;
                break;
            }
        }
        atexit_lock_release();
        if (!dtor) {
            break;
        }
        dtor(arg);
    }
}

} /* extern "C" */
