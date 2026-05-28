#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <wchar.h>

#include "..\rshare-common\rshare_ioctls.h"

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

static int print_vdisplay_state(HANDLE device)
{
    DWORD returned = 0;
    RSHARE_VDISPLAY_STATE state = {0};
    if (!DeviceIoControl(device, IOCTL_RSHARE_VDISPLAY_QUERY_STATE, NULL, 0, &state, sizeof(state), &returned, NULL)) {
        wprintf(L"vdisplay query state failed: %lu\n", GetLastError());
        return 16;
    }

    printf("vdisplay state abi=%u active=%u %lux%lu@%lu connector=%lu\n",
        state.Abi,
        state.Active,
        state.Width,
        state.Height,
        state.RefreshRateMillihz,
        state.ConnectorIndex);
    return 0;
}

static int probe_vdisplay(int argc, wchar_t** argv)
{
    if (argc < 3) {
        print_usage();
        return 13;
    }

    HANDLE device = open_device(RSHARE_VDISPLAY_DOS_DEVICE_NAME, L"vdisplay");
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
