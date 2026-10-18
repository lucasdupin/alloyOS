// Freestanding compiler/ELF probe, not a replacement for libc++ or JavaScriptCore.
#include <stddef.h>
#include <stdint.h>
#include "expression.h"

static int64_t call(uint64_t number, uint64_t a = 0, uint64_t b = 0, uint64_t c = 0)
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

// Keep a separate translation unit call to exercise real AArch64 C++ dispatch.
extern "C" __attribute__((section(".text._start"), noreturn))
void _start(uint64_t heap, uint64_t length)
{
    (void)heap;
    (void)length;
    int status = 72;
    if (call(402) == 1) {
        Product expression(6, 7);
        status = evaluate(expression) == 42 ? 0 : 73;
        if (!status) {
            // The protected ABI has no stdout capability. Its bounded diagnostic
            // syscall currently labels every report "user-panic" in the log.
            static const char message[] = "CXX_NATIVE_PASS virtual-dispatch=42";
            if (call(403, reinterpret_cast<uint64_t>(message), sizeof(message) - 1) != 0)
                status = 74;
        }
    }
    call(60, static_cast<uint64_t>(status));
    for (;;) asm volatile("yield");
}
