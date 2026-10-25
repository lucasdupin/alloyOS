#ifndef _RESOLV_H
#define _RESOLV_H

#include <sys/types.h>
#include <netinet/in.h>
#include <arpa/nameser.h>

#ifdef __cplusplus
extern "C" {
#endif

int res_query(const char* dname, int class, int type, unsigned char* answer, int anslen);

#ifdef __cplusplus
}
#endif

#endif /* _RESOLV_H */
