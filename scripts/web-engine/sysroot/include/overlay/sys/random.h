#ifndef _SYS_RANDOM_H
#define _SYS_RANDOM_H

#include <stddef.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define GRND_NONBLOCK 0x0001U
#define GRND_RANDOM   0x0002U
#define GRND_INSECURE 0x0004U

ssize_t getrandom(void* buf, size_t buflen, unsigned int flags);
int getentropy(void* buf, size_t buflen);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_RANDOM_H */
