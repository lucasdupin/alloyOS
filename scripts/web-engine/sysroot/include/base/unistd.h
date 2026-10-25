#ifndef _ALLOYOS_UNISTD_H
#define _ALLOYOS_UNISTD_H

#include <stddef.h>
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

#define _SC_PAGESIZE          30
#define _SC_PAGE_SIZE         _SC_PAGESIZE
#define _SC_NPROCESSORS_CONF  83
#define _SC_NPROCESSORS_ONLN  84

#ifndef SEEK_SET
#define SEEK_SET 0
#endif
#ifndef SEEK_CUR
#define SEEK_CUR 1
#endif
#ifndef SEEK_END
#define SEEK_END 2
#endif

__attribute__((noreturn)) void _exit(int status);
pid_t getpid(void);
int close(int fd);
ssize_t read(int fd, void* buf, size_t count);
ssize_t write(int fd, const void* buf, size_t count);
off_t lseek(int fd, off_t offset, int whence);
int isatty(int fd);
int usleep(useconds_t usec);
unsigned int sleep(unsigned int seconds);
long sysconf(int name);
void* sbrk(intptr_t increment);
int getentropy(void* buf, size_t buflen);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_UNISTD_H */
