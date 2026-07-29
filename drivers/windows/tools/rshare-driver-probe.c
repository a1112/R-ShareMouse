#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cfgmgr32.h>
#include <initguid.h>
#include <stdio.h>
#include <stdlib.h>
#include <wchar.h>

#include "..\rshare-common\rshare_ioctls.h"

// {8c1fd719-6fb8-4f82-a4d2-07c6fd490875}
DEFINE_GUID(
    GUID_DEVINTERFACE_RSHARE_VDISPLAY,
    0x8c1fd719,
    0x6fb8,
    0x4f82,
    0xa4,
    0xd2,
    0x07,
    0xc6,
    0xfd,
    0x49,
    0x08,
    0x75);

typedef struct RSHARE_PROBE_VDISPLAY_MODE {
    DWORD Width;
    DWORD Height;
    DWORD RefreshRateMillihz;
} RSHARE_PROBE_VDISPLAY_MODE;

static const RSHARE_PROBE_VDISPLAY_MODE RSHARE_PROBE_VDISPLAY_MODES[] = {
    {1920, 1080, 60000},
    {1920, 1080, 144000},
    {1920, 1080, 90000},
    {2560, 1440, 144000},
    {2560, 1440, 90000},
    {2560, 1440, 60000},
    {3840, 2160, 60000},
    {1600, 900, 60000},
    {1280, 720, 90000},
    {1280, 720, 60000},
    {1024, 768, 75000},
    {1024, 768, 60000},
};

static BOOL vdisplay_is_supported_mode(DWORD width, DWORD height, DWORD refreshRateMillihz)
{
    for (size_t index = 0; index < ARRAYSIZE(RSHARE_PROBE_VDISPLAY_MODES); index++) {
        RSHARE_PROBE_VDISPLAY_MODE mode = RSHARE_PROBE_VDISPLAY_MODES[index];
        if (mode.Width == width && mode.Height == height && mode.RefreshRateMillihz == refreshRateMillihz) {
            return TRUE;
        }
    }

    return FALSE;
}

static void print_usage(void)
{
    wprintf(L"usage:\n");
    wprintf(L"  rshare-driver-probe\n");
    wprintf(L"  rshare-driver-probe filter status\n");
    wprintf(L"  rshare-driver-probe filter stats\n");
    wprintf(L"  rshare-driver-probe filter test\n");
    wprintf(L"  rshare-driver-probe filter watch [timeout_seconds]\n");
    wprintf(L"  rshare-driver-probe filter drain [quiet_ms] [timeout_seconds]\n");
    wprintf(L"  rshare-driver-probe filter watch-keyboard [timeout_seconds]\n");
    wprintf(L"  rshare-driver-probe filter watch-mouse [timeout_seconds]\n");
    wprintf(L"  rshare-driver-probe vhid status\n");
    wprintf(L"  rshare-driver-probe vhid inject-smoke\n");
    wprintf(L"  rshare-driver-probe vdisplay status\n");
    wprintf(L"  rshare-driver-probe vdisplay create [width height refresh_millihz]\n");
    wprintf(L"  rshare-driver-probe vdisplay remove\n");
}

static HANDLE open_device(const wchar_t* path, const wchar_t* label)
{
    HANDLE device = CreateFileW(
        path,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        NULL);

    if (device == INVALID_HANDLE_VALUE) {
        wprintf(L"%ls open failed: %lu\n", label, GetLastError());
    }
    return device;
}

static HANDLE open_filter_event_stream(void)
{
    HANDLE device = CreateFileW(
        L"\\\\.\\RShareInputControl",
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
        NULL);
    if (device == INVALID_HANDLE_VALUE) {
        wprintf(L"filter event stream open failed: %lu\n", GetLastError());
    }
    return device;
}

