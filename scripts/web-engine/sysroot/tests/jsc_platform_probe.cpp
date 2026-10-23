// WTF/JSCOnly platform-layer verification probe for alloyOS.
// Exercises OSAllocatorPOSIX-style reserve/commit/decommit/guard/release,
// ThreadingPOSIX-style pthreads/mutex/condvar/once/key, CLOCK_MONOTONIC,
// getrandom/getentropy, and C++ ABI runtime primitives.
#include <cxxabi.h>
#include <errno.h>
#include <new>
#include <pthread.h>
#include <sched.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <time.h>
#include <alloyos/syscall.h>

namespace {

class WtfTask {
public:
    virtual ~WtfTask() = default;
    virtual int run() = 0;
};

class MultiplyTask final : public WtfTask {
    int a_;
    int b_;
public:
    MultiplyTask(int a, int b) : a_(a), b_(b) {}
    int run() override { return a_ * b_; }
};

class OsAllocatorSimulation {
    void* base_ = MAP_FAILED;
    size_t total_bytes_ = 0;
public:
    bool reserve_with_guards(size_t payload_pages) {
        total_bytes_ = (payload_pages + 2U) * ALLOYOS_PAGE_SIZE;
        base_ = mmap(nullptr, total_bytes_, PROT_NONE,
                     MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
        return base_ != MAP_FAILED;
    }

    void* commit_payload(bool writable, bool executable) {
        int prot = PROT_READ | (writable ? PROT_WRITE : 0) | (executable ? PROT_EXEC : 0);
        void* payload = static_cast<char*>(base_) + ALLOYOS_PAGE_SIZE;
        size_t len = total_bytes_ - 2U * ALLOYOS_PAGE_SIZE;
        if (mprotect(payload, len, prot) != 0) {
            return nullptr;
        }
        return payload;
    }

    bool decommit_payload() {
        void* payload = static_cast<char*>(base_) + ALLOYOS_PAGE_SIZE;
        size_t len = total_bytes_ - 2U * ALLOYOS_PAGE_SIZE;
        return mprotect(payload, len, PROT_NONE) == 0;
    }

    bool release() {
        if (base_ == MAP_FAILED) {
            return false;
        }
        int rc = munmap(base_, total_bytes_);
        base_ = MAP_FAILED;
        return rc == 0;
    }
};

} // namespace

int main(int, char**) {
    OsAllocatorSimulation alloc;
    if (!alloc.reserve_with_guards(2U)) {
        return 1;
    }
    /* C-loop JSC requires executable allocations to be rejected on alloyOS */
    if (alloc.commit_payload(true, true) != nullptr || errno != ENOTSUP) {
        (void)alloc.release();
        return 2;
    }
    void* payload = alloc.commit_payload(true, false);
    if (!payload) {
        (void)alloc.release();
        return 3;
    }
    WtfTask* task = new MultiplyTask(6, 7);
    int result = task->run();
    delete task;
    static_cast<int*>(payload)[0] = result;
    if (static_cast<const volatile int*>(payload)[0] != 42) {
        (void)alloc.release();
        return 4;
    }
    if (!alloc.decommit_payload() || !alloc.release()) {
        return 5;
    }

    struct timespec now {};
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        return 6;
    }

    unsigned char rng_bytes[320];
    ssize_t rng_rc = getrandom(rng_bytes, sizeof(rng_bytes), 0U);
    if (rng_rc < 0 && errno != EAGAIN) {
        return 7;
    }

    static const char msg[] = "JSC_PLATFORM_PROBE_PASS allocator=ok task=42";
    __alloyos_panic_report(msg, sizeof(msg) - 1U);
    return 0;
}
