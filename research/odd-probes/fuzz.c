/* UBSan fuzz of the stateful ODD paths + a few behavioural probes (audit scratch, not part of the repo). */
#include "directional_dpi.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint64_t s = 0x9E3779B97F4A7C15ull;
static uint32_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return (uint32_t)s; }
static int16_t rdelta(void) {
    switch (rnd() % 6) {
    case 0: return (int16_t)(rnd() & 0xFFFF);           /* full int16 incl. -32768 */
    case 1: return (int16_t)((int)(rnd() % 7) - 3);      /* tiny */
    case 2: return 0;
    case 3: return (rnd() & 1) ? 32767 : -32768;
    default: return (int16_t)((int)(rnd() % 401) - 200);
    }
}

int main(void) {
    long n = 0;
    for (int prof = 0; prof < 20000; ++prof) {
        ddpi_engine_t e;
        ddpi_engine_init(&e, (uint16_t)(rnd() & 0xFFFF));
        ddpi_profile_v1_t p = e.profile;
        p.flags = (uint8_t)(rnd() & 0xFF) | DDPI_FLAG_ENABLED;
        p.flags &= (uint8_t)~DDPI_FLAG_BYPASS_FOR_CALIBRATION;
        p.global_gain_q8_8 = (uint16_t)(16 + rnd() % 1009);
        for (int i = 0; i < 8; ++i) p.multipliers_q8_8[i] = (uint16_t)(16 + rnd() % 1009);
        p.reserved = (uint16_t)(rnd() & 0xFFFF);
        ddpi_engine_set_profile(&e, &p);
        if (rnd() & 1) ddpi_engine_set_sensor_dpi(&e, (uint16_t)(rnd() & 0xFFFF));
        uint32_t t = rnd();
        for (int k = 0; k < 200; ++k) {
            int16_t ox, oy;
            t += rnd() % 120; /* includes uint32 wrap */
            ddpi_process_motion(&e, rdelta(), rdelta(), t, &ox, &oy);
            if (ox == -32768 || oy == -32768) { printf("emitted -32768\n"); return 1; }
            ++n;
        }
        uint16_t cpi;
        ddpi_profile_normalize_cpi_headroom(&p, &cpi);
    }
    printf("fuzz: %ld process_motion calls, no UB trap\n", n);

    /* Probe 1: mirror symmetry of the estimator with a point-symmetric profile:
     * (dx,dy) sequence vs (-dx,-dy) sequence should give exactly negated outputs if rounding were symmetric. */
    {
        ddpi_engine_t a, b;
        ddpi_engine_init(&a, 800);
        ddpi_profile_v1_t p = a.profile;
        p.flags = DDPI_FLAG_ENABLED;
        const uint16_t m[8] = {256, 300, 384, 340, 256, 300, 384, 340}; /* point symmetric: M[k] == M[k+4] */
        memcpy(p.multipliers_q8_8, m, sizeof m);
        ddpi_profile_set_window_counts(&p, 16);
        ddpi_engine_set_profile(&a, &p);
        ddpi_engine_init(&b, 800);
        ddpi_engine_set_profile(&b, &p);
        int mism = 0, scale_mism = 0;
        for (int k = 0; k < 100000; ++k) {
            int16_t dx = (int16_t)((int)(rnd() % 61) - 30), dy = (int16_t)((int)(rnd() % 61) - 30);
            int16_t ax, ay, bx, by;
            ddpi_process_delta(&a, dx, dy, &ax, &ay);
            ddpi_process_delta(&b, (int16_t)-dx, (int16_t)-dy, &bx, &by);
            if (a.last_scale_q16_16 != b.last_scale_q16_16) ++scale_mism;
            if (ax != -bx || ay != -by) ++mism;
        }
        printf("probe mirror (L=16): scale mismatches %d / 100000, output mismatches %d / 100000\n", scale_mism, mism);
    }
    /* Probe 2: mirror with L=0 (no estimator) */
    {
        ddpi_profile_v1_t p; ddpi_profile_init_default(&p, 800);
        p.flags = DDPI_FLAG_ENABLED;
        const uint16_t m[8] = {256, 300, 384, 340, 256, 300, 384, 340};
        memcpy(p.multipliers_q8_8, m, sizeof m);
        ddpi_profile_sanitize_and_sign(&p);
        int mism = 0;
        for (int k = 0; k < 200000; ++k) {
            int16_t dx = (int16_t)((int)(rnd() % 65535) - 32767), dy = (int16_t)((int)(rnd() % 65535) - 32767);
            if (ddpi_compute_scale_q16_16(&p, dx, dy, NULL) != ddpi_compute_scale_q16_16(&p, (int16_t)-dx, (int16_t)-dy, NULL)) ++mism;
        }
        printf("probe mirror scale (L=0): mismatches %d / 200000\n", mism);
    }
    /* Probe 3: estimator effective decay length: after a long +X run, how many counts of +Y (1 count/report)
     * until the estimate passes 45 deg (octant changes from E-side to S-side)? */
    for (int L = 4; L <= 128; L *= 2) {
        ddpi_engine_t e; ddpi_engine_init(&e, 800);
        ddpi_profile_v1_t p = e.profile; p.flags = DDPI_FLAG_ENABLED; p.multipliers_q8_8[DDPI_DIR_S] = 512;
        ddpi_profile_set_window_counts(&p, (uint8_t)L);
        ddpi_engine_set_profile(&e, &p);
        int16_t ox, oy;
        for (int k = 0; k < 2000; ++k) ddpi_process_delta(&e, 1, 0, &ox, &oy);
        int c = 0;
        while (e.dir_y_q16_16 <= e.dir_x_q16_16 && c < 100000) { ddpi_process_delta(&e, 0, 1, &ox, &oy); ++c; }
        printf("probe L=%3d: %d counts of 90-deg turn until estimate passes 45 deg (ln2*2L=%.1f)\n", L, c, 0.6931 * 2 * L);
    }
    /* Probe 4: 8-bit QMK path: does a gain > 1 lose counts when the adapter clamps to int8 after the engine? */
    {
        ddpi_engine_t e; ddpi_engine_init(&e, 800);
        ddpi_profile_v1_t p = e.profile; p.flags = DDPI_FLAG_ENABLED; p.global_gain_q8_8 = 512;
        ddpi_engine_set_profile(&e, &p);
        long in = 0, out = 0; int16_t ox, oy;
        for (int k = 0; k < 100; ++k) { ddpi_process_delta(&e, 100, 0, &ox, &oy); in += 100; out += (ox > 127 ? 127 : ox); }
        printf("probe int8 clamp after engine: in %ld x2 = %ld, emitted %ld (lost %ld)\n", in, 2 * in, out, 2 * in - out);
    }
    return 0;
}
