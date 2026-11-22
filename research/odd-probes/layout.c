#include "directional_dpi.h"
#include <stddef.h>
#define P(T,f) _Static_assert(1,""); 
const unsigned layout[] = {
  sizeof(ddpi_profile_v1_t), offsetof(ddpi_profile_v1_t, crc16),
  sizeof(ddpi_engine_t), offsetof(ddpi_engine_t, rem_x_q16_16), offsetof(ddpi_engine_t, last_octant),
  offsetof(ddpi_engine_t, last_scale_q16_16), offsetof(ddpi_engine_t, sensor_dpi), offsetof(ddpi_engine_t, clock_known),
  offsetof(ddpi_engine_t, clock_ms), _Alignof(ddpi_engine_t) };
