#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>

#include "..\rshare-common\rshare_ioctls.h"

static int probe_filter(void)
{
    HANDLE device = CreateFileW(
        L"\\\\.\\RShareInputControl",
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        NULL);

    if (device == INVALID_HANDLE_VALUE) {
        wprintf(L"open failed: %lu\n", GetLastError());
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
    HANDLE device = CreateFileW(
        L"\\\\.\\RShareVirtualHidControl",
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        NULL);

    if (device == INVALID_HANDLE_VALUE) {
        wprintf(L"vhid open failed: %lu\n", GetLastError());
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

int main(void)
{
    int filter_result = probe_filter();
    int vhid_result = probe_vhid();

    if (filter_result != 0) {
        return filter_result;
    }
    return vhid_result;
}
