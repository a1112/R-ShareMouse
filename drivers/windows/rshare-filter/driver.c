#include <ntddk.h>
#include <ntddkbd.h>
#include <ntddmou.h>
#include <wdf.h>
#include <wdmsec.h>
#include <kbdmou.h>

#include "..\rshare-common\rshare_ioctls.h"

#define RSHARE_EVENT_QUEUE_CAPACITY 128u

DRIVER_INITIALIZE DriverEntry;
EVT_WDF_DRIVER_DEVICE_ADD RShareFilterEvtDeviceAdd;
EVT_WDF_OBJECT_CONTEXT_CLEANUP RShareFilterEvtCleanup;
EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL RShareFilterEvtIoDeviceControl;
EVT_WDF_IO_QUEUE_IO_INTERNAL_DEVICE_CONTROL RShareFilterEvtIoInternalDeviceControl;

typedef struct _RSHARE_CONTROL_CONTEXT {
    WDFQUEUE Queue;
    RSHARE_DRIVER_EVENT Events[RSHARE_EVENT_QUEUE_CAPACITY];
    ULONG Head;
    ULONG Tail;
    ULONG Count;
    KSPIN_LOCK Lock;
} RSHARE_CONTROL_CONTEXT, *PRSHARE_CONTROL_CONTEXT;

typedef struct _RSHARE_FILTER_DEVICE_CONTEXT {
    CONNECT_DATA UpperConnectData;
    ULONG DeviceKind;
} RSHARE_FILTER_DEVICE_CONTEXT, *PRSHARE_FILTER_DEVICE_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RSHARE_CONTROL_CONTEXT, RShareControlGetContext)
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RSHARE_FILTER_DEVICE_CONTEXT, RShareFilterDeviceGetContext)

static WDFDEVICE g_RShareControlDevice;

static ULONGLONG RShareTimestampUs(void)
{
    LARGE_INTEGER now;
    KeQuerySystemTimePrecise(&now);
    return (ULONGLONG)(now.QuadPart / 10);
}

static ULONGLONG RShareDeviceToken(PDEVICE_OBJECT deviceObject, ULONG unitId)
{
    return (((ULONGLONG)(ULONG_PTR)deviceObject) << 16) ^ (ULONGLONG)unitId;
}

static VOID RShareQueueEvent(const RSHARE_DRIVER_EVENT* event)
{
    PRSHARE_CONTROL_CONTEXT context;
    KIRQL oldIrql;

    if (g_RShareControlDevice == NULL) {
        return;
    }

    context = RShareControlGetContext(g_RShareControlDevice);
    KeAcquireSpinLock(&context->Lock, &oldIrql);
    if (context->Count == RSHARE_EVENT_QUEUE_CAPACITY) {
        context->Tail = (context->Tail + 1u) % RSHARE_EVENT_QUEUE_CAPACITY;
        context->Count--;
    }

    context->Events[context->Head] = *event;
    context->Head = (context->Head + 1u) % RSHARE_EVENT_QUEUE_CAPACITY;
    context->Count++;
    KeReleaseSpinLock(&context->Lock, oldIrql);
}

static BOOLEAN RSharePopEvent(PRSHARE_CONTROL_CONTEXT context, PRSHARE_DRIVER_EVENT event)
{
    BOOLEAN hasEvent = FALSE;
    KIRQL oldIrql;

    KeAcquireSpinLock(&context->Lock, &oldIrql);
    if (context->Count > 0u) {
        *event = context->Events[context->Tail];
        context->Tail = (context->Tail + 1u) % RSHARE_EVENT_QUEUE_CAPACITY;
        context->Count--;
        hasEvent = TRUE;
    }
    KeReleaseSpinLock(&context->Lock, oldIrql);

    return hasEvent;
}

static VOID RShareSeedSyntheticEvent(ULONG deviceKind, ULONG eventKind)
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

    RShareQueueEvent(&event);
}

static VOID RShareQueueKeyboardEvent(PDEVICE_OBJECT deviceObject, PKEYBOARD_INPUT_DATA input)
{
    RSHARE_DRIVER_EVENT event;

    RtlZeroMemory(&event, sizeof(event));
    event.Abi = RSHARE_DRIVER_ABI;
    event.Source = RSHARE_SOURCE_HARDWARE;
    event.DeviceKind = RSHARE_DEVICE_KEYBOARD;
    event.EventKind = RSHARE_EVENT_KEY;
    event.Flags = input->Flags;
    event.DeviceId = RShareDeviceToken(deviceObject, input->UnitId);
    event.DeviceInstanceHash = event.DeviceId;
    event.Value0 = input->MakeCode;
    event.Value1 = (input->Flags & KEY_BREAK) != 0 ? 0 : 1;
    event.Value2 = input->Flags;
    event.TimestampUs = RShareTimestampUs();

    RShareQueueEvent(&event);
}

