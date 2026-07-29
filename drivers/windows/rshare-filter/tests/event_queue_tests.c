#include <assert.h>
#include <stdio.h>
#include <string.h>

#define RSHARE_EVENT_QUEUE_PORTABLE_TEST 1
#ifndef RSHARE_EVENT_QUEUE_CAPACITY
#define RSHARE_EVENT_QUEUE_CAPACITY 4u
#endif
#include "../event_queue.h"

static RSHARE_DRIVER_EVENT event(ULONG kind, LONG value0, LONG value1)
{
    RSHARE_DRIVER_EVENT result;

    memset(&result, 0, sizeof(result));
    result.Abi = RSHARE_DRIVER_ABI;
    result.Source = RSHARE_SOURCE_HARDWARE;
    result.DeviceKind = kind == RSHARE_EVENT_KEY
        ? RSHARE_DEVICE_KEYBOARD
        : RSHARE_DEVICE_MOUSE;
    result.EventKind = kind;
    result.DeviceId = 7u;
    result.DeviceInstanceHash = 11u;
    result.Value0 = value0;
    result.Value1 = value1;
    result.TimestampUs = 100u + (ULONGLONG)value0;
    return result;
}

static RSHARE_DRIVER_EVENT absolute_event(LONG x, LONG y, ULONGLONG device_id)
{
    RSHARE_DRIVER_EVENT result = event(RSHARE_EVENT_MOUSE_MOVE, x, y);
    result.Flags = 0x00000001u;
    result.Value2 = 0x55;
    result.DeviceId = device_id;
    result.TimestampUs = 1000u + device_id;
    return result;
}

static void adjacent_relative_motion_accumulates(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_DRIVER_EVENT first = event(RSHARE_EVENT_MOUSE_MOVE, 4, -3);
    RSHARE_DRIVER_EVENT second = event(RSHARE_EVENT_MOUSE_MOVE, 6, 8);
    RSHARE_DRIVER_EVENT popped;

    assert(RShareEventQueuePush(&queue, &first) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &second) == RShareRealtimeCoalesced);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.EventKind == RSHARE_EVENT_MOUSE_MOVE);
    assert(popped.Value0 == 10);
    assert(popped.Value1 == 5);
    assert(popped.TimestampUs == second.TimestampUs);
    assert(!RShareEventQueuePop(&queue, &popped));
}

static void adjacent_absolute_motion_replaces_with_latest(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_DRIVER_EVENT first = absolute_event(10, 20, 7u);
    RSHARE_DRIVER_EVENT second = absolute_event(30, 40, 7u);
    RSHARE_DRIVER_EVENT popped;

    assert(RShareEventQueuePush(&queue, &first) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &second) == RShareRealtimeCoalesced);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.Value0 == second.Value0);
    assert(popped.Value1 == second.Value1);
    assert(popped.Value2 == second.Value2);
    assert(popped.TimestampUs == second.TimestampUs);
    assert(!RShareEventQueuePop(&queue, &popped));
}

static void coalescing_never_crosses_discrete_barriers(void)
{
    const ULONG barriers[] = {
        RSHARE_EVENT_KEY,
        RSHARE_EVENT_MOUSE_BUTTON,
        RSHARE_EVENT_MOUSE_WHEEL,
    };
    ULONG index;

    for (index = 0; index < sizeof(barriers) / sizeof(barriers[0]); ++index) {
        RSHARE_EVENT_QUEUE queue = {0};
        RSHARE_DRIVER_EVENT before = event(RSHARE_EVENT_MOUSE_MOVE, 2, 3);
        RSHARE_DRIVER_EVENT barrier = event(barriers[index], 40 + (LONG)index, 1);
        RSHARE_DRIVER_EVENT after = event(RSHARE_EVENT_MOUSE_MOVE, 5, 7);
        RSHARE_DRIVER_EVENT popped;

        assert(RShareEventQueuePush(&queue, &before) == RShareEventQueued);
        assert(RShareEventQueuePush(&queue, &barrier) == RShareEventQueued);
        assert(RShareEventQueuePush(&queue, &after) == RShareEventQueued);

        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == RSHARE_EVENT_MOUSE_MOVE);
        assert(popped.Value0 == 2);
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == barriers[index]);
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == RSHARE_EVENT_MOUSE_MOVE);
        assert(popped.Value0 == 5);
        assert(!RShareEventQueuePop(&queue, &popped));
    }

    for (index = 0; index < sizeof(barriers) / sizeof(barriers[0]); ++index) {
        RSHARE_EVENT_QUEUE queue = {0};
        RSHARE_DRIVER_EVENT before = absolute_event(10, 20, 7u);
        RSHARE_DRIVER_EVENT barrier = event(barriers[index], 50 + (LONG)index, 1);
        RSHARE_DRIVER_EVENT after = absolute_event(30, 40, 7u);
        RSHARE_DRIVER_EVENT popped;

        assert(RShareEventQueuePush(&queue, &before) == RShareEventQueued);
        assert(RShareEventQueuePush(&queue, &barrier) == RShareEventQueued);
        assert(RShareEventQueuePush(&queue, &after) == RShareEventQueued);
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.Value0 == before.Value0);
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == barriers[index]);
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.Value0 == after.Value0);
    }
}

