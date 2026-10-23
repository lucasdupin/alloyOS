// Freestanding compiler/ELF probe, not a replacement for libc++ or JavaScriptCore.
#include <stddef.h>
#include <stdint.h>
#include "expression.h"

extern "C" int64_t call(uint64_t number, uint64_t a = 0, uint64_t b = 0, uint64_t c = 0)
{
    register uint64_t x0 asm("x0") = a;
    register uint64_t x1 asm("x1") = b;
    register uint64_t x2 asm("x2") = c;
    register uint64_t x8 asm("x8") = number;
    asm volatile("svc #0" : "+r"(x0) : "r"(x1), "r"(x2), "r"(x8) : "memory", "cc");
    return static_cast<int64_t>(x0);
}

static uint32_t heap_allocated_blocks;
static uint32_t heap_freed_blocks;

void* operator new(size_t size)
{
    size_t pages = ((size + 16 + 4095) / 4096) * 4096;
    int64_t address = call(404, pages, 0, 0);
    if (address <= 0 || call(405, static_cast<uint64_t>(address), pages, 3) != 0) {
        call(60, 81);
        for (;;) asm volatile("yield");
    }
    *reinterpret_cast<uint64_t*>(address) = pages;
    __atomic_fetch_add(&heap_allocated_blocks, 1, __ATOMIC_ACQ_REL);
    return reinterpret_cast<void*>(address + 16);
}

void operator delete(void* ptr) noexcept
{
    if (!ptr)
        return;
    uint64_t base = reinterpret_cast<uint64_t>(ptr) - 16;
    uint64_t pages = *reinterpret_cast<uint64_t*>(base);
    if (call(407, base, pages, 0) != 0) {
        call(60, 82);
        for (;;) asm volatile("yield");
    }
    __atomic_fetch_add(&heap_freed_blocks, 1, __ATOMIC_ACQ_REL);
}

void operator delete(void* ptr, size_t) noexcept
{
    operator delete(ptr);
}

// AArch64 Itanium C++ ABI static-local guard variables:
// bit 0 is initialized, bit 1 is in-progress lock.
extern "C" int __cxa_guard_acquire(uint64_t* guard)
{
    uint32_t* word = reinterpret_cast<uint32_t*>(guard);
    for (;;) {
        uint32_t state = __atomic_load_n(word, __ATOMIC_ACQUIRE);
        if (state & 1)
            return 0;
        if (state == 0) {
            uint32_t expected = 0;
            if (__atomic_compare_exchange_n(word, &expected, 2, false, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE))
                return 1;
            continue;
        }
        call(411, reinterpret_cast<uint64_t>(word), 2, 0);
    }
}

extern "C" void __cxa_guard_release(uint64_t* guard)
{
    uint32_t* word = reinterpret_cast<uint32_t*>(guard);
    __atomic_store_n(word, 1, __ATOMIC_RELEASE);
    call(412, reinterpret_cast<uint64_t>(word), INT32_MAX, 0);
}

extern "C" void __cxa_guard_abort(uint64_t* guard)
{
    uint32_t* word = reinterpret_cast<uint32_t*>(guard);
    __atomic_store_n(word, 0, __ATOMIC_RELEASE);
    call(412, reinterpret_cast<uint64_t>(word), INT32_MAX, 0);
}

class Product final : public Expression {
    int left;
    int right;
public:
    Product(int a, int b) : left(a), right(b) { }
    int evaluate() const override { return left * right; }
};

static volatile int constructor_count;
struct Bootstrap {
    Bootstrap() { constructor_count = constructor_count + 1; }
};
static Bootstrap bootstrap;
alignas(64) thread_local int tls_initialized = 7;
thread_local int tls_zero;

extern "C" char __tdata_start[];
extern "C" char __tdata_end[];
extern "C" char __tbss_end[];

static uint32_t static_local_init_calls;
struct SharedSingleton {
    int value;
    SharedSingleton()
    {
        uint32_t count = __atomic_fetch_add(&static_local_init_calls, 1, __ATOMIC_ACQ_REL) + 1;
        value = static_cast<int>(count * 100 + 23);
    }
};

static int shared_singleton_value()
{
    static SharedSingleton instance;
    return instance.value;
}

