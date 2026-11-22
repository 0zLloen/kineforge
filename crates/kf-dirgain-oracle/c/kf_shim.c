/*
 * KineForge oracle shim around the vendored engine (not part of upstream).
 *
 * kf_shim_layout(i) returns the i-th entry of LAYOUT, or (size_t)-1 past the end;
 * the Rust #[repr(C)] mirrors in src/c.rs list the same facts in the same order.
 * The other three functions are linkable copies of the header's static inline helpers.
 */

#include <stddef.h>

#include "directional_dpi.h"

static const size_t LAYOUT[] = {
    sizeof(ddpi_profile_v1_t),
    offsetof(ddpi_profile_v1_t, magic),
    offsetof(ddpi_profile_v1_t, version),
    offsetof(ddpi_profile_v1_t, flags),
    offsetof(ddpi_profile_v1_t, base_dpi),
    offsetof(ddpi_profile_v1_t, global_gain_q8_8),
    offsetof(ddpi_profile_v1_t, multipliers_q8_8),
    offsetof(ddpi_profile_v1_t, reserved),
    offsetof(ddpi_profile_v1_t, crc16),
    sizeof(ddpi_engine_t),
    offsetof(ddpi_engine_t, profile),
    offsetof(ddpi_engine_t, rem_x_q16_16),
    offsetof(ddpi_engine_t, rem_y_q16_16),
    offsetof(ddpi_engine_t, dir_x_q16_16),
    offsetof(ddpi_engine_t, dir_y_q16_16),
    offsetof(ddpi_engine_t, last_octant),
    offsetof(ddpi_engine_t, last_scale_q16_16),
    offsetof(ddpi_engine_t, last_motion_ms),
    offsetof(ddpi_engine_t, sensor_dpi),
    offsetof(ddpi_engine_t, clock_known),
    offsetof(ddpi_engine_t, clock_ms),
};

size_t kf_shim_layout(size_t i);
uint8_t kf_shim_get_window_counts(const ddpi_profile_v1_t *profile);
void kf_shim_set_window_counts(ddpi_profile_v1_t *profile, uint8_t window_counts);
uint32_t kf_shim_get_idle_ms(const ddpi_profile_v1_t *profile);

size_t kf_shim_layout(size_t i) {
    return i < sizeof(LAYOUT) / sizeof(LAYOUT[0]) ? LAYOUT[i] : (size_t)-1;
}

uint8_t kf_shim_get_window_counts(const ddpi_profile_v1_t *profile) {
    return ddpi_profile_get_window_counts(profile);
}

void kf_shim_set_window_counts(ddpi_profile_v1_t *profile, uint8_t window_counts) {
    ddpi_profile_set_window_counts(profile, window_counts);
}

uint32_t kf_shim_get_idle_ms(const ddpi_profile_v1_t *profile) {
    return ddpi_profile_get_idle_ms(profile);
}