static void absolute_motion_is_realtime_under_full_queue_pressure(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_EVENT_QUEUE_STATS stats;
    RSHARE_DRIVER_EVENT popped;
    ULONG index;

    for (index = 0; index < RSHARE_EVENT_QUEUE_CAPACITY; ++index) {
        RSHARE_DRIVER_EVENT absolute = absolute_event(
            10 + (LONG)index,
            20 + (LONG)index,
            100u + index);
        assert(RShareEventQueuePush(&queue, &absolute) == RShareEventQueued);
    }
    {
        RSHARE_DRIVER_EVENT newest = absolute_event(90, 91, 999u);
        assert(RShareEventQueuePush(&queue, &newest) == RShareRealtimeDropped);
    }
    RShareEventQueueGetStats(&queue, &stats);
    assert(stats.RealtimeDroppedCount == 1u);
    assert(stats.ReliableOverflowCount == 0u);

    {
        RSHARE_DRIVER_EVENT key = event(RSHARE_EVENT_KEY, 77, 1);
        assert(RShareEventQueuePush(&queue, &key) == RShareEventQueued);
    }
    for (index = 1; index < RSHARE_EVENT_QUEUE_CAPACITY; ++index) {
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == RSHARE_EVENT_MOUSE_MOVE);
        assert(popped.DeviceId == 100u + index);
    }
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.EventKind == RSHARE_EVENT_KEY);
    assert(popped.Value0 == 77);
}

static void wraparound_middle_realtime_eviction_preserves_discrete_fifo(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_DRIVER_EVENT popped;
    RSHARE_DRIVER_EVENT dummy = event(RSHARE_EVENT_KEY, 1, 1);
    RSHARE_DRIVER_EVENT key = event(RSHARE_EVENT_KEY, 10, 1);
    RSHARE_DRIVER_EVENT button = event(RSHARE_EVENT_MOUSE_BUTTON, 20, 1);
    RSHARE_DRIVER_EVENT motion = event(RSHARE_EVENT_MOUSE_MOVE, 30, 31);
    RSHARE_DRIVER_EVENT wheel = event(RSHARE_EVENT_MOUSE_WHEEL, 40, 41);
    RSHARE_DRIVER_EVENT newest = event(RSHARE_EVENT_KEY, 50, 1);
    const ULONG expected[] = {
        RSHARE_EVENT_KEY,
        RSHARE_EVENT_MOUSE_BUTTON,
        RSHARE_EVENT_MOUSE_WHEEL,
        RSHARE_EVENT_KEY,
    };
    ULONG index;

    assert(RShareEventQueuePush(&queue, &dummy) == RShareEventQueued);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(RShareEventQueuePush(&queue, &key) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &button) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &motion) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &wheel) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &newest) == RShareEventQueued);

    for (index = 0; index < sizeof(expected) / sizeof(expected[0]); ++index) {
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == expected[index]);
    }
}

