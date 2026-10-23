#include "verify.h"
#include <alloyos/syscall.h>

namespace {

int g_global_ctor_ran = 0;

struct GlobalInitTracker {
    GlobalInitTracker() { g_global_ctor_ran += 1; }
    ~GlobalInitTracker() { g_global_ctor_ran = 0; }
};

GlobalInitTracker g_tracker;

} // namespace

int main(int, char**) {
    if (g_global_ctor_ran != 1) {
        return 1;
    }
    SysrootEvaluator* evaluator = create_sysroot_evaluator(1);
    int product = evaluator->compute(6, 7);
    delete evaluator;
    if (product != 42) {
        return 2;
    }
    int worker_rc = run_sysroot_worker_checks();
    if (worker_rc != 0) {
        return worker_rc;
    }
    static const char msg[] = "SYSROOT_CXX_PASS virtual=42 mman=ok pthread=ok time=ok";
    __alloyos_panic_report(msg, sizeof(msg) - 1U);
    return 0;
}
