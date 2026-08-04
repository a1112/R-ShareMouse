#pragma once

#include <stddef.h>
#include <stdint.h>

#define RSHARE_MACOS_VDISPLAY_SERVICE_CLASS "RShareMacVirtualDisplay"
#define RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE 0x52534d56u

#define RSHARE_DRIVER_ABI 1u
#define RSHARE_CAP_VIRTUAL_DISPLAY 0x00000010u

#define RSHARE_VDISPLAY_ACTIVITY_REMOVED 0u
#define RSHARE_VDISPLAY_ACTIVITY_ACTIVE 1u
#define RSHARE_VDISPLAY_ACTIVITY_PENDING 2u

enum RShareMacosVdisplaySelector {
    RSHARE_MACOS_SELECTOR_QUERY_VERSION = 0,
    RSHARE_MACOS_SELECTOR_QUERY_CAPABILITIES = 1,
    RSHARE_MACOS_SELECTOR_VDISPLAY_QUERY_STATE = 2,
    RSHARE_MACOS_SELECTOR_VDISPLAY_CREATE = 3,
    RSHARE_MACOS_SELECTOR_VDISPLAY_REMOVE = 4,
};

typedef struct RShareDriverVersion {
    uint16_t major;
    uint16_t minor;
    uint16_t patch;
    uint16_t abi;
} RShareDriverVersion;

typedef struct RShareDriverCapabilities {
    uint16_t abi;
    uint16_t reserved0;
    uint32_t flags;
    uint32_t max_event_size;
    uint32_t reserved;
} RShareDriverCapabilities;

typedef struct RShareVdisplayRequest {
    uint32_t width;
    uint32_t height;
    uint32_t refresh_rate_millihz;
    uint32_t flags;
} RShareVdisplayRequest;

typedef struct RShareVdisplayState {
    uint16_t abi;
    uint16_t active;
    uint32_t width;
    uint32_t height;
    uint32_t refresh_rate_millihz;
    uint32_t connector_index;
} RShareVdisplayState;

#if defined(__cplusplus)
static_assert(sizeof(RShareDriverVersion) == 8, "RShareDriverVersion ABI size changed");
static_assert(offsetof(RShareDriverVersion, major) == 0, "RShareDriverVersion.major ABI offset changed");
static_assert(offsetof(RShareDriverVersion, abi) == 6, "RShareDriverVersion.abi ABI offset changed");
static_assert(sizeof(RShareDriverCapabilities) == 16, "RShareDriverCapabilities ABI size changed");
static_assert(offsetof(RShareDriverCapabilities, abi) == 0, "RShareDriverCapabilities.abi ABI offset changed");
static_assert(offsetof(RShareDriverCapabilities, reserved0) == 2, "RShareDriverCapabilities.reserved0 ABI offset changed");
static_assert(offsetof(RShareDriverCapabilities, flags) == 4, "RShareDriverCapabilities.flags ABI offset changed");
static_assert(offsetof(RShareDriverCapabilities, max_event_size) == 8, "RShareDriverCapabilities.max_event_size ABI offset changed");
static_assert(offsetof(RShareDriverCapabilities, reserved) == 12, "RShareDriverCapabilities.reserved ABI offset changed");
static_assert(sizeof(RShareVdisplayRequest) == 16, "RShareVdisplayRequest ABI size changed");
static_assert(offsetof(RShareVdisplayRequest, width) == 0, "RShareVdisplayRequest.width ABI offset changed");
static_assert(offsetof(RShareVdisplayRequest, flags) == 12, "RShareVdisplayRequest.flags ABI offset changed");
static_assert(sizeof(RShareVdisplayState) == 20, "RShareVdisplayState ABI size changed");
static_assert(offsetof(RShareVdisplayState, abi) == 0, "RShareVdisplayState.abi ABI offset changed");
static_assert(offsetof(RShareVdisplayState, active) == 2, "RShareVdisplayState.active ABI offset changed");
static_assert(offsetof(RShareVdisplayState, width) == 4, "RShareVdisplayState.width ABI offset changed");
static_assert(offsetof(RShareVdisplayState, connector_index) == 16, "RShareVdisplayState.connector_index ABI offset changed");
#else
#define RSHARE_STATIC_ASSERT_JOIN2(a, b) a##b
#define RSHARE_STATIC_ASSERT_JOIN(a, b) RSHARE_STATIC_ASSERT_JOIN2(a, b)
#define RSHARE_STATIC_ASSERT(e) \
    typedef char RSHARE_STATIC_ASSERT_JOIN(rshare_static_assert_, __LINE__)[(e) ? 1 : -1]

RSHARE_STATIC_ASSERT(sizeof(RShareDriverVersion) == 8);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverVersion, major) == 0);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverVersion, abi) == 6);
RSHARE_STATIC_ASSERT(sizeof(RShareDriverCapabilities) == 16);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverCapabilities, abi) == 0);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverCapabilities, reserved0) == 2);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverCapabilities, flags) == 4);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverCapabilities, max_event_size) == 8);
RSHARE_STATIC_ASSERT(offsetof(RShareDriverCapabilities, reserved) == 12);
RSHARE_STATIC_ASSERT(sizeof(RShareVdisplayRequest) == 16);
RSHARE_STATIC_ASSERT(offsetof(RShareVdisplayRequest, width) == 0);
RSHARE_STATIC_ASSERT(offsetof(RShareVdisplayRequest, flags) == 12);
RSHARE_STATIC_ASSERT(sizeof(RShareVdisplayState) == 20);
RSHARE_STATIC_ASSERT(offsetof(RShareVdisplayState, abi) == 0);
RSHARE_STATIC_ASSERT(offsetof(RShareVdisplayState, active) == 2);
RSHARE_STATIC_ASSERT(offsetof(RShareVdisplayState, width) == 4);
RSHARE_STATIC_ASSERT(offsetof(RShareVdisplayState, connector_index) == 16);
#endif