static void coalescing_handles_ring_end_and_saturates_both_directions(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_DRIVER_EVENT popped;
    RSHARE_DRIVER_EVENT dummy = event(RSHARE_EVENT_KEY, 1, 1);
    RSHARE_DRIVER_EVENT positive = event(RSHARE_EVENT_MOUSE_MOVE, INT32_MAX - 4, INT32_MIN + 4);
    RSHARE_DRIVER_EVENT delta = event(RSHARE_EVENT_MOUSE_MOVE, 10, -10);
    ULONG index;

    for (index = 0; index < 3u; ++index) {
        assert(RShareEventQueuePush(&queue, &dummy) == RShareEventQueued);
        assert(RShareEventQueuePop(&queue, &popped));
    }
    assert(queue.Head == 3u);
    assert(RShareEventQueuePush(&queue, &positive) == RShareEventQueued);
    assert(queue.Head == 0u);
    assert(RShareEventQueuePush(&queue, &delta) == RShareRealtimeCoalesced);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.Value0 == INT32_MAX);
    assert(popped.Value1 == INT32_MIN);

    memset(&queue, 0, sizeof(queue));
    for (index = 0; index < 3u; ++index) {
        assert(RShareEventQueuePush(&queue, &dummy) == RShareEventQueued);
        assert(RShareEventQueuePop(&queue, &popped));
    }
    {
        RSHARE_DRIVER_EVENT first = absolute_event(10, 20, 7u);
        RSHARE_DRIVER_EVENT latest = absolute_event(30, 40, 7u);
        assert(RShareEventQueuePush(&queue, &first) == RShareEventQueued);
        assert(RShareEventQueuePush(&queue, &latest) == RShareRealtimeCoalesced);
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.Value0 == 30);
        assert(popped.Value1 == 40);
    }
}

static void discrete_events_are_never_silently_evicted(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_DRIVER_EVENT key1 = event(RSHARE_EVENT_KEY, 1, 1);
    RSHARE_DRIVER_EVENT motion = event(RSHARE_EVENT_MOUSE_MOVE, 2, 2);
    RSHARE_DRIVER_EVENT button = event(RSHARE_EVENT_MOUSE_BUTTON, 3, 1);
    RSHARE_DRIVER_EVENT wheel = event(RSHARE_EVENT_MOUSE_WHEEL, 4, 0);
    RSHARE_DRIVER_EVENT key2 = event(RSHARE_EVENT_KEY, 5, 1);
    RSHARE_DRIVER_EVENT popped;
    const ULONG expected_kinds[] = {
        RSHARE_EVENT_KEY,
        RSHARE_EVENT_MOUSE_BUTTON,
        RSHARE_EVENT_MOUSE_WHEEL,
        RSHARE_EVENT_KEY,
    };
    ULONG index;

    assert(RShareEventQueuePush(&queue, &key1) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &motion) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &button) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &wheel) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &key2) == RShareEventQueued);

    for (index = 0; index < sizeof(expected_kinds) / sizeof(expected_kinds[0]); ++index) {
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == expected_kinds[index]);
    }
    assert(!RShareEventQueuePop(&queue, &popped));
}

static void all_discrete_full_queue_latches_overflow_outside_ring(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_DRIVER_EVENT popped;
    ULONG index;

    for (index = 0; index < RSHARE_EVENT_QUEUE_CAPACITY; ++index) {
        RSHARE_DRIVER_EVENT key = event(RSHARE_EVENT_KEY, 10 + (LONG)index, 1);
        assert(RShareEventQueuePush(&queue, &key) == RShareEventQueued);
    }

    {
        RSHARE_DRIVER_EVENT overflowed = event(RSHARE_EVENT_MOUSE_BUTTON, 9, 1);
        assert(
            RShareEventQueuePush(&queue, &overflowed)
            == RShareReliableOverflowLatched);
    }

    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.EventKind == RSHARE_EVENT_RELIABLE_OVERFLOW);
    for (index = 0; index < RSHARE_EVENT_QUEUE_CAPACITY; ++index) {
        assert(RShareEventQueuePop(&queue, &popped));
        assert(popped.EventKind == RSHARE_EVENT_KEY);
        assert(popped.Value0 == 10 + (LONG)index);
    }
    assert(!RShareEventQueuePop(&queue, &popped));
}

