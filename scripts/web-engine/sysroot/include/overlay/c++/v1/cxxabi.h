#ifndef _ALLOYOS_CXXABI_H
#define _ALLOYOS_CXXABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void __cxa_pure_virtual(void);
void __cxa_deleted_virtual(void);

int __cxa_guard_acquire(uint64_t* guard_object);
void __cxa_guard_release(uint64_t* guard_object);
void __cxa_guard_abort(uint64_t* guard_object);

int __cxa_atexit(void (*destructor)(void*), void* arg, void* dso_handle);
void __cxa_finalize(void* dso_handle);

extern void* __dso_handle;

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_CXXABI_H */
