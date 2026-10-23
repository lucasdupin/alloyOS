#ifndef ALLOYOS_SYSROOT_VERIFY_H
#define ALLOYOS_SYSROOT_VERIFY_H

#include <stddef.h>
#include <stdint.h>

class SysrootEvaluator {
public:
    virtual ~SysrootEvaluator() = default;
    virtual int compute(int a, int b) const = 0;
};

struct alignas(64) AlignedPayload {
    uint64_t values[8];
};

SysrootEvaluator* create_sysroot_evaluator(int multiplier);
int run_sysroot_worker_checks(void);

#endif /* ALLOYOS_SYSROOT_VERIFY_H */