static void repeated_reliable_overflow_uses_one_marker_and_truthful_count(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_EVENT_QUEUE_STATS stats;
    RSHARE_DRIVER_EVENT popped;
    ULONG index;

    for (index = 0; index < RSHARE_EVENT_QUEUE_CAPACITY; ++index) {
        RSHARE_DRIVER_EVENT key = event(RSHARE_EVENT_KEY, 10 + (LONG)index, 1);
        assert(RShareEventQueuePush(&queue, &key) == RShareEventQueued);
    }
    for (index = 0; index < 3u; ++index) {
        RSHARE_DRIVER_EVENT button = event(
            RSHARE_EVENT_MOUSE_BUTTON,
            30 + (LONG)index,
            1);
        assert(
            RShareEventQueuePush(&queue, &button)
            == RShareReliableOverflowLatched);
    }
    RShareEventQueueGetStats(&queue, &stats);
    assert(stats.ReliableOverflowCount == 3u);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.EventKind == RSHARE_EVENT_RELIABLE_OVERFLOW);

    {
        RSHARE_DRIVER_EVENT button = event(RSHARE_EVENT_MOUSE_BUTTON, 40, 1);
        assert(
            RShareEventQueuePush(&queue, &button)
            == RShareReliableOverflowLatched);
    }
    RShareEventQueueGetStats(&queue, &stats);
    assert(stats.ReliableOverflowCount == 4u);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.EventKind == RSHARE_EVENT_RELIABLE_OVERFLOW);
    assert(RShareEventQueuePop(&queue, &popped));
    assert(popped.EventKind == RSHARE_EVENT_KEY);
}

static void stats_distinguish_realtime_and_reliable_outcomes(void)
{
    RSHARE_EVENT_QUEUE queue = {0};
    RSHARE_EVENT_QUEUE_STATS stats;
    RSHARE_DRIVER_EVENT motion1 = event(RSHARE_EVENT_MOUSE_MOVE, 1, 1);
    RSHARE_DRIVER_EVENT motion2 = event(RSHARE_EVENT_MOUSE_MOVE, 2, 2);
    ULONG index;

    assert(RShareEventQueuePush(&queue, &motion1) == RShareEventQueued);
    assert(RShareEventQueuePush(&queue, &motion2) == RShareRealtimeCoalesced);
    memset(&queue, 0, sizeof(queue));

    for (index = 0; index < RSHARE_EVENT_QUEUE_CAPACITY; ++index) {
        RSHARE_DRIVER_EVENT key = event(RSHARE_EVENT_KEY, 20 + (LONG)index, 1);
        assert(RShareEventQueuePush(&queue, &key) == RShareEventQueued);
    }
    assert(RShareEventQueuePush(&queue, &motion1) == RShareRealtimeDropped);
    {
        RSHARE_DRIVER_EVENT button = event(RSHARE_EVENT_MOUSE_BUTTON, 1, 1);
        assert(
            RShareEventQueuePush(&queue, &button)
            == RShareReliableOverflowLatched);
    }

    RShareEventQueueGetStats(&queue, &stats);
    assert(stats.RealtimeCoalescedCount == 0u);
    assert(stats.RealtimeDroppedCount == 1u);
    assert(stats.ReliableOverflowCount == 1u);

    {
        RSHARE_EVENT_QUEUE coalesced_queue = {0};
        assert(RShareEventQueuePush(&coalesced_queue, &motion1) == RShareEventQueued);
        assert(
            RShareEventQueuePush(&coalesced_queue, &motion2)
            == RShareRealtimeCoalesced);
        RShareEventQueueGetStats(&coalesced_queue, &stats);
        assert(stats.RealtimeCoalescedCount == 1u);
        assert(stats.RealtimeDroppedCount == 0u);
        assert(stats.ReliableOverflowCount == 0u);
    }
}

int main(void)
{
    adjacent_relative_motion_accumulates();
    adjacent_absolute_motion_replaces_with_latest();
    coalescing_never_crosses_discrete_barriers();
    absolute_motion_is_realtime_under_full_queue_pressure();
    wraparound_middle_realtime_eviction_preserves_discrete_fifo();
    coalescing_handles_ring_end_and_saturates_both_directions();
    discrete_events_are_never_silently_evicted();
    all_discrete_full_queue_latches_overflow_outside_ring();
    repeated_reliable_overflow_uses_one_marker_and_truthful_count();
    stats_distinguish_realtime_and_reliable_outcomes();
    puts("rshare-filter semantic queue tests passed");
    return 0;
}
