#define _POSIX_C_SOURCE 200809L
#include "directional_dpi.h"
#include <stdio.h>
#include <time.h>
int main(void){ ddpi_engine_t e; ddpi_engine_init(&e,800); ddpi_profile_v1_t p=e.profile; p.flags=DDPI_FLAG_ENABLED;
 const uint16_t m[8]={256,300,384,340,230,200,170,210}; for(int i=0;i<8;i++)p.multipliers_q8_8[i]=m[i]; ddpi_profile_set_window_counts(&p,16); ddpi_engine_set_profile(&e,&p);
 struct timespec a,b; clock_gettime(CLOCK_MONOTONIC,&a); long acc=0; uint32_t s=1;
 for(long k=0;k<20000000;k++){ s=s*1664525u+1013904223u; int16_t ox,oy; ddpi_process_motion(&e,(int16_t)((s>>8)%41-20),(int16_t)((s>>16)%41-20),(uint32_t)(k/8),&ox,&oy); acc+=ox+oy; }
 clock_gettime(CLOCK_MONOTONIC,&b); double ns=((b.tv_sec-a.tv_sec)*1e9+(b.tv_nsec-a.tv_nsec))/2e7; printf("x86-64 -O2: %.1f ns/report (acc %ld)\n",ns,acc); }