static DWORD wait_filter_event(
    HANDLE device,
    DWORD timeout_ms,
    RSHARE_DRIVER_EVENT* event)
{
    OVERLAPPED overlapped = {0};
    DWORD returned = 0;
    DWORD error;
    DWORD wait_result;

    overlapped.hEvent = CreateEventW(NULL, TRUE, FALSE, NULL);
    if (overlapped.hEvent == NULL) {
        return GetLastError();
    }

    if (DeviceIoControl(
        device,
        IOCTL_RSHARE_WAIT_EVENT,
        NULL,
        0,
        event,
        sizeof(*event),
        NULL,
        &overlapped)) {
        if (!GetOverlappedResult(device, &overlapped, &returned, FALSE)) {
            error = GetLastError();
            CloseHandle(overlapped.hEvent);
            return error;
        }
        CloseHandle(overlapped.hEvent);
        return returned == sizeof(*event) ? ERROR_SUCCESS : ERROR_INVALID_DATA;
    }

    error = GetLastError();
    if (error != ERROR_IO_PENDING) {
        CloseHandle(overlapped.hEvent);
        return error;
    }

    wait_result = WaitForSingleObject(overlapped.hEvent, timeout_ms);
    if (wait_result == WAIT_TIMEOUT) {
        (VOID)CancelIoEx(device, &overlapped);
        (VOID)GetOverlappedResult(device, &overlapped, &returned, TRUE);
        CloseHandle(overlapped.hEvent);
        return WAIT_TIMEOUT;
    }
    if (wait_result != WAIT_OBJECT_0) {
        error = GetLastError();
        (VOID)CancelIoEx(device, &overlapped);
        (VOID)GetOverlappedResult(device, &overlapped, &returned, TRUE);
        CloseHandle(overlapped.hEvent);
        return error;
    }

    if (!GetOverlappedResult(device, &overlapped, &returned, FALSE)) {
        error = GetLastError();
        CloseHandle(overlapped.hEvent);
        return error;
    }
    CloseHandle(overlapped.hEvent);
    return returned == sizeof(*event) ? ERROR_SUCCESS : ERROR_INVALID_DATA;
}

static void print_driver_event(const char* prefix, const RSHARE_DRIVER_EVENT* event)
{
    printf(
        "%s device=%lu kind=%lu source=%hu flags=0x%08lx value0=%ld value1=%ld value2=%ld id=%016llx instance=%016llx\n",
        prefix,
        event->DeviceKind,
        event->EventKind,
        event->Source,
        event->Flags,
        event->Value0,
        event->Value1,
        event->Value2,
        (unsigned long long)event->DeviceId,
        (unsigned long long)event->DeviceInstanceHash);
}

static void print_filter_stats(const RSHARE_FILTER_STATS_V2* stats)
{
    printf(
        "stats abi=%hu version=%hu stats_bytes=%lu queue=%lu/%lu queued=%llu coalesced_realtime=%llu dropped_realtime=%llu reliable_overflow=%llu keyboard_connect=%llu mouse_connect=%llu keyboard_events=%llu mouse_events=%llu\n",
        stats->Abi,
        stats->Version,
        stats->StructSize,
        stats->QueueDepth,
        stats->QueueCapacity,
        (unsigned long long)stats->QueuedEventCount,
        (unsigned long long)stats->RealtimeCoalescedCount,
        (unsigned long long)stats->RealtimeDroppedCount,
        (unsigned long long)stats->ReliableOverflowCount,
        (unsigned long long)stats->KeyboardConnectCount,
        (unsigned long long)stats->MouseConnectCount,
        (unsigned long long)stats->KeyboardEventCount,
        (unsigned long long)stats->MouseEventCount);
}

