#include <ntddk.h>
#include <wdf.h>
#include <wdmsec.h>
#include <vhf.h>

#include "..\rshare-common\rshare_ioctls.h"

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD RShareVhidEvtDeviceAdd;
EVT_WDF_OBJECT_CONTEXT_CLEANUP RShareVhidEvtCleanup;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RShareVhidEvtControlIoDeviceControl;

static FAST_MUTEX g_RShareVhidLock;
static VHFHANDLE g_RShareVhidHandle;
static UCHAR g_RShareMouseButtons;

static const UCHAR RShareKeyboardMouseReportDescriptor[] = {
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01,
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00,
    0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07,
    0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02,
    0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x05, 0x15, 0x00, 0x25, 0x01, 0x95, 0x05,
    0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03,
    0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
    0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08,
    0x95, 0x03, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38,
    0x02, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0
};

typedef struct _RSHARE_VHID_CONTEXT {
    VHFHANDLE VhfHandle;
} RSHARE_VHID_CONTEXT, *PRSHARE_VHID_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RSHARE_VHID_CONTEXT, RShareVhidGetContext)

static LONG RShareClampMouseDelta(LONG value)
{
    if (value > 127) {
        return 127;
    }
    if (value < -127) {
        return -127;
    }
    return value;
}

static UCHAR RShareMouseButtonMask(LONG button)
{
    switch (button) {
    case 1:
        return 0x01;
    case 2:
        return 0x04;
    case 3:
        return 0x02;
    case 4:
        return 0x08;
    case 5:
        return 0x10;
    default:
        return 0;
    }
}

static UCHAR RShareVkToModifier(LONG vk)
{
    switch (vk) {
    case 0x10:
    case 0xA0:
        return 0x02;
    case 0xA1:
        return 0x20;
    case 0x11:
    case 0xA2:
        return 0x01;
    case 0xA3:
        return 0x10;
    case 0x12:
    case 0xA4:
        return 0x04;
    case 0xA5:
        return 0x40;
    case 0x5B:
        return 0x08;
    case 0x5C:
        return 0x80;
    default:
        return 0;
    }
}

static UCHAR RShareVkToHidUsage(LONG vk)
{
    if (vk >= 'A' && vk <= 'Z') {
        return (UCHAR)(0x04 + (vk - 'A'));
    }
    if (vk >= '1' && vk <= '9') {
        return (UCHAR)(0x1E + (vk - '1'));
    }
    if (vk == '0') {
        return 0x27;
    }

    switch (vk) {
    case 0x0D:
        return 0x28;
    case 0x1B:
        return 0x29;
    case 0x08:
        return 0x2A;
    case 0x09:
        return 0x2B;
    case 0x20:
        return 0x2C;
    default:
        return 0;
    }
}

static NTSTATUS RShareSubmitKeyboardReport(VHFHANDLE handle, LONG vk, BOOLEAN pressed)
{
    UCHAR report[9] = {0};
    HID_XFER_PACKET packet;
    UCHAR modifier = RShareVkToModifier(vk);
    UCHAR usage = RShareVkToHidUsage(vk);

    report[0] = 0x01;
    if (modifier != 0) {
        report[1] = pressed ? modifier : 0;
    } else if (usage != 0 && pressed) {
        report[3] = usage;
    } else if (usage == 0) {
        return STATUS_NOT_SUPPORTED;
    }

    RtlZeroMemory(&packet, sizeof(packet));
    packet.reportBuffer = report;
    packet.reportBufferLen = sizeof(report);
    packet.reportId = report[0];
    return VhfReadReportSubmit(handle, &packet);
}

static NTSTATUS RShareSubmitMouseReport(VHFHANDLE handle, UCHAR buttons, LONG dx, LONG dy, LONG wheel, LONG horizontalWheel)
{
    UCHAR report[6] = {0};
    HID_XFER_PACKET packet;

    report[0] = 0x02;
    report[1] = buttons & 0x1F;
    report[2] = (UCHAR)(CHAR)RShareClampMouseDelta(dx);
    report[3] = (UCHAR)(CHAR)RShareClampMouseDelta(dy);
    report[4] = (UCHAR)(CHAR)RShareClampMouseDelta(wheel);
    report[5] = (UCHAR)(CHAR)RShareClampMouseDelta(horizontalWheel);

    RtlZeroMemory(&packet, sizeof(packet));
    packet.reportBuffer = report;
    packet.reportBufferLen = sizeof(report);
    packet.reportId = report[0];
    return VhfReadReportSubmit(handle, &packet);
}

