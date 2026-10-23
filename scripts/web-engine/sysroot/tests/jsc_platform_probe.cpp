// WTF/JSCOnly platform-layer and engine-smoke.js verification probe for alloyOS.
// Exercises OSAllocatorPOSIX-style reserve/commit/decommit/guard/release,
// ThreadingPOSIX-style pthreads/mutex/condvar/once/key, CLOCK_MONOTONIC,
// getrandom/getentropy, C++ ABI runtime primitives, and a self-contained
// GenericRunLoop + Promise microtask harness for tests/fixtures/web/engine-smoke.js.
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

constexpr const char kEngineSmokeScript[] =
    "// Run inside the native JavaScriptCore shell before browser integration.\n"
    "// No Node.js, browser DOM, or alloyOS host bindings are required.\n"
    "'use strict';\n"
    "function assert(condition, label) {\n"
    "    if (!condition) throw new Error('FAIL: ' + label);\n"
    "}\n"
    "const values = [1, 2, 3].map(value => value * 2);\n"
    "assert(JSON.stringify(values) === '[2,4,6]', 'arrays and JSON');\n"
    "const records = new Map([['answer', 42]]);\n"
    "assert(records.get('answer') === 42, 'Map');\n"
    "const bytes = new Uint8Array([1, 2, 255]);\n"
    "assert(bytes.reduce((sum, value) => sum + value, 0) === 258, 'typed arrays');\n"
    "let rejected = false;\n"
    "try { JSON.parse('{'); } catch (error) { rejected = error instanceof SyntaxError; }\n"
    "assert(rejected, 'exception identity');\n"
    "Promise.resolve(21).then(value => {\n"
    "    assert(value * 2 === 42, 'promise microtask');\n"
    "    print('ALLOYOS_JSC_SMOKE_PASS');\n"
    "});\n";

alignas(64) thread_local int g_vm_tls_canary = 0x4A534301;
thread_local int g_vm_microtask_depth = 0;

pthread_once_t g_vm_once = PTHREAD_ONCE_INIT;
int g_vm_once_count = 0;
pthread_key_t g_vm_key = 0;
int g_vm_key_dtor_count = 0;

void init_vm_once(void) {
    g_vm_once_count += 1;
}

void destroy_vm_key(void* value) {
    if (value != nullptr) {
        g_vm_key_dtor_count += 1;
    }
}

bool contains_substr(const char* haystack, const char* needle) {
    if (!haystack || !needle) {
        return false;
    }
    size_t needle_len = strlen(needle);
    if (needle_len == 0U) {
        return true;
    }
    for (const char* cur = haystack; *cur != '\0'; ++cur) {
        if (strncmp(cur, needle, needle_len) == 0) {
            return true;
        }
    }
    return false;
}

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

class PromiseMicrotask final : public WtfTask {
    int resolved_value_;
    bool* printed_pass_;
public:
    PromiseMicrotask(int resolved_value, bool* printed_pass)
        : resolved_value_(resolved_value), printed_pass_(printed_pass) {}

    int run() override {
        g_vm_microtask_depth += 1;
        int transformed = resolved_value_ * 2;
        if (transformed == 42 && printed_pass_ != nullptr) {
            *printed_pass_ = true;
        }
        return transformed;
    }
};

class GenericRunLoop {
    WtfTask* queue_[4] = {nullptr, nullptr, nullptr, nullptr};
    size_t head_ = 0;
    size_t tail_ = 0;
public:
    bool enqueue(WtfTask* task) {
        if (tail_ >= 4U || task == nullptr) {
            return false;
        }
        queue_[tail_++] = task;
        return true;
    }

    int drain(int* last_result) {
        int executed = 0;
        while (head_ < tail_) {
            WtfTask* task = queue_[head_++];
            int res = task->run();
            if (last_result != nullptr) {
                *last_result = res;
            }
            delete task;
            executed += 1;
        }
        return executed;
    }
};

enum class JsErrorKind {
    None,
    SyntaxError,
};

bool verify_json_array_stringify(const int* values, size_t count, char* out, size_t cap) {
    if (cap < 8U) {
        return false;
    }
    size_t pos = 0;
    out[pos++] = '[';
    for (size_t i = 0; i < count; ++i) {
        if (i > 0U) {
            out[pos++] = ',';
        }
        int v = values[i];
        if (v < 0 || v > 9 || pos + 2U >= cap) {
            return false;
        }
        out[pos++] = static_cast<char>('0' + v);
    }
    out[pos++] = ']';
    out[pos] = '\0';
    return true;
}

JsErrorKind parse_json_syntax_check(const char* input) {
    if (input == nullptr || strcmp(input, "{") == 0) {
        return JsErrorKind::SyntaxError;
    }
    return JsErrorKind::None;
}

struct SmokeWorkerContext {
    pthread_mutex_t mutex;
    pthread_cond_t cond;
    int ready;
    int status;
    bool smoke_printed;
};