static VOID RShareQueueMouseEvent(PDEVICE_OBJECT deviceObject, PMOUSE_INPUT_DATA input)
{
    RSHARE_DRIVER_EVENT event;

    if (input->LastX != 0 || input->LastY != 0) {
        RtlZeroMemory(&event, sizeof(event));
        event.Abi = RSHARE_DRIVER_ABI;
        event.Source = RSHARE_SOURCE_HARDWARE;
        event.DeviceKind = RSHARE_DEVICE_MOUSE;
        event.EventKind = RSHARE_EVENT_MOUSE_MOVE;
        event.Flags = input->Flags;
        event.DeviceId = RShareDeviceToken(deviceObject, input->UnitId);
        event.DeviceInstanceHash = event.DeviceId;
        event.Value0 = input->LastX;
        event.Value1 = input->LastY;
        event.Value2 = input->Flags;
        event.TimestampUs = RShareTimestampUs();
        RShareQueueEvent(&event);
    }

    if (input->ButtonFlags != 0) {
        struct ButtonMap {
            USHORT DownFlag;
            USHORT UpFlag;
            LONG ButtonCode;
        };
        const struct ButtonMap buttons[] = {
            { MOUSE_LEFT_BUTTON_DOWN, MOUSE_LEFT_BUTTON_UP, 1 },
            { MOUSE_MIDDLE_BUTTON_DOWN, MOUSE_MIDDLE_BUTTON_UP, 2 },
            { MOUSE_RIGHT_BUTTON_DOWN, MOUSE_RIGHT_BUTTON_UP, 3 },
            { MOUSE_BUTTON_4_DOWN, MOUSE_BUTTON_4_UP, 4 },
            { MOUSE_BUTTON_5_DOWN, MOUSE_BUTTON_5_UP, 5 },
        };
        ULONG index;

        for (index = 0; index < ARRAYSIZE(buttons); index++) {
            if ((input->ButtonFlags & buttons[index].DownFlag) != 0 ||
                (input->ButtonFlags & buttons[index].UpFlag) != 0) {
                RtlZeroMemory(&event, sizeof(event));
                event.Abi = RSHARE_DRIVER_ABI;
                event.Source = RSHARE_SOURCE_HARDWARE;
                event.DeviceKind = RSHARE_DEVICE_MOUSE;
                event.EventKind = RSHARE_EVENT_MOUSE_BUTTON;
                event.Flags = input->Flags;
                event.DeviceId = RShareDeviceToken(deviceObject, input->UnitId);
                event.DeviceInstanceHash = event.DeviceId;
                event.Value0 = buttons[index].ButtonCode;
                event.Value1 = (input->ButtonFlags & buttons[index].DownFlag) != 0 ? 1 : 0;
                event.Value2 = input->ButtonFlags;
                event.TimestampUs = RShareTimestampUs();
                RShareQueueEvent(&event);
            }
        }
    }

    if ((input->ButtonFlags & MOUSE_WHEEL) != 0) {
        RtlZeroMemory(&event, sizeof(event));
        event.Abi = RSHARE_DRIVER_ABI;
        event.Source = RSHARE_SOURCE_HARDWARE;
        event.DeviceKind = RSHARE_DEVICE_MOUSE;
        event.EventKind = RSHARE_EVENT_MOUSE_WHEEL;
        event.Flags = input->Flags;
        event.DeviceId = RShareDeviceToken(deviceObject, input->UnitId);
        event.DeviceInstanceHash = event.DeviceId;
        event.Value0 = 0;
        event.Value1 = (SHORT)input->ButtonData;
        event.Value2 = input->ButtonFlags;
        event.TimestampUs = RShareTimestampUs();
        RShareQueueEvent(&event);
    }

    if ((input->ButtonFlags & MOUSE_HWHEEL) != 0) {
        RtlZeroMemory(&event, sizeof(event));
        event.Abi = RSHARE_DRIVER_ABI;
        event.Source = RSHARE_SOURCE_HARDWARE;
        event.DeviceKind = RSHARE_DEVICE_MOUSE;
        event.EventKind = RSHARE_EVENT_MOUSE_WHEEL;
        event.Flags = input->Flags;
        event.DeviceId = RShareDeviceToken(deviceObject, input->UnitId);
        event.DeviceInstanceHash = event.DeviceId;
        event.Value0 = (SHORT)input->ButtonData;
        event.Value1 = 0;
        event.Value2 = input->ButtonFlags;
        event.TimestampUs = RShareTimestampUs();
        RShareQueueEvent(&event);
    }
}