static NTSTATUS RShareSubmitInjectReport(PRSHARE_INJECT_REPORT report)
{
    NTSTATUS status;
    UCHAR buttonMask;

    ExAcquireFastMutex(&g_RShareVhidLock);
    if (g_RShareVhidHandle == NULL) {
        status = STATUS_DEVICE_NOT_READY;
    } else if (report->ReportKind == RSHARE_REPORT_KEYBOARD) {
        status = RShareSubmitKeyboardReport(g_RShareVhidHandle, report->Value0, report->Value1 != 0);
    } else if (report->ReportKind == RSHARE_REPORT_MOUSE_MOVE) {
        status = RShareSubmitMouseReport(g_RShareVhidHandle, g_RShareMouseButtons, report->Value0, report->Value1, 0, 0);
    } else if (report->ReportKind == RSHARE_REPORT_MOUSE_BUTTON) {
        buttonMask = RShareMouseButtonMask(report->Value0);
        if (buttonMask == 0) {
            status = STATUS_NOT_SUPPORTED;
        } else {
            if (report->Value1 != 0) {
                g_RShareMouseButtons |= buttonMask;
            } else {
                g_RShareMouseButtons &= (UCHAR)~buttonMask;
            }
            status = RShareSubmitMouseReport(g_RShareVhidHandle, g_RShareMouseButtons, 0, 0, 0, 0);
        }
    } else if (report->ReportKind == RSHARE_REPORT_MOUSE_WHEEL) {
        status = RShareSubmitMouseReport(g_RShareVhidHandle, g_RShareMouseButtons, 0, 0, report->Value1, report->Value0);
    } else {
        status = STATUS_INVALID_DEVICE_REQUEST;
    }
    ExReleaseFastMutex(&g_RShareVhidLock);

    return status;
}

static NTSTATUS RShareVhidCreateControlDevice(WDFDRIVER driver)
{
    WDFDEVICE device;
    WDF_IO_QUEUE_CONFIG queueConfig;
    WDF_OBJECT_ATTRIBUTES attributes;
    PWDFDEVICE_INIT deviceInit;
    UNICODE_STRING deviceName;
    UNICODE_STRING symbolicLink;
    UNICODE_STRING sddl;
    NTSTATUS status;

    RtlInitUnicodeString(&sddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;BU)");
    deviceInit = WdfControlDeviceInitAllocate(driver, &sddl);
    if (deviceInit == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }

    RtlInitUnicodeString(&deviceName, RSHARE_VHID_NT_DEVICE_NAME);
    status = WdfDeviceInitAssignName(deviceInit, &deviceName);
    if (!NT_SUCCESS(status)) {
        WdfDeviceInitFree(deviceInit);
        return status;
    }

    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    status = WdfDeviceCreate(&deviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        WdfDeviceInitFree(deviceInit);
        return status;
    }

    RtlInitUnicodeString(&symbolicLink, RSHARE_VHID_DOS_DEVICE_NAME);
    status = WdfDeviceCreateSymbolicLink(device, &symbolicLink);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = RShareVhidEvtControlIoDeviceControl;
    status = WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WdfControlFinishInitializing(device);
    return STATUS_SUCCESS;
}

NTSTATUS DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    WDF_DRIVER_CONFIG config;
    WDF_OBJECT_ATTRIBUTES attributes;
    WDFDRIVER driver;
    NTSTATUS status;

    ExInitializeFastMutex(&g_RShareVhidLock);
    g_RShareMouseButtons = 0;
    WDF_DRIVER_CONFIG_INIT(&config, RShareVhidEvtDeviceAdd);
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    status = WdfDriverCreate(DriverObject, RegistryPath, &attributes, &config, &driver);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    return RShareVhidCreateControlDevice(driver);
}

