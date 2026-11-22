#include "directional_dpi.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
static uint64_t s = 0x1234567ull;
static uint32_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return (uint32_t)s; }
int main(void) {
    /* window vs per-report step: counts of a 90-degree turn until the estimate passes 45 degrees */
    printf("turn-counts to cross 45deg after 90deg turn; rows L, cols step s per report\n      ");
    int steps[] = {1, 2, 4, 8, 16, 32, 64, 128};
    for (int j = 0; j < 8; ++j) printf("%6d", steps[j]);
    printf("   (ideal ~ln2*2L)\n");
    for (int L = 4; L <= 128; L *= 2) {
        printf("L=%3d ", L);
        for (int j = 0; j < 8; ++j) {
            int st = steps[j];
            ddpi_engine_t e; ddpi_engine_init(&e, 800);
            ddpi_profile_v1_t p = e.profile; p.flags = DDPI_FLAG_ENABLED; p.multipliers_q8_8[DDPI_DIR_S] = 512;
            ddpi_profile_set_window_counts(&p, (uint8_t)L); ddpi_engine_set_profile(&e, &p);
            int16_t ox, oy;
            for (int k = 0; k < 4000 / st + 10; ++k) ddpi_process_delta(&e, (int16_t)st, 0, &ox, &oy);
            int c = 0;
            while (e.dir_y_q16_16 <= e.dir_x_q16_16 && c < 100000) { ddpi_process_delta(&e, 0, (int16_t)st, &ox, &oy); c += st; }
            printf("%6d", c);
        }
        printf("   %.0f\n", 0.6931 * 2 * L);
    }
    /* mirror asymmetry magnitude with L=16 */
    ddpi_engine_t a, b; ddpi_engine_init(&a, 800);
    ddpi_profile_v1_t p = a.profile; p.flags = DDPI_FLAG_ENABLED;
    const uint16_t m[8] = {256, 300, 384, 340, 256, 300, 384, 340};
    memcpy(p.multipliers_q8_8, m, sizeof m);
    ddpi_profile_set_window_counts(&p, 16);
    ddpi_engine_set_profile(&a, &p); ddpi_engine_init(&b, 800); ddpi_engine_set_profile(&b, &p);
    long sa = 0, sb = 0, sin = 0; uint32_t maxd = 0; double sumrel = 0;
    for (int k = 0; k < 100000; ++k) {
        int16_t dx = (int16_t)((int)(rnd() % 61) - 20), dy = (int16_t)((int)(rnd() % 61) - 30);
        int16_t ax, ay, bx, by;
        ddpi_process_delta(&a, dx, dy, &ax, &ay); ddpi_process_delta(&b, (int16_t)-dx, (int16_t)-dy, &bx, &by);
        uint32_t d = a.last_scale_q16_16 > b.last_scale_q16_16 ? a.last_scale_q16_16 - b.last_scale_q16_16 : b.last_scale_q16_16 - a.last_scale_q16_16;
        if (d > maxd) maxd = d; sumrel += (double)d / a.last_scale_q16_16;
        sa += ax; sb += bx; sin += dx;
    }
    printf("mirror L=16: max |scale diff| %u Q16 LSB (%.4f%%), mean rel %.5f%%; sum out_x a=%ld, -b=%ld (raw %ld)\n",
           maxd, 100.0 * maxd / 65536, 100.0 * sumrel / 100000, sa, -sb, sin);
    return 0;
}
