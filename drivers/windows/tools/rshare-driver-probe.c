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

static int probe_filter(void)
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

    RSHARE_TEST_PACKET packet = {0};
    packet.DeviceKind = RSHARE_DEVICE_KEYBOARD;
    packet.EventKind = RSHARE_EVENT_SYNTHETIC;
    if (!DeviceIoControl(device, IOCTL_RSHARE_EMIT_TEST_PACKET, &packet, sizeof(packet), NULL, 0, &returned, NULL)) {
        wprintf(L"emit test packet failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 4;
    }

    RSHARE_DRIVER_EVENT event = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_READ_EVENT, NULL, 0, &event, sizeof(event), &returned, NULL)) {
        wprintf(L"read event failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 5;
    }

    printf("event device=%lu kind=%lu source=%u value0=%ld value1=%ld\n",
        event.DeviceKind,
        event.EventKind,
        event.Source,
        event.Value0,
        event.Value1);

    CloseHandle(device);
    return 0;
}

static int probe_vhid(void)
{
    HANDLE device = open_device(L"\\\\.\\RShareVirtualHidControl", L"vhid");

    if (device == INVALID_HANDLE_VALUE) {
        return 6;
    }

    RSHARE_DRIVER_VERSION version = {0};
    DWORD returned = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_VERSION, NULL, 0, &version, sizeof(version), &returned, NULL)) {
        wprintf(L"vhid query version failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 7;
    }

    printf("vhid version %u.%u.%u abi %u\n", version.Major, version.Minor, version.Patch, version.Abi);

    RSHARE_DRIVER_CAPABILITIES capabilities = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_QUERY_CAPABILITIES, NULL, 0, &capabilities, sizeof(capabilities), &returned, NULL)) {
        wprintf(L"vhid query capabilities failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 8;
    }

    printf("vhid capabilities flags=0x%08lx max_event=%lu\n", capabilities.Flags, capabilities.MaxEventSize);

    RSHARE_INJECT_REPORT report = {0};
    report.ReportKind = RSHARE_REPORT_KEYBOARD;
    report.Value0 = 0x10;
    report.Value1 = 1;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject shift down failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 9;
    }

    report.Value1 = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject shift up failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 10;
    }

    report.ReportKind = RSHARE_REPORT_MOUSE_MOVE;
    report.Value0 = 4;
    report.Value1 = 0;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse move failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 11;
    }

    report.Value0 = -4;
    if (!DeviceIoControl(device, IOCTL_RSHARE_INJECT_REPORT, &report, sizeof(report), NULL, 0, &returned, NULL)) {
        wprintf(L"vhid inject mouse restore failed: %lu\n", GetLastError());
        CloseHandle(device);
        return 12;
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
        if (wcscmp(argv[1], L"vdisplay") == 0) {
            return probe_vdisplay(argc, argv);
        }
        print_usage();
        return 1;
    }

    int filter_result = probe_filter();
    int vhid_result = probe_vhid();

    if (filter_result != 0) {
        return filter_result;
    }
    return vhid_result;
}