static VOID RShareFilterKeyboardServiceCallback(
    _In_ PDEVICE_OBJECT DeviceObject,
    _In_ PKEYBOARD_INPUT_DATA InputDataStart,
    _In_ PKEYBOARD_INPUT_DATA InputDataEnd,
    _Inout_ PULONG InputDataConsumed)
{
    WDFDEVICE device = WdfWdmDeviceGetWdfDeviceHandle(DeviceObject);
    PRSHARE_FILTER_DEVICE_CONTEXT context = RShareFilterDeviceGetContext(device);
    PKEYBOARD_INPUT_DATA input;

    for (input = InputDataStart; input < InputDataEnd; input++) {
        RShareQueueKeyboardEvent(DeviceObject, input);
    }

    (*(PSERVICE_CALLBACK_ROUTINE)context->UpperConnectData.ClassService)(
        context->UpperConnectData.ClassDeviceObject,
        InputDataStart,
        InputDataEnd,
        InputDataConsumed);
}

static VOID RShareFilterMouseServiceCallback(
    _In_ PDEVICE_OBJECT DeviceObject,
    _In_ PMOUSE_INPUT_DATA InputDataStart,
    _In_ PMOUSE_INPUT_DATA InputDataEnd,
    _Inout_ PULONG InputDataConsumed)
{
    WDFDEVICE device = WdfWdmDeviceGetWdfDeviceHandle(DeviceObject);
    PRSHARE_FILTER_DEVICE_CONTEXT context = RShareFilterDeviceGetContext(device);
    PMOUSE_INPUT_DATA input;

    for (input = InputDataStart; input < InputDataEnd; input++) {
        RShareQueueMouseEvent(DeviceObject, input);
    }

    (*(PSERVICE_CALLBACK_ROUTINE)context->UpperConnectData.ClassService)(
        context->UpperConnectData.ClassDeviceObject,
        InputDataStart,
        InputDataEnd,
        InputDataConsumed);
}

static VOID RShareForwardRequest(WDFDEVICE device, WDFREQUEST request)
{
    WDF_REQUEST_SEND_OPTIONS options;

    WDF_REQUEST_SEND_OPTIONS_INIT(&options, WDF_REQUEST_SEND_OPTION_SEND_AND_FORGET);
    if (!WdfRequestSend(request, WdfDeviceGetIoTarget(device), &options)) {
        WdfRequestComplete(request, WdfRequestGetStatus(request));
    }
}

static NTSTATUS RShareCreateControlDevice(WDFDRIVER driver, PWDFDEVICE_INIT deviceInit)
{
    WDF_OBJECT_ATTRIBUTES attributes;
    WDFDEVICE device;
    WDF_IO_QUEUE_CONFIG queueConfig;
    UNICODE_STRING symbolicLink;
    NTSTATUS status;
    PRSHARE_CONTROL_CONTEXT context;

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RSHARE_CONTROL_CONTEXT);
    status = WdfDeviceCreate(&deviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    RtlInitUnicodeString(&symbolicLink, RSHARE_DOS_DEVICE_NAME);
    status = WdfDeviceCreateSymbolicLink(device, &symbolicLink);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    context = RShareControlGetContext(device);
    KeInitializeSpinLock(&context->Lock);
    context->Head = 0;
    context->Tail = 0;
    context->Count = 0;

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchSequential);
    queueConfig.EvtIoDeviceControl = RShareFilterEvtIoDeviceControl;
    status = WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, &context->Queue);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    g_RShareControlDevice = device;
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

    g_RShareControlDevice = NULL;
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
    WDF_IO_QUEUE_CONFIG queueConfig;
    PRSHARE_FILTER_DEVICE_CONTEXT context;
    NTSTATUS status;

    UNREFERENCED_PARAMETER(Driver);

    WdfFdoInitSetFilter(DeviceInit);
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RSHARE_FILTER_DEVICE_CONTEXT);
    status = WdfDeviceCreate(&DeviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    context = RShareFilterDeviceGetContext(device);
    RtlZeroMemory(&context->UpperConnectData, sizeof(context->UpperConnectData));
    context->DeviceKind = 0;

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&queueConfig, WdfIoQueueDispatchParallel);
    queueConfig.EvtIoInternalDeviceControl = RShareFilterEvtIoInternalDeviceControl;
    status = WdfIoQueueCreate(device, &queueConfig, WDF_NO_OBJECT_ATTRIBUTES, WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    return STATUS_SUCCESS;
}

