/*++

Module Name:

    trace.h

Abstract:

    WPP trace definitions for the R-ShareMouse virtual display UMDF driver.

Environment:

    Windows User-Mode Driver Framework 2

--*/

#pragma once

#define WPP_CONTROL_GUIDS                                                   \
    WPP_DEFINE_CONTROL_GUID(                                                \
        RShareVDisplayTraceGuid, (23f7a6b0,6c4a,4d86,9e69,430e1b0f06aa),    \
        WPP_DEFINE_BIT(RSHARE_VDISPLAY_TRACE_DRIVER)                        \
        WPP_DEFINE_BIT(RSHARE_VDISPLAY_TRACE_DEVICE)                        \
        WPP_DEFINE_BIT(RSHARE_VDISPLAY_TRACE_SWAPCHAIN)                     \
    )

#define WPP_FLAG_LEVEL_LOGGER(flag, level) WPP_LEVEL_LOGGER(flag)
#define WPP_FLAG_LEVEL_ENABLED(flag, level) \
    (WPP_LEVEL_ENABLED(flag) && WPP_CONTROL(WPP_BIT_ ## flag).Level >= level)

// begin_wpp config
// FUNC TraceEvents(LEVEL, FLAGS, MSG, ...);
// end_wpp

#define RSHARE_VDISPLAY_TRACING_ID L"R-ShareMouse\\UMDF\\VirtualDisplay"