struct ThreadSharedState {
    alignas(4) uint32_t child_ready;
    alignas(4) uint32_t parent_ready;
    int64_t parent_pid;
    int64_t parent_tid;
    int64_t child_pid;
    int64_t child_tid;
    int child_singleton;
    int child_product;
    int child_tls_ok;
};

extern "C" __attribute__((noreturn)) void cxx_child_thread(uint64_t arg)
{
    ThreadSharedState* shared = reinterpret_cast<ThreadSharedState*>(arg);
    shared->child_pid = call(39);
    shared->child_tid = call(413);
    // Fresh child TLS block must start with the ELF template values (0 and 7).
    int initial_ok = check_tls(0);
    int child_canary = static_cast<int>(shared->child_tid * 16 + 3);
    tls_zero = child_canary;
    tls_initialized = child_canary + 7;
    Expression* expr = new Product(8, 9);
    shared->child_product = evaluate(*expr);
    delete expr;
    shared->child_singleton = shared_singleton_value();
    __atomic_store_n(&shared->child_ready, 1, __ATOMIC_RELEASE);
    call(412, reinterpret_cast<uint64_t>(&shared->child_ready), 1, 0);
    while (__atomic_load_n(&shared->parent_ready, __ATOMIC_ACQUIRE) == 0)
        call(411, reinterpret_cast<uint64_t>(&shared->parent_ready), 0, 0);
    shared->child_tls_ok = initial_ok && check_tls(child_canary);
    int exit_code = (shared->child_tls_ok && shared->child_product == 72 && shared->child_singleton == 123) ? 37 : 91;
    call(409, static_cast<uint64_t>(exit_code));
    for (;;) asm volatile("yield");
}

static bool verify_cxx_thread(int parent_canary)
{
    constexpr size_t stack_bytes = 4 * 4096;
    constexpr size_t tls_bytes = 4096;
    int64_t region = call(404, stack_bytes + tls_bytes, 0, 0);
    if (region <= 0 || call(405, static_cast<uint64_t>(region), stack_bytes + tls_bytes, 3) != 0)
        return false;
    uint64_t stack_base = static_cast<uint64_t>(region);
    uint64_t child_tp = stack_base + stack_bytes;
    size_t tdata_len = static_cast<size_t>(__tdata_end - __tdata_start);
    size_t total_tls = static_cast<size_t>(__tbss_end - __tdata_start);
    if (64 + total_tls > tls_bytes)
        return false;
    // Variant I static TLS layout: 16-byte TCB at TP, followed by 64-byte aligned template.
    char* dst = reinterpret_cast<char*>(child_tp + 64);
    for (size_t i = 0; i < tdata_len; ++i)
        dst[i] = __tdata_start[i];

    ThreadSharedState shared = {};
    shared.parent_pid = call(39);
    shared.parent_tid = call(413);
    if (shared.parent_pid != shared.parent_tid)
        return false;

    Expression* parent_expr = new Product(6, 7);
    int parent_val = evaluate(*parent_expr);
    delete parent_expr;
    if (parent_val != 42)
        return false;

    uint64_t descriptor[6] = {
        reinterpret_cast<uint64_t>(&cxx_child_thread),
        reinterpret_cast<uint64_t>(&shared),
        stack_base,
        stack_bytes,
        child_tp,
        0,
    };
    int64_t tid = call(408, reinterpret_cast<uint64_t>(descriptor), sizeof(descriptor), 0);
    if (tid <= 0 || tid == shared.parent_tid)
        return false;

    int parent_singleton = shared_singleton_value();
    while (__atomic_load_n(&shared.child_ready, __ATOMIC_ACQUIRE) == 0) {
        int64_t w = call(411, reinterpret_cast<uint64_t>(&shared.child_ready), 0, 0);
        if (w != 0 && w != -11)
            return false;
    }
    // Parent TLS must be untouched by child thread's TLS writes.
    if (!check_tls(parent_canary))
        return false;
    __atomic_store_n(&shared.parent_ready, 1, __ATOMIC_RELEASE);
    if (call(412, reinterpret_cast<uint64_t>(&shared.parent_ready), 1, 0) < 0)
        return false;

    int32_t child_status = 0;
    for (;;) {
        int64_t joined = call(410, static_cast<uint64_t>(tid), reinterpret_cast<uint64_t>(&child_status), 0);
        if (joined == 0)
            break;
        if (joined != -11)
            return false;
        call(24);
    }
    if (call(407, stack_base, stack_bytes + tls_bytes, 0) != 0)
        return false;

    return child_status == 37
        && shared.child_pid == shared.parent_pid
        && shared.child_tid == tid
        && shared.child_tls_ok == 1
        && parent_singleton == 123
        && __atomic_load_n(&static_local_init_calls, __ATOMIC_ACQUIRE) == 1
        && __atomic_load_n(&heap_allocated_blocks, __ATOMIC_ACQUIRE) == 2
        && __atomic_load_n(&heap_freed_blocks, __ATOMIC_ACQUIRE) == 2
        && check_tls(parent_canary);
}

