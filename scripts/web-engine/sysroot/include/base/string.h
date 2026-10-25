#ifndef _ALLOYOS_STRING_H
#define _ALLOYOS_STRING_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void* memcpy(void* dst, const void* src, size_t n);
void* memmove(void* dst, const void* src, size_t n);
void* memset(void* s, int c, size_t n);
int memcmp(const void* s1, const void* s2, size_t n);
void* memchr(const void* s, int c, size_t n);

size_t strlen(const char* s);
size_t strnlen(const char* s, size_t maxlen);
int strcmp(const char* s1, const char* s2);
int strncmp(const char* s1, const char* s2, size_t n);
char* strcpy(char* dst, const char* src);
char* strncpy(char* dst, const char* src, size_t n);
char* strdup(const char* s);
char* strndup(const char* s, size_t n);
char* strchr(const char* s, int c);
char* strrchr(const char* s, int c);
char* strstr(const char* haystack, const char* needle);
char* strcat(char* dest, const char* src);
char* strncat(char* dest, const char* src, size_t n);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_STRING_H */
