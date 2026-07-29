#include "event_queue.h"

#define RSHARE_MOUSE_MOVE_ABSOLUTE_FLAG 0x00000001u
#define RSHARE_LONG_MAX_VALUE 2147483647L
#define RSHARE_LONG_MIN_VALUE (-2147483647L - 1L)

static BOOLEAN RShareIsRealtimeMotion(const RSHARE_DRIVER_EVENT* event)
{
    return event->EventKind == RSHARE_EVENT_MOUSE_MOVE;
}

static BOOLEAN RShareIsAbsoluteMotion(const RSHARE_DRIVER_EVENT* event)
{
    return (event->Flags & RSHARE_MOUSE_MOVE_ABSOLUTE_FLAG) != 0u;
}

static BOOLEAN RShareCanCoalesce(
    const RSHARE_DRIVER_EVENT* previous,
    const RSHARE_DRIVER_EVENT* event)
{
    return RShareIsRealtimeMotion(previous)
        && RShareIsRealtimeMotion(event)
        && previous->Source == event->Source
        && previous->DeviceKind == event->DeviceKind
        && previous->DeviceId == event->DeviceId
        && previous->DeviceInstanceHash == event->DeviceInstanceHash
        && previous->Flags == event->Flags
        && previous->Value2 == event->Value2;
}

static LONG RShareSaturatingAddLong(LONG left, LONG right)
{
    long long total = (long long)left + (long long)right;

    if (total > (long long)RSHARE_LONG_MAX_VALUE) {
        return (LONG)RSHARE_LONG_MAX_VALUE;
    }
    if (total < (long long)RSHARE_LONG_MIN_VALUE) {
        return (LONG)RSHARE_LONG_MIN_VALUE;
    }
    return (LONG)total;
}

static ULONG RSharePhysicalIndex(const RSHARE_EVENT_QUEUE* queue, ULONG logicalIndex)
{
    return (queue->Tail + logicalIndex) % RSHARE_EVENT_QUEUE_CAPACITY;
}

static BOOLEAN RShareDropOldestRealtime(RSHARE_EVENT_QUEUE* queue)
{
    ULONG logicalIndex;

    for (logicalIndex = 0u; logicalIndex < queue->Count; ++logicalIndex) {
        ULONG physicalIndex = RSharePhysicalIndex(queue, logicalIndex);
        ULONG shiftIndex;

        if (!RShareIsRealtimeMotion(&queue->Events[physicalIndex])) {
            continue;
        }

        for (shiftIndex = logicalIndex; shiftIndex + 1u < queue->Count; ++shiftIndex) {
            ULONG current = RSharePhysicalIndex(queue, shiftIndex);
            ULONG next = RSharePhysicalIndex(queue, shiftIndex + 1u);
            queue->Events[current] = queue->Events[next];
        }
        queue->Count--;
        queue->Head = RSharePhysicalIndex(queue, queue->Count);
        queue->Stats.RealtimeDroppedCount++;
        return TRUE;
    }

    return FALSE;
}

static void RShareLatchReliableOverflow(
    RSHARE_EVENT_QUEUE* queue,
    const RSHARE_DRIVER_EVENT* event)
{
    if (!queue->ReliableOverflowLatched) {
        queue->ReliableOverflowEvent = *event;
        queue->ReliableOverflowEvent.EventKind = RSHARE_EVENT_RELIABLE_OVERFLOW;
        queue->ReliableOverflowEvent.Value0 = 0;
        queue->ReliableOverflowEvent.Value1 = 0;
        queue->ReliableOverflowEvent.Value2 = 0;
        queue->ReliableOverflowLatched = TRUE;
    }
    queue->Stats.ReliableOverflowCount++;
}

RSHARE_EVENT_QUEUE_PUSH_RESULT
RShareEventQueuePush(
    RSHARE_EVENT_QUEUE* queue,
    const RSHARE_DRIVER_EVENT* event)
{
    if (queue->Count > 0u && RShareIsRealtimeMotion(event)) {
        ULONG previousIndex =
            (queue->Head + RSHARE_EVENT_QUEUE_CAPACITY - 1u)
            % RSHARE_EVENT_QUEUE_CAPACITY;
        RSHARE_DRIVER_EVENT* previous = &queue->Events[previousIndex];

        if (RShareCanCoalesce(previous, event)) {
            if (RShareIsAbsoluteMotion(event)) {
                *previous = *event;
            } else {
                previous->Value0 =
                    RShareSaturatingAddLong(previous->Value0, event->Value0);
                previous->Value1 =
                    RShareSaturatingAddLong(previous->Value1, event->Value1);
                previous->TimestampUs = event->TimestampUs;
            }
            queue->Stats.RealtimeCoalescedCount++;
            return RShareRealtimeCoalesced;
        }
    }

    if (queue->Count == RSHARE_EVENT_QUEUE_CAPACITY) {
        if (RShareIsRealtimeMotion(event)) {
            queue->Stats.RealtimeDroppedCount++;
            return RShareRealtimeDropped;
        }
        if (!RShareDropOldestRealtime(queue)) {
            RShareLatchReliableOverflow(queue, event);
            return RShareReliableOverflowLatched;
        }
    }

    queue->Events[queue->Head] = *event;
    queue->Head = (queue->Head + 1u) % RSHARE_EVENT_QUEUE_CAPACITY;
    queue->Count++;
    queue->Stats.QueuedEventCount++;
    return RShareEventQueued;
}

BOOLEAN
RShareEventQueuePop(
    RSHARE_EVENT_QUEUE* queue,
    RSHARE_DRIVER_EVENT* event)
{
    if (queue->ReliableOverflowLatched) {
        *event = queue->ReliableOverflowEvent;
        queue->ReliableOverflowLatched = FALSE;
        return TRUE;
    }

    if (queue->Count == 0u) {
        return FALSE;
    }

    *event = queue->Events[queue->Tail];
    queue->Tail = (queue->Tail + 1u) % RSHARE_EVENT_QUEUE_CAPACITY;
    queue->Count--;
    return TRUE;
}

void
RShareEventQueueGetStats(
    const RSHARE_EVENT_QUEUE* queue,
    RSHARE_EVENT_QUEUE_STATS* stats)
{
    *stats = queue->Stats;
}