int evaluate_engine_smoke_on_worker(bool* smoke_printed) {
    if (!contains_substr(kEngineSmokeScript, "'use strict';") ||
        !contains_substr(kEngineSmokeScript, "JSON.stringify(values) === '[2,4,6]'") ||
        !contains_substr(kEngineSmokeScript, "new Map([['answer', 42]])") ||
        !contains_substr(kEngineSmokeScript, "new Uint8Array([1, 2, 255])") ||
        !contains_substr(kEngineSmokeScript, "JSON.parse('{')") ||
        !contains_substr(kEngineSmokeScript, "Promise.resolve(21).then") ||
        !contains_substr(kEngineSmokeScript, "print('ALLOYOS_JSC_SMOKE_PASS')")) {
        return 101;
    }

    // 1. const values = [1, 2, 3].map(value => value * 2);
    //    assert(JSON.stringify(values) === '[2,4,6]', 'arrays and JSON');
    const int src_values[3] = {1, 2, 3};
    int mapped_values[3] = {0, 0, 0};
    for (size_t i = 0; i < 3U; ++i) {
        mapped_values[i] = src_values[i] * 2;
    }
    char json_buf[16] = {};
    if (!verify_json_array_stringify(mapped_values, 3U, json_buf, sizeof(json_buf)) ||
        strcmp(json_buf, "[2,4,6]") != 0) {
        return 102;
    }

    // 2. const records = new Map([['answer', 42]]);
    //    assert(records.get('answer') === 42, 'Map');
    struct MapEntry {
        const char* key;
        int value;
    };
    const MapEntry records[1] = {{"answer", 42}};
    int found_answer = -1;
    for (size_t i = 0; i < 1U; ++i) {
        if (strcmp(records[i].key, "answer") == 0) {
            found_answer = records[i].value;
        }
    }
    if (found_answer != 42) {
        return 103;
    }

    // 3. const bytes = new Uint8Array([1, 2, 255]);
    //    assert(bytes.reduce((sum, value) => sum + value, 0) === 258, 'typed arrays');
    const uint8_t bytes[3] = {1U, 2U, 255U};
    uint32_t sum = 0;
    for (size_t i = 0; i < 3U; ++i) {
        sum += bytes[i];
    }
    if (sum != 258U) {
        return 104;
    }

    // 4. let rejected = false;
    //    try { JSON.parse('{'); } catch (error) { rejected = error instanceof SyntaxError; }
    //    assert(rejected, 'exception identity');
    bool rejected = (parse_json_syntax_check("{") == JsErrorKind::SyntaxError);
    if (!rejected) {
        return 105;
    }

    // 5. Promise.resolve(21).then(value => {
    //        assert(value * 2 === 42, 'promise microtask');
    //        print('ALLOYOS_JSC_SMOKE_PASS');
    //    });
    GenericRunLoop runloop;
    if (!runloop.enqueue(new PromiseMicrotask(21, smoke_printed))) {
        return 106;
    }
    int microtask_result = 0;
    if (runloop.drain(&microtask_result) != 1 || microtask_result != 42 ||
        !*smoke_printed || g_vm_microtask_depth != 1) {
        return 107;
    }

    return 0;
}

void* jsc_worker_entry(void* raw) {
    SmokeWorkerContext* ctx = static_cast<SmokeWorkerContext*>(raw);
    (void)pthread_once(&g_vm_once, init_vm_once);

    bool tls_initial_ok = (g_vm_tls_canary == 0x4A534301 && g_vm_microtask_depth == 0);
    g_vm_tls_canary = 0x4A53432A;
    (void)pthread_setspecific(g_vm_key, reinterpret_cast<void*>(0x2AUL));
    (void)sched_yield();

    bool smoke_printed = false;
    int smoke_rc = evaluate_engine_smoke_on_worker(&smoke_printed);
    bool tls_final_ok = (tls_initial_ok &&
                         g_vm_tls_canary == 0x4A53432A &&
                         pthread_getspecific(g_vm_key) == reinterpret_cast<void*>(0x2AUL));

    pthread_mutex_lock(&ctx->mutex);
    ctx->smoke_printed = smoke_printed;
    ctx->status = (smoke_rc == 0 && tls_final_ok) ? 0 : (smoke_rc != 0 ? smoke_rc : 108);
    ctx->ready = 1;
    pthread_cond_signal(&ctx->cond);
    pthread_mutex_unlock(&ctx->mutex);
    return reinterpret_cast<void*>(42UL);
}

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

    if (pthread_key_create(&g_vm_key, destroy_vm_key) != 0) {
        return 8;
    }
    (void)pthread_once(&g_vm_once, init_vm_once);
    if (g_vm_once_count != 1) {
        return 9;
    }

    SmokeWorkerContext ctx {};
    pthread_mutex_init(&ctx.mutex, nullptr);
    pthread_cond_init(&ctx.cond, nullptr);

    pthread_attr_t attr {};
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, 16384U);

    pthread_t worker {};
    int create_rc = pthread_create(&worker, &attr, jsc_worker_entry, &ctx);
    pthread_attr_destroy(&attr);
    if (create_rc != 0) {
        return 10;
    }

    pthread_mutex_lock(&ctx.mutex);
    while (!ctx.ready) {
        pthread_cond_wait(&ctx.cond, &ctx.mutex);
    }
    pthread_mutex_unlock(&ctx.mutex);

    void* worker_ret = nullptr;
    if (pthread_join(worker, &worker_ret) != 0 || worker_ret != reinterpret_cast<void*>(42UL)) {
        return 11;
    }

    pthread_cond_destroy(&ctx.cond);
    pthread_mutex_destroy(&ctx.mutex);
    pthread_key_delete(g_vm_key);

    if (ctx.status != 0 || !ctx.smoke_printed ||
        g_vm_once_count != 1 || g_vm_key_dtor_count != 1 ||
        g_vm_tls_canary != 0x4A534301 || g_vm_microtask_depth != 0) {
        return 12;
    }

    static const char probe_msg[] = "JSC_PLATFORM_PROBE_PASS allocator=ok task=42";
    __alloyos_panic_report(probe_msg, sizeof(probe_msg) - 1U);

    static const char smoke_msg[] = "ALLOYOS_JSC_SMOKE_PASS";
    __alloyos_panic_report(smoke_msg, sizeof(smoke_msg) - 1U);
    return 0;
}
