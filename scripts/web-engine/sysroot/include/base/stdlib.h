#ifndef _ALLOYOS_STDLIB_H
#define _ALLOYOS_STDLIB_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1

void* malloc(size_t size);
void* calloc(size_t nmemb, size_t size);
void* realloc(void* ptr, size_t size);
void free(void* ptr);
int posix_memalign(void** memptr, size_t alignment, size_t size);
void* aligned_alloc(size_t alignment, size_t size);

__attribute__((noreturn)) void abort(void);
__attribute__((noreturn)) void exit(int status);
__attribute__((noreturn)) void _Exit(int status);
int atexit(void (*func)(void));

double strtod(const char* nptr, char** endptr);
long strtol(const char* nptr, char** endptr, int base);
unsigned long strtoul(const char* nptr, char** endptr, int base);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_STDLIB_H */
