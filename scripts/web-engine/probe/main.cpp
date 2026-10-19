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