using Initializer = void (*)();
extern "C" Initializer __init_array_start[];
extern "C" Initializer __init_array_end[];
#ifdef ALLOYOS_PROBE_NEWLIB
extern "C" int check_libc(uintptr_t, size_t);
#endif

static bool monotonic(uint64_t& result)
{
    struct Timespec { int64_t seconds; int64_t nanoseconds; } time;
    if (call(228, 1, reinterpret_cast<uint64_t>(&time)) || time.seconds < 0
        || time.nanoseconds < 0 || time.nanoseconds >= 1000000000
        || static_cast<uint64_t>(time.seconds) > UINT64_MAX / 1000000000)
        return false;
    result = static_cast<uint64_t>(time.seconds) * 1000000000 + time.nanoseconds;
    return true;
}

// Keep a separate translation unit call to exercise real AArch64 C++ dispatch.
extern "C" __attribute__((section(".text._start"), noreturn))
void _start(uint64_t heap, uint64_t length)
{
    (void)heap;
    (void)length;
    int status = 72;
    if (call(402) == 1) {
        for (Initializer* init = __init_array_start; init != __init_array_end; ++init)
            (*init)();
        Product expression(6, 7);
        status = evaluate(expression) == 42 ? 0 : 73;
        if (!status && constructor_count != 1)
            status = 75;
        if (!status && !check_tls(0))
            status = 76;
        if (!status) {
            int canary = static_cast<int>(call(39));
            if (canary <= 0)
                status = 77;
            tls_zero = canary;
            tls_initialized = canary + 7;
            for (int iteration = 0; !status && iteration != 8; ++iteration) {
                // Both explicit rescheduling and timer preemption must preserve TP.
                call(24);
                uint64_t start, now;
                if (!monotonic(start)) {
                    status = 80;
                    break;
                }
                do {
                    // Spend the interval executing user instructions between
                    // clock samples. No cooperative yield inside this interval.
                    for (int spin = 0; spin != 100000; ++spin)
                        asm volatile("" ::: "memory");
                    if (!check_tls(canary)) {
                        status = 78;
                        break;
                    }
                    if (!monotonic(now) || now < start) {
                        status = 80;
                        break;
                    }
                } while (now - start < 20000000);
            }
            if (!status && !verify_cxx_thread(canary))
                status = 83;
        }
#ifdef ALLOYOS_PROBE_NEWLIB
        if (!status && check_libc(heap, length))
            status = 79;
#endif
        if (!status) {
            // The protected ABI has no stdout capability. Its bounded diagnostic
            // syscall currently labels every report "user-panic" in the log.
            static const char message[] = "CXX_NATIVE_PASS virtual-dispatch=42 constructors=1 tls=pid";
            if (call(403, reinterpret_cast<uint64_t>(message), sizeof(message) - 1) != 0)
                status = 74;
            static const char thread_message[] = "CXX_THREAD_PASS tid!=pid tls=isolated guard=1 new_delete=2";
            if (!status && call(403, reinterpret_cast<uint64_t>(thread_message), sizeof(thread_message) - 1) != 0)
                status = 74;
#ifdef ALLOYOS_PROBE_NEWLIB
            static const char libc_message[] = "LIBC_NATIVE_PASS memcpy memmove strtod sin";
            if (!status && call(403, reinterpret_cast<uint64_t>(libc_message), sizeof(libc_message) - 1) != 0)
                status = 74;
#endif
        }
    }
    call(60, static_cast<uint64_t>(status));
    for (;;) asm volatile("yield");
}
