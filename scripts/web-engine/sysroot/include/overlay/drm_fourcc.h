#ifndef _DRM_FOURCC_H_
#define _DRM_FOURCC_H_

#include <stdint.h>

#define fourcc_code(a, b, c, d) ((uint32_t)(a) | ((uint32_t)(b) << 8) | \
                                 ((uint32_t)(c) << 16) | ((uint32_t)(d) << 24))

#define DRM_FORMAT_XRGB8888     fourcc_code('X', 'R', '2', '4')
#define DRM_FORMAT_ARGB8888     fourcc_code('A', 'R', '2', '4')
#define DRM_FORMAT_RGBA8888     fourcc_code('R', 'A', '2', '4')
#define DRM_FORMAT_RGBX8888     fourcc_code('R', 'X', '2', '4')
#define DRM_FORMAT_BGRA8888     fourcc_code('B', 'A', '2', '4')
#define DRM_FORMAT_BGRX8888     fourcc_code('B', 'X', '2', '4')

#endif /* _DRM_FOURCC_H_ */
