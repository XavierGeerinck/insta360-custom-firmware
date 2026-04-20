/* Synthesized replacement for speexdsp_config_types.h.in. speexdsp's autotools
 * build would generate this file from configure checks. We target hosts that
 * provide <stdint.h>, so the typedefs resolve to stdint's exact-width types. */
#ifndef __SPEEX_TYPES_H__
#define __SPEEX_TYPES_H__

#include <stdint.h>

typedef int16_t spx_int16_t;
typedef uint16_t spx_uint16_t;
typedef int32_t spx_int32_t;
typedef uint32_t spx_uint32_t;

#endif