static wchar_t* first_device_interface_path(GUID* interface_guid)
{
    ULONG length = 0;
    CONFIGRET status = CM_Get_Device_Interface_List_SizeW(
        &length,
        interface_guid,
        NULL,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT);
    if (status != CR_SUCCESS || length <= 1) {
        wprintf(L"vdisplay interface list size failed: 0x%08lx length=%lu\n", status, length);
        return NULL;
    }

    wchar_t* buffer = (wchar_t*)calloc(length, sizeof(wchar_t));
    if (buffer == NULL) {
        wprintf(L"vdisplay interface allocation failed\n");
        return NULL;
    }

    status = CM_Get_Device_Interface_ListW(
        interface_guid,
        NULL,
        buffer,
        length,
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT);
    if (status != CR_SUCCESS || buffer[0] == L'\0') {
        wprintf(L"vdisplay interface list failed: 0x%08lx\n", status);
        free(buffer);
        return NULL;
    }

    size_t first_len = wcslen(buffer);
    wchar_t* path = (wchar_t*)calloc(first_len + 1, sizeof(wchar_t));
    if (path == NULL) {
        free(buffer);
        return NULL;
    }
    wcscpy_s(path, first_len + 1, buffer);
    free(buffer);
    return path;
}

static int probe_filter(BOOL emit_test)
{
    HANDLE device = open_device(L"\\\\.\\RShareInputControl", L"filter");

    if (device == INVALID_HANDLE_VALUE) {
        return 2;
    }

    RSHARE_DRIVER_VERSION version = {0};
    DWORD returned = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_VERSION, NULL, 0, &version, sizeof(version), &returned, NULL)) {
        wprintf(L"query version failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 3;
    }

    printf("version %u.%u.%u abi %u\n", version.Major, version.Minor, version.Patch, version.Abi);

    RSHARE_DRIVER_CAPABILITIES capabilities = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_CAPABILITIES, NULL, 0, &capabilities, sizeof(capabilities), &returned, NULL)) {
        wprintf(L"filter query capabilities failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 4;
    }

    printf("filter capabilities flags=0x%08lx max_event=%lu\n", capabilities.Flags, capabilities.MaxEventSize);

    if (!emit_test) {
        CloseHandle(device);
        return 0;
    }

    RSHARE_TEST_PACKET packet = {0};
    packet.DeviceKind = RSHARE_DEVICE_KEYBOARD;
    packet.EventKind = RSHARE_EVENT_SYNTHETIC;
    if (!DeviceIoControl(device, IOCTL_RSHARE_EMIT_TEST_PACKET, &packet, sizeof(packet), NULL, 0, &returned, NULL)) {
        wprintf(L"emit test packet failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 5;
    }

    RSHARE_DRIVER_EVENT event = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_READ_EVENT, NULL, 0, &event, sizeof(event), &returned, NULL)) {
        wprintf(L"read event failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 6;
    }

    print_driver_event("event", &event);

    CloseHandle(device);
    return 0;
}

static int probe_filter_stats(void)
{
    HANDLE device = open_device(L"\\\\.\\RShareInputControl", L"filter");

    if (device == INVALID_HANDLE_VALUE) {
        return 2;
    }

    DWORD returned = 0;
    RSHARE_FILTER_STATS_V2 stats = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_FILTER_STATS_V2, NULL, 0, &stats, sizeof(stats), &returned, NULL)) {
        wprintf(L"filter query stats failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 22;
    }

    print_filter_stats(&stats);
    CloseHandle(device);
    return 0;
}

static BOOL filter_event_matches_target(const RSHARE_DRIVER_EVENT* event, ULONG target_device_kind)
{
    if (event->Source != RSHARE_SOURCE_HARDWARE) {
        return FALSE;
    }

    return target_device_kind == 0 || event->DeviceKind == target_device_kind;
}

