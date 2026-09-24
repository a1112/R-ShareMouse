#include "../report_state.h"
#ifdef RSHARE_FREESTANDING_TEST
#define assert(expression) do { if (!(expression)) return __LINE__; } while (0)
#define puts(message) ((void)0)
#else
#include <assert.h>
#include <stdio.h>
#endif

static int32_t read_i32(const uint8_t *p)
{
    uint32_t value = (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
    return (int32_t)value;
}
int main(void)
{
    RSHARE_KEY_STATE keys = {{0}};
    uint8_t keyboard[6], mouse[18];
    int usage;
    for (usage = 4; usage <= 10; ++usage) RShareKeySet(&keys, (uint8_t)usage, 1);
    RShareKeyReport(&keys, keyboard);
    for (usage = 0; usage < 6; ++usage) assert(keyboard[usage] == 1);
    RShareKeySet(&keys, 7, 0);
    RShareKeyReport(&keys, keyboard);
    assert(keyboard[0] == 4 && keyboard[1] == 5 && keyboard[2] == 6);
    assert(keyboard[3] == 8 && keyboard[4] == 9 && keyboard[5] == 10);
    for (usage = 4; usage <= 10; ++usage) RShareKeySet(&keys, (uint8_t)usage, 0);
    RShareKeyReport(&keys, keyboard);
    for (usage = 0; usage < 6; ++usage) assert(keyboard[usage] == 0);
    RShareMouseReport(mouse, 31, 300, -300, INT32_MAX, INT32_MIN);
    assert(mouse[0] == 2 && mouse[1] == 31);
    assert(read_i32(mouse + 2) == 300 && read_i32(mouse + 6) == -300);
    assert(read_i32(mouse + 10) == INT32_MAX && read_i32(mouse + 14) == INT32_MIN);
    puts("VHF report state: rollover recovery and full-width motion passed");
    return 0;
}
