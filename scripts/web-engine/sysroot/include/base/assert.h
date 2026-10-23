#ifndef _ALLOYOS_ASSERT_H
#define _ALLOYOS_ASSERT_H

#include <stdlib.h>

#undef assert
#ifdef NDEBUG
#define assert(expr) ((void)0)
#else
#define assert(expr) ((expr) ? (void)0 : abort())
#endif

#endif /* _ALLOYOS_ASSERT_H */
