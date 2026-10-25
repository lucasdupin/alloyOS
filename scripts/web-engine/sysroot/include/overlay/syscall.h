#ifndef _SYS_SYSCALL_H
#define _SYS_SYSCALL_H

#include <sys/types.h>
#include <fcntl.h>

#define SYS_gettid 178
#define SYS_getrandom 278

#ifdef __cplusplus
extern "C" {
#endif

long syscall(long number, ...);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_SYSCALL_H */
