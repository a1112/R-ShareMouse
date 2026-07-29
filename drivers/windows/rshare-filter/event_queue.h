#pragma once

#if defined(RSHARE_EVENT_QUEUE_PORTABLE_TEST)
#include <stdint.h>

typedef uint8_t BOOLEAN;
typedef uint16_t USHORT;
typedef uint32_t ULONG;
typedef int32_t LONG;
typedef uint64_t ULONGLONG;

#ifndef TRUE
#define TRUE ((BOOLEAN)1u)
#endif
#ifndef FALSE
#define FALSE ((BOOLEAN)0u)
#endif

#define RSHARE_DRIVER_ABI 1
#define RSHARE_SOURCE_HARDWARE 1u
#define RSHARE_DEVICE_KEYBOARD 1u
#define RSHARE_DEVICE_MOUSE 2u
#define RSHARE_EVENT_KEY 1u
#define RSHARE_EVENT_MOUSE_MOVE 2u
#define RSHARE_EVENT_MOUSE_BUTTON 3u
#define RSHARE_EVENT_MOUSE_WHEEL 4u
#define RSHARE_EVENT_SYNTHETIC 5u
#define RSHARE_EVENT_RELIABLE_OVERFLOW 6u

typedef struct _RSHARE_DRIVER_EVENT {
    USHORT Abi;
    USHORT Source;
    ULONG DeviceKind;
    ULONG EventKind;
    ULONG Flags;
    ULONGLONG DeviceId;
    ULONGLONG DeviceInstanceHash;
    LONG Value0;
    LONG Value1;
    LONG Value2;
    ULONGLONG TimestampUs;
} RSHARE_DRIVER_EVENT, *PRSHARE_DRIVER_EVENT;
#else
#include "..\rshare-common\rshare_ioctls.h"
#endif

#ifndef RSHARE_EVENT_QUEUE_CAPACITY
#define RSHARE_EVENT_QUEUE_CAPACITY 128u
#endif

typedef enum _RSHARE_EVENT_QUEUE_PUSH_RESULT {
    RShareEventQueued,
    RShareRealtimeCoalesced,
    RShareRealtimeDropped,
    RShareReliableOverflowLatched
} RSHARE_EVENT_QUEUE_PUSH_RESULT;

typedef struct _RSHARE_EVENT_QUEUE_STATS {
    ULONGLONG QueuedEventCount;
    ULONGLONG RealtimeCoalescedCount;
    ULONGLONG RealtimeDroppedCount;
    ULONGLONG ReliableOverflowCount;
} RSHARE_EVENT_QUEUE_STATS, *PRSHARE_EVENT_QUEUE_STATS;

typedef struct _RSHARE_EVENT_QUEUE {
    RSHARE_DRIVER_EVENT Events[RSHARE_EVENT_QUEUE_CAPACITY];
    ULONG Head;
    ULONG Tail;
    ULONG Count;
    BOOLEAN ReliableOverflowLatched;
    RSHARE_DRIVER_EVENT ReliableOverflowEvent;
    RSHARE_EVENT_QUEUE_STATS Stats;
} RSHARE_EVENT_QUEUE, *PRSHARE_EVENT_QUEUE;

RSHARE_EVENT_QUEUE_PUSH_RESULT
RShareEventQueuePush(
    RSHARE_EVENT_QUEUE* queue,
    const RSHARE_DRIVER_EVENT* event);

BOOLEAN
RShareEventQueuePop(
    RSHARE_EVENT_QUEUE* queue,
    RSHARE_DRIVER_EVENT* event);

void
RShareEventQueueGetStats(
    const RSHARE_EVENT_QUEUE* queue,
    RSHARE_EVENT_QUEUE_STATS* stats);