static int probe_filter_watch(DWORD timeout_seconds, ULONG target_device_kind)
{
    HANDLE device = open_filter_event_stream();

    if (device == INVALID_HANDLE_VALUE) {
        return 14;
    }

    if (timeout_seconds == 0) {
        timeout_seconds = 15;
    }

    ULONGLONG deadline = GetTickCount64() + ((ULONGLONG)timeout_seconds * 1000ULL);
    while (GetTickCount64() < deadline) {
        RSHARE_DRIVER_EVENT event = {0};
        ULONGLONG now = GetTickCount64();
        if (now >= deadline) {
            break;
        }
        DWORD remaining = (DWORD)(deadline - now);
        DWORD error = wait_filter_event(device, remaining, &event);
        if (error == ERROR_SUCCESS) {
            BOOL matches_target = filter_event_matches_target(&event, target_device_kind);
            if (target_device_kind == 0 || matches_target) {
                print_driver_event("event", &event);
            }
            if (matches_target) {
                CloseHandle(device);
                return 0;
            }
            continue;
        }

        if (error == WAIT_TIMEOUT) {
            break;
        }

        wprintf(L"filter watch read failed: %lu\n", error);
        CloseHandle(device);
        return 15;
    }

    if (target_device_kind == RSHARE_DEVICE_KEYBOARD) {
        wprintf(L"filter watch timed out without a keyboard hardware event\n");
    } else if (target_device_kind == RSHARE_DEVICE_MOUSE) {
        wprintf(L"filter watch timed out without a mouse hardware event\n");
    } else {
        wprintf(L"filter watch timed out without a hardware event\n");
    }
    CloseHandle(device);
    return 16;
}

static int probe_filter_drain(DWORD quiet_ms, DWORD timeout_seconds)
{
    HANDLE device = open_filter_event_stream();

    if (device == INVALID_HANDLE_VALUE) {
        return 14;
    }

    if (quiet_ms == 0) {
        quiet_ms = 500;
    }
    if (timeout_seconds == 0) {
        timeout_seconds = 10;
    }

    DWORD drained = 0;
    ULONGLONG deadline = GetTickCount64() + ((ULONGLONG)timeout_seconds * 1000ULL);
    while (GetTickCount64() < deadline) {
        RSHARE_DRIVER_EVENT event = {0};
        ULONGLONG now = GetTickCount64();
        if (now >= deadline) {
            break;
        }
        DWORD wait_ms = quiet_ms;
        DWORD remaining = (DWORD)(deadline - now);
        if (wait_ms > remaining) {
            wait_ms = remaining;
        }
        DWORD error = wait_filter_event(device, wait_ms, &event);
        if (error == ERROR_SUCCESS) {
            print_driver_event("drained", &event);
            drained++;
            continue;
        }

        if (error == WAIT_TIMEOUT) {
            printf("filter drain idle drained=%lu quiet_ms=%lu\n", drained, quiet_ms);
            CloseHandle(device);
            return 0;
        }

        wprintf(L"filter drain read failed: %lu\n", error);
        CloseHandle(device);
        return 20;
    }

    wprintf(L"filter drain timed out before idle\n");
    CloseHandle(device);
    return 21;
}

static int probe_vhid(BOOL inject_smoke)
{
    HANDLE device = open_device(L"\\\\.\\RShareVirtualHidControl", L"vhid");

    if (device == INVALID_HANDLE_VALUE) {
        return 7;
    }

    RSHARE_DRIVER_VERSION version = {0};
    DWORD returned = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_VERSION, NULL, 0, &version, sizeof(version), &returned, NULL)) {
        wprintf(L"vhid query version failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 8;
    }

    printf("vhid version %u.%u.%u abi %u\n", version.Major, version.Minor, version.Patch, version.Abi);

    RSHARE_DRIVER_CAPABILITIES capabilities = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_CAPABILITIES, NULL, 0, &capabilities, sizeof(capabilities), &returned, NULL)) {
        wprintf(L"vhid query capabilities failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 9;
    }

    printf("vhid capabilities flags=0x%08lx max_event=%lu\n", capabilities.Flags, capabilities.MaxEventSize);

    if (!inject_smoke) {
        CloseHandle(device);
        return 0;
    }

    RSHARE_INJECT_REPORT report = {0};
    report.ReportKind = RSHARE_REPORT_KEYBOARD;
    report.Value0 = 0x10;
    report.Value1 = 1;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject shift down failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 10;
    }

    report.Value1 = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject shift up failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 11;
    }

    report.ReportKind = RSHARE_REPORT_MOUSE_MOVE;
    report.Value0 = 4;
    report.Value1 = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse move failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 12;
    }

    report.Value0 = -4;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse restore failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 13;
    }

    report.ReportKind = RSHARE_REPORT_MOUSE_BUTTON;
    report.Value0 = 1;
    report.Value1 = 1;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse button down failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 14;
    }

    report.Value1 = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse button up failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 15;
    }

    report.ReportKind = RSHARE_REPORT_MOUSE_WHEEL;
    report.Value0 = 0;
    report.Value1 = 1;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse wheel failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 16;
    }

    report.Value0 = 1;
    report.Value1 = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject horizontal mouse wheel failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 17;
    }

    printf("vhid inject smoke ok\n");
    CloseHandle(device);
    return 0;
}

