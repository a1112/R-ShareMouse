#pragma once
#include <stdint.h>

/* Keep all held usages, even while the boot-compatible 6KRO report overflows. */
typedef struct RSHARE_KEY_STATE { uint8_t held[102]; } RSHARE_KEY_STATE;
static inline void RShareKeySet(RSHARE_KEY_STATE *state, uint8_t usage, int pressed)
{
    if (usage >= 4 && usage < sizeof(state->held)) state->held[usage] = pressed ? 1 : 0;
}
static inline void RShareKeyReport(const RSHARE_KEY_STATE *state, uint8_t output[6])
{
    unsigned int usage, count = 0;
    for (usage = 0; usage < 6; ++usage) output[usage] = 0;
    for (usage = 4; usage < sizeof(state->held); ++usage) {
        if (!state->held[usage]) continue;
        if (count == 6) { for (usage = 0; usage < 6; ++usage) output[usage] = 1; return; } /* HID ErrorRollOver */
        output[count++] = (uint8_t)usage;
    }
}
static inline void RSharePutI32(uint8_t *out, int32_t value)
{
    uint32_t bits = (uint32_t)value;
    out[0] = (uint8_t)bits; out[1] = (uint8_t)(bits >> 8);
    out[2] = (uint8_t)(bits >> 16); out[3] = (uint8_t)(bits >> 24);
}
static inline void RShareMouseReport(uint8_t out[18], uint8_t buttons, int32_t x, int32_t y, int32_t wheel, int32_t horizontal)
{
    out[0] = 2; out[1] = buttons & 31;
    RSharePutI32(out + 2, x); RSharePutI32(out + 6, y);
    RSharePutI32(out + 10, wheel); RSharePutI32(out + 14, horizontal);
}