VOID RShareFilterEvtCleanup(WDFOBJECT DriverObject)
{
    UNREFERENCED_PARAMETER(DriverObject);
    g_RShareControlDevice = NULL;
}

VOID RShareFilterEvtIoInternalDeviceControl(
    WDFQUEUE Queue,
    WDFREQUEST Request,
    size_t OutputBufferLength,
    size_t InputBufferLength,
    ULONG IoControlCode)
{
    WDFDEVICE device = WdfIoQueueGetDevice(Queue);
    PRSHARE_FILTER_DEVICE_CONTEXT context = RShareFilterDeviceGetContext(device);
    WDF_REQUEST_PARAMETERS parameters;
    PCONNECT_DATA connectData;

    UNREFERENCED_PARAMETER(OutputBufferLength);
    UNREFERENCED_PARAMETER(InputBufferLength);

    WDF_REQUEST_PARAMETERS_INIT(&parameters);
    WdfRequestGetParameters(Request, &parameters);

    if (IoControlCode == IOCTL_INTERNAL_KEYBOARD_CONNECT ||
        IoControlCode == IOCTL_INTERNAL_MOUSE_CONNECT) {
        connectData = (PCONNECT_DATA)parameters.Parameters.DeviceIoControl.Type3InputBuffer;
        if (connectData == NULL) {
            WdfRequestComplete(Request, STATUS_INVALID_PARAMETER);
            return;
        }
        if (context->UpperConnectData.ClassService != NULL) {
            WdfRequestComplete(Request, STATUS_SHARING_VIOLATION);
            return;
        }

        context->UpperConnectData = *connectData;
        connectData->ClassDeviceObject = WdfDeviceWdmGetDeviceObject(device);
        if (IoControlCode == IOCTL_INTERNAL_KEYBOARD_CONNECT) {
            context->DeviceKind = RSHARE_DEVICE_KEYBOARD;
            connectData->ClassService = (PVOID)RShareFilterKeyboardServiceCallback;
        } else {
            context->DeviceKind = RSHARE_DEVICE_MOUSE;
            connectData->ClassService = (PVOID)RShareFilterMouseServiceCallback;
        }
        RShareForwardRequest(device, Request);
        return;
    }

    if (IoControlCode == IOCTL_INTERNAL_KEYBOARD_DISCONNECT ||
        IoControlCode == IOCTL_INTERNAL_MOUSE_DISCONNECT) {
        RtlZeroMemory(&context->UpperConnectData, sizeof(context->UpperConnectData));
        context->DeviceKind = 0;
    }

    RShareForwardRequest(device, Request);
}

VOID RShareFilterEvtIoDeviceControl(
    WDFQUEUE Queue,
    WDFREQUEST Request,
    size_t OutputBufferLength,
    size_t InputBufferLength,
    ULONG IoControlCode)
{
    WDFDEVICE device = WdfIoQueueGetDevice(Queue);
    PRSHARE_CONTROL_CONTEXT context = RShareControlGetContext(device);
    NTSTATUS status = STATUS_SUCCESS;
    size_t bytes = 0;

    switch (IoControlCode) {
    case IOCTL_RSHARE_QUERY_VERSION: {
        PRSHARE_DRIVER_VERSION version;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(*version), (PVOID*)&version, NULL);
        if (NT_SUCCESS(status)) {
            version->Major = 0;
            version->Minor = 2;
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
            RShareSeedSyntheticEvent(packet->DeviceKind, packet->EventKind);
        }
        break;
    }
    case IOCTL_RSHARE_READ_EVENT: {
        PRSHARE_DRIVER_EVENT event;
        status = WdfRequestRetrieveOutputBuffer(Request, sizeof(*event), (PVOID*)&event, NULL);
        if (NT_SUCCESS(status)) {
            if (RSharePopEvent(context, event)) {
                bytes = sizeof(*event);
            } else {
                status = STATUS_NO_MORE_ENTRIES;
            }
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
