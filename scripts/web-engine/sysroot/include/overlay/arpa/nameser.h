#ifndef _ARPA_NAMESER_H
#define _ARPA_NAMESER_H

#include <sys/types.h>
#include <stdint.h>

#define C_IN 1

#define T_A     1
#define T_NS    2
#define T_CNAME 5
#define T_SOA   6
#define T_PTR   12
#define T_MX    15
#define T_TXT   16
#define T_AAAA  28
#define T_SRV   33

#define NS_INT16SZ 2
#define NS_INT32SZ 4

#define GETSHORT(s, cp) do { \
    const unsigned char *t_cp = (const unsigned char *)(cp); \
    (s) = ((uint16_t)t_cp[0] << 8) | ((uint16_t)t_cp[1]); \
    (cp) += NS_INT16SZ; \
} while (0)

#define GETLONG(l, cp) do { \
    const unsigned char *t_cp = (const unsigned char *)(cp); \
    (l) = ((uint32_t)t_cp[0] << 24) | ((uint32_t)t_cp[1] << 16) | \
          ((uint32_t)t_cp[2] << 8) | ((uint32_t)t_cp[3]); \
    (cp) += NS_INT32SZ; \
} while (0)

typedef struct {
    unsigned id :16;
    unsigned rd :1;
    unsigned tc :1;
    unsigned aa :1;
    unsigned opcode :4;
    unsigned qr :1;
    unsigned rcode :4;
    unsigned cd :1;
    unsigned ad :1;
    unsigned unused :1;
    unsigned ra :1;
    unsigned qdcount :16;
    unsigned ancount :16;
    unsigned nscount :16;
    unsigned arcount :16;
} HEADER;

#ifdef __cplusplus
extern "C" {
#endif

int dn_expand(const unsigned char *msg, const unsigned char *eomorig, const unsigned char *comp_dn, char *exp_dn, int length);
int dn_comp(const char *src, unsigned char *dst, int dstsiz, unsigned char **dnptrs, unsigned char **lastdnptr);

#ifdef __cplusplus
}
#endif

#endif /* _ARPA_NAMESER_H */