static const char* vdisplay_activity_name(USHORT activity)
{
    switch (activity) {
    case RSHARE_VDISPLAY_ACTIVITY_REMOVED:
        return "Removed";
    case RSHARE_VDISPLAY_ACTIVITY_ACTIVE:
        return "Active";
    case RSHARE_VDISPLAY_ACTIVITY_PENDING:
        return "Pending";
    default:
        return "Unknown";
    }
}

static int print_vdisplay_state(HANDLE device)
{
    DWORD returned = 0;
    RSHARE_VDISPLAY_STATE state = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_VDISPLAY_QUERY_STATE, NULL, 0, &state, sizeof(state), &returned, NULL)) {
        wprintf(L"vdisplay query state failed: %lu\n", GetLastError());
        return 16;
    }

    printf("vdisplay state abi=%u active=%u %lux%lu@%lu connector=%lu activity=%s\n",
        state.Abi,
        state.Active,
        state.Width,
        state.Height,
        state.RefreshRateMillihz,
        state.ConnectorIndex,
        vdisplay_activity_name(state.Active));
    return 0;
}

static int probe_vdisplay(int argc, wchar_t** argv)
{
    if (argc < 3) {
        print_usage();
        return 13;
    }

    GUID interface_guid = GUID_DEVINTERFACE_RSHARE_VDISPLAY;
    wchar_t* path = first_device_interface_path(&interface_guid);
    if (path == NULL) {
        return 14;
    }

    HANDLE device = open_device(path, L"vdisplay");
    free(path);
    if (device == INVALID_HANDLE_VALUE) {
        return 14;
    }

    DWORD returned = 0;
    RSHARE_DRIVER_VERSION version = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_VERSION, NULL, 0, &version, sizeof(version), &returned, NULL)) {
        wprintf(L"vdisplay query version failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 15;
    }
    printf("vdisplay version %u.%u.%u abi %u\n", version.Major, version.Minor, version.Patch, version.Abi);

    RSHARE_DRIVER_CAPABILITIES capabilities = {0};
    if (DeviceIoControl(device, IOCTL_RSHARE_QUERY_CAPABILITIES, NULL, 0, &capabilities, sizeof(capabilities), &returned, NULL)) {
        printf("vdisplay capabilities flags=0x%08lx max_event=%lu\n", capabilities.Flags, capabilities.MaxEventSize);
    }

    if (wcscmp(argv[2], L"status") == 0) {
        int result = print_vdisplay_state(device);
        CloseHandle(device);
        return result;
    }

    if (wcscmp(argv[2], L"create") == 0) {
        RSHARE_VDISPLAY_REQUEST request = {0};
        request.Width = argc > 3 ? wcstoul(argv[3], NULL, 10) : 1920;
        request.Height = argc > 4 ? wcstoul(argv[4], NULL, 10) : 1080;
        request.RefreshRateMillihz = argc > 5 ? wcstoul(argv[5], NULL, 10) : 60000;
        if (!vdisplay_is_supported_mode(request.Width, request.Height, request.RefreshRateMillihz)) {
            wprintf(
                L"unsupported vdisplay mode %lux%lu@%lu; use a driver-reported mode\n",
                request.Width,
                request.Height,
                request.RefreshRateMillihz);
            CloseHandle(device);
            return 19;
        }

        if (!DeviceIoControl(device, IOCTL_RSHARE_VDISPLAY_CREATE, &request, sizeof(request), NULL, 0, &returned, NULL)) {
            wprintf(L"vdisplay create failed: %lu\n", GetLastError());
            CloseHandle(device);
            return 17;
        }

        printf("vdisplay create requested %lux%lu@%lu\n", request.Width, request.Height, request.RefreshRateMillihz);
        int result = print_vdisplay_state(device);
        CloseHandle(device);
        return result;
    }

    if (wcscmp(argv[2], L"remove") == 0) {
        if (!DeviceIoControl(device, IOCTL_RSHARE_VDISPLAY_REMOVE, NULL, 0, NULL, 0, &returned, NULL)) {
            wprintf(L"vdisplay remove failed: %lu\n", GetLastError());
            CloseHandle(device);
            return 18;
        }

        printf("vdisplay remove requested\n");
        int result = print_vdisplay_state(device);
        CloseHandle(device);
        return result;
    }

    print_usage();
    CloseHandle(device);
    return 19;
}

