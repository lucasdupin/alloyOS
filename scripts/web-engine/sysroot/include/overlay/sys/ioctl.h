#ifndef _SYS_IOCTL_H
#define _SYS_IOCTL_H 1

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef FIONREAD
#define FIONREAD 0x541B
#endif
#ifndef FIONBIO
#define FIONBIO  0x5421
#endif

int ioctl(int fd, unsigned long request, ...);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_IOCTL_H */
