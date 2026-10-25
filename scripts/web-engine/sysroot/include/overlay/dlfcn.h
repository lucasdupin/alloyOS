#ifndef _DLFCN_H
#define _DLFCN_H
#ifdef __cplusplus
extern "C" {
#endif
#define RTLD_LAZY   1
#define RTLD_NOW    2
#define RTLD_GLOBAL 4
#define RTLD_LOCAL  8
#define RTLD_NOLOAD 16
#define RTLD_DEFAULT ((void *) 0)
static inline void* dlopen(const char* filename, int flags) { (void)filename; (void)flags; return (void*)0; }
static inline void* dlsym(void* handle, const char* symbol) { (void)handle; (void)symbol; return (void*)0; }
static inline int dlclose(void* handle) { (void)handle; return 0; }
static inline char* dlerror(void) { return (char*)"dlopen not supported"; }
#ifdef __cplusplus
}
#endif
#endif