NTSTATUS RShareVhidEvtDeviceAdd(WDFDRIVER Driver, PWDFDEVICE_INIT DeviceInit)
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDFDEVICE device;
    VHF_CONFIG vhfConfig;
    PRSHARE_VHID_CONTEXT context;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(Driver);

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RSHARE_VHID_CONTEXT);
    attributes.EvtCleanupCallback = RShareVhidEvtCleanup;
    status = WdfDeviceCreate(&DeviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    context = RShareVhidGetContext(device);
    VHF_CONFIG_INIT(&vhfConfig, WdfDeviceWdmGetDeviceObject(device), sizeof(RShareKeyboardMouseReportDescriptor), (PUCHAR)RShareKeyboardMouseReportDescriptor);
    status = VhfCreate(&vhfConfig, &context->VhfHandle);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    status = VhfStart(context->VhfHandle);
    if (!NT_SUCCESS(status)) {
        VhfDelete(context->VhfHandle, TRUE);
        context->VhfHandle = NULL;
        return status;
    }

    ExAcquireFastMutex(&g_RShareVhidLock);
    g_RShareVhidHandle = context->VhfHandle;
    ExReleaseFastMutex(&g_RShareVhidLock);

    return STATUS_SUCCESS;
}

VOID RShareVhidEvtCleanup(WDFOBJECT DeviceObject)
{
    PRSHARE_VHID_CONTEXT context = RShareVhidGetContext(DeviceObject);

    if (context->VhfHandle != NULL) {
        ExAcquireFastMutex(&g_RShareVhidLock);
        if (g_RShareVhidHandle == context->VhfHandle) {
            g_RShareVhidHandle = NULL;
            g_RShareMouseButtons = 0;
        }
        ExReleaseFastMutex(&g_RShareVhidLock);

        VhfDelete(context->VhfHandle, TRUE);
        context->VhfHandle = NULL;
    }
}

VOID RShareVhidEvtControlIoDeviceControl(
    WDFQUEUE Queue,
    WDFREQUEST Request,
    size_t OutputBufferLength,
    size_t InputBufferLength,
    ULONG IoControlCode)
{
    NTSTATUS status = STATUS_SUCCESS;
    size_t bytes = 0;

    switch (IoControlCode) {
    case IOCTL_RSHARE_QUERY_VERSION: {
        PRSHARE_DRIVER_VERSION version;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(*version), (PVOID*)&version, NULL);
        if (NT_SUCCESS(status)) {
            version->Major = 0;
            version->Minor = 1;
            version->Patch = 0;
            version->Abi = RSHARE_DRIVER_ABI;
            bytes = sizeof(*version);
        }
        break;
    }
    case IOCTL_RSHARE_QUERY_CAPABILITIES: {
        PRSHARE_DRIVER_CAPABILITIES capabilities;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(*capabilities), (PVOID*)&capabilities, NULL);
        if (NT_SUCCESS(status)) {
            capabilities->Abi = RSHARE_DRIVER_ABI;
            capabilities->Flags = RSHARE_CAP_VIRTUAL_KEYBOARD | RSHARE_CAP_VIRTUAL_MOUSE | RSHARE_CAP_VIRTUAL_GAMEPAD_SCAFFOLD;
            capabilities->MaxEventSize = sizeof(RSHARE_DRIVER_EVENT);
            capabilities->Reserved = 0;
            bytes = sizeof(*capabilities);
        }
        break;
    }
    case IOCTL_RSHARE_INJECT_REPORT:
        {
            PRSHARE_INJECT_REPORT report;
            status = WdfRequestRetrieveInputBuffer(Request, sizeof(*report), (PVOID*)&report, NULL);
            if (NT_SUCCESS(status)) {
                status = RShareSubmitInjectReport(report);
            }
        }
        break;
    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }

    UNREFERENCED_PARAMETER(Queue);
    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);
    WdfRequestCompleteWithInformation(Request, status, bytes);
}
