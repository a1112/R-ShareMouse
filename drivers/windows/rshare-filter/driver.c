#include <ntddk.h>
#include <wdf.h>
#include <wdmsec.h>

#include "..\rshare-common\rshare_ioctls.h"

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD RShareFilterEvtDeviceAdd;
EVT_WDF_OBJECT_CONTEXT_CLEANUP RShareFilterEvtCleanup;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RShareFilterEvtIoDeviceControl;

typedef struct _RSHARE_FILTER_CONTEXT {
    WDFQUEUE Queue;
    RSHARE_DRIVER_EVENT LastEvent;
    BOOLEAN HasEvent;
    FAST_MUTEX Lock;
} RSHARE_FILTER_CONTEXT, *PRSHARE_FILTER_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RSHARE_FILTER_CONTEXT, RShareFilterGetContext)

static ULONGLONG RShareTimestampUs(void)
{
    LARGE_INTEGER now;
    KeQuerySystemTimePrecise(&now);
    return (ULONGLONG)(now.QuadPart / 10);
}

static VOID RShareSeedSyntheticEvent(PRSHARE_FILTER_CONTEXT context, ULONG deviceKind, ULONG eventKind)
{
    RSHARE_DRIVER_EVENT event;

    RtlZeroMemory(&event, sizeof(event));
    event.Abi = RSHARE_DRIVER_ABI;
    event.Source = RSHARE_SOURCE_DRIVER_TEST;
    event.DeviceKind = deviceKind;
    event.EventKind = eventKind;
    event.DeviceId = ((ULONGLONG)deviceKind << 32) | eventKind;
    event.DeviceInstanceHash = 0x5253484152450001ull;
    event.Value0 = deviceKind == RSHARE_DEVICE_KEYBOARD ? 0x10 : 4;
    event.Value1 = 1;
    event.TimestampUs = RShareTimestampUs();

    ExAcquireFastMutex(&context->Lock);
    context->LastEvent = event;
    context->HasEvent = TRUE;
    ExReleaseFastMutex(&context->Lock);
}

static NTSTATUS RShareCreateControlDevice(WDFDRIVER driver, PWDFDEVICE_INIT deviceInit)
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDFDEVICE device;
    WDF_IO_QUEUE_CONFIG queueConfig;
    UNICODE_STRING symbolicLink;
    NTSTATUS status;

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RSHARE_FILTER_CONTEXT);
    status = WdfDeviceCreate(&deviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    RtlInitUnicodeString(&symbolicLink, RSHARE_DOS_DEVICE_NAME);
    status = WdfDeviceCreateSymbolicLink(device, &symbolicLink);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    PRSHARE_FILTER_CONTEXT context = RShareFilterGetContext(device);
    ExInitializeFastMutex(&context->Lock);
    context->HasEvent = FALSE;

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = RShareFilterEvtIoDeviceControl;
    status = WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, &context->Queue);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WdfControlFinishInitializing(device);
    UNREFERENCED_PARAMETER(driver);
    return STATUS_SUCCESS;
}

NTSTATUS DriverEntry(PDRIVER_OBJECT DriverObject, PUNICODE_STRING RegistryPath)
{
    WDF_DRIVER_CONFIG config;
    WDF_OBJECT_ATTRIBUTES attributes;
    WDFDRIVER driver;
    PWDFDEVICE_INIT controlInit;
    UNICODE_STRING deviceName;
    UNICODE_STRING sddl;
    NTSTATUS status;

    WDF_DRIVER_CONFIG_INIT(&config, RShareFilterEvtDeviceAdd);
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    attributes.EvtCleanupCallback = RShareFilterEvtCleanup;

    status = WdfDriverCreate(DriverObject, RegistryPath, &attributes, &config, &driver);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    RtlInitUnicodeString(&deviceName, RSHARE_NT_DEVICE_NAME);
    RtlInitUnicodeString(&sddl, L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;BU)");
    controlInit = WdfControlDeviceInitAllocate(driver, &sddl);
    if (controlInit == NULL) {
        return STATUS_INSUFFICIENT_RESOURCES;
    }
    WdfDeviceInitAssignName(controlInit, &deviceName);
    status = RShareCreateControlDevice(driver, controlInit);
    if (!NT_SUCCESS(status)) {
        WdfDeviceInitFree(controlInit);
    }

    return status;
}

NTSTATUS RShareFilterEvtDeviceAdd(WDFDRIVER Driver, PWDFDEVICE_INIT DeviceInit)
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDFDEVICE device;

    UNREFERENCED_PARAMETER(Driver);

    WdfFdoInitSetFilter(DeviceInit);
    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    return WdfDeviceCreate(&DeviceInit, &attributes, &device);
}

VOID RShareFilterEvtCleanup(WDFOBJECT DriverObject)
{
    UNREFERENCED_PARAMETER(DriverObject);
}

VOID RShareFilterEvtIoDeviceControl(
    WDFQUEUE Queue,
    WDFREQUEST Request,
    size_t OutputBufferLength,
    size_t InputBufferLength,
    ULONG IoControlCode)
{
    WDFDEVICE device = WdfIoQueueGetDevice(Queue);
    PRSHARE_FILTER_CONTEXT context = RShareFilterGetContext(device);
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
            capabilities->Flags = RSHARE_CAP_FILTER_EVENTS;
            capabilities->MaxEventSize = sizeof(RSHARE_DRIVER_EVENT);
            capabilities->Reserved = 0;
            bytes = sizeof(*capabilities);
        }
        break;
    }
    case IOCTL_RSHARE_EMIT_TEST_PACKET: {
        PRSHARE_TEST_PACKET packet;
        status = WdfRequestRetrieveInputBuffer(Request, sizeof(*packet), (PVOID*)&packet, NULL);
        if (NT_SUCCESS(status)) {
            RShareSeedSyntheticEvent(context, packet->DeviceKind, packet->EventKind);
        }
        break;
    }
    case IOCTL_RSHARE_READ_EVENT: {
        PRSHARE_DRIVER_EVENT event;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(*event), (PVOID*)&event, NULL);
        if (NT_SUCCESS(status)) {
            ExAcquireFastMutex(&context->Lock);
            if (context->HasEvent) {
                *event = context->LastEvent;
                context->HasEvent = FALSE;
                bytes = sizeof(*event);
            } else {
                status = STATUS_NO_MORE_ENTRIES;
            }
            ExReleaseFastMutex(&context->Lock);
        }
        break;
    }
    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }

    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);
    WdfRequestCompleteWithInformation(Request, status, bytes);
}