int wmain(int argc, wchar_t** argv)
{
    if (argc > 1) {
        if (wcscmp(argv[1], L"filter") == 0) {
            if (argc < 3) {
                print_usage();
                return 1;
            }
            if (wcscmp(argv[2], L"status") == 0) {
                return probe_filter(FALSE);
            }
            if (wcscmp(argv[2], L"stats") == 0) {
                return probe_filter_stats();
            }
            if (wcscmp(argv[2], L"test") == 0) {
                return probe_filter(TRUE);
            }
            if (wcscmp(argv[2], L"watch") == 0) {
                DWORD timeout_seconds = 15;
                if (argc >= 4) {
                    timeout_seconds = (DWORD)_wtoi(argv[3]);
                }
                return probe_filter_watch(timeout_seconds, 0);
            }
            if (wcscmp(argv[2], L"watch-keyboard") == 0) {
                DWORD timeout_seconds = 15;
                if (argc >= 4) {
                    timeout_seconds = (DWORD)_wtoi(argv[3]);
                }
                return probe_filter_watch(timeout_seconds, RSHARE_DEVICE_KEYBOARD);
            }
            if (wcscmp(argv[2], L"watch-mouse") == 0) {
                DWORD timeout_seconds = 15;
                if (argc >= 4) {
                    timeout_seconds = (DWORD)_wtoi(argv[3]);
                }
                return probe_filter_watch(timeout_seconds, RSHARE_DEVICE_MOUSE);
            }
            if (wcscmp(argv[2], L"drain") == 0) {
                DWORD quiet_ms = 500;
                DWORD timeout_seconds = 10;
                if (argc >= 4) {
                    quiet_ms = (DWORD)_wtoi(argv[3]);
                }
                if (argc >= 5) {
                    timeout_seconds = (DWORD)_wtoi(argv[4]);
                }
                return probe_filter_drain(quiet_ms, timeout_seconds);
            }
            print_usage();
            return 1;
        }
        if (wcscmp(argv[1], L"vhid") == 0) {
            if (argc < 3) {
                print_usage();
                return 1;
            }
            if (wcscmp(argv[2], L"status") == 0) {
                return probe_vhid(FALSE);
            }
            if (wcscmp(argv[2], L"inject-smoke") == 0) {
                return probe_vhid(TRUE);
            }
            print_usage();
            return 1;
        }
        if (wcscmp(argv[1], L"vdisplay") == 0) {
            return probe_vdisplay(argc, argv);
        }
        print_usage();
        return 1;
    }

    int filter_result = probe_filter(TRUE);
    int vhid_result = probe_vhid(TRUE);

    if (filter_result != 0) {
        return filter_result;
    }
    return vhid_result;
}
