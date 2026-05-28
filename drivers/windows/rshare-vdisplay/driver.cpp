#include <initguid.h>

#include "driver.h"

using namespace Microsoft::WRL;
using namespace RShare::VirtualDisplay;

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

static constexpr DWORD RSHARE_VDISPLAY_MONITOR_COUNT = 1;

// {fb3b7c60-8355-4ea0-94c4-ae166915b937}
static constexpr GUID RSHARE_VDISPLAY_MONITOR_CONTAINER_ID = {
    0xfb3b7c60,
    0x8355,
    0x4ea0,
    {0x94, 0xc4, 0xae, 0x16, 0x69, 0x15, 0xb9, 0x37},
};

// EDID monitor name: RSHARE VDISP; serial: RSM00000001.
static const BYTE s_RShareVirtualDisplayEdid[] = {
    0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x4A, 0x6D, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x24, 0x01, 0x04, 0xA5, 0x34, 0x1D, 0x78, 0x0A, 0xCF, 0x74, 0xA3, 0x57, 0x4C, 0xB0, 0x23,
    0x09, 0x48, 0x4C, 0x21, 0x08, 0x00, 0x81, 0x80, 0x95, 0x00, 0xA9, 0x40, 0xB3, 0x00, 0xD1, 0xC0,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x3A, 0x80, 0x18, 0x71, 0x38, 0x2D, 0x40, 0x58, 0x2C,
    0x45, 0x00, 0x08, 0x22, 0x21, 0x00, 0x00, 0x1E, 0x00, 0x00, 0x00, 0xFC, 0x00, 'R', 'S', 'H',
    'A', 'R', 'E', ' ', 'V', 'D', 'I', 'S', 'P', 0x0A, 0x00, 0x00, 0x00, 0xFD, 0x00, 0x30,
    0x90, 0x1E, 0xA0, 0x3C, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xFF,
    0x00, 'R', 'S', 'M', '0', '0', '0', '0', '0', '0', '0', '1', 0x20, 0x20, 0x0A, 0xF6,
};

static const RShareDisplayMode RShareMonitorModes[] = {
    {3840, 2160, 60000},
    {2560, 1440, 144000},
    {2560, 1440, 90000},
    {2560, 1440, 60000},
    {1920, 1080, 144000},
    {1920, 1080, 90000},
    {1920, 1080, 60000},
    {1600, 900, 60000},
    {1280, 720, 90000},
    {1280, 720, 60000},
    {1024, 768, 75000},
    {1024, 768, 60000},
};

static RShareDisplayMode RShareModeFromState(const RSHARE_VDISPLAY_STATE& state)
{
    return {
        state.Width == 0 ? RShareMonitorModes[0].Width : state.Width,
        state.Height == 0 ? RShareMonitorModes[0].Height : state.Height,
        state.RefreshRateMillihz == 0 ? RShareMonitorModes[0].RefreshRateMillihz : state.RefreshRateMillihz,
    };
}

static std::vector<RShareDisplayMode> RShareModesForState(const RSHARE_VDISPLAY_STATE& state)
{
    std::vector<RShareDisplayMode> modes;
    modes.push_back(RShareModeFromState(state));

    for (const auto& mode : RShareMonitorModes) {
        bool duplicate = false;
        for (const auto& existing : modes) {
            if (existing.Width == mode.Width && existing.Height == mode.Height && existing.RefreshRateMillihz == mode.RefreshRateMillihz) {
                duplicate = true;
                break;
            }
        }
        if (!duplicate) {
            modes.push_back(mode);
        }
    }

    return modes;
}

static DWORD RShareRefreshMillihzFromSignalInfo(const DISPLAYCONFIG_VIDEO_SIGNAL_INFO& signalInfo)
{
    UINT denominator = signalInfo.vSyncFreq.Denominator == 0 ? 1 : signalInfo.vSyncFreq.Denominator;
    UINT divider = signalInfo.AdditionalSignalInfo.vSyncFreqDivider == 0 ? 1 : signalInfo.AdditionalSignalInfo.vSyncFreqDivider;
    UINT64 millihz = (static_cast<UINT64>(signalInfo.vSyncFreq.Numerator) * 1000u) / denominator;
    millihz /= divider;
    return millihz == 0 ? RShareMonitorModes[0].RefreshRateMillihz : static_cast<DWORD>(millihz);
}

static RShareDisplayMode RShareModeFromSignalInfo(const DISPLAYCONFIG_VIDEO_SIGNAL_INFO& signalInfo)
{
    return {
        signalInfo.activeSize.cx == 0 ? signalInfo.totalSize.cx : signalInfo.activeSize.cx,
        signalInfo.activeSize.cy == 0 ? signalInfo.totalSize.cy : signalInfo.activeSize.cy,
        RShareRefreshMillihzFromSignalInfo(signalInfo),
    };
}

extern "C" DRIVER_INITIALIZE DriverEntry;

EVT_WDF_DRIVER_DEVICE_ADD RShareVDisplayDeviceAdd;
EVT_WDF_DEVICE_D0_ENTRY RShareVDisplayDeviceD0Entry;

EVT_IDD_CX_DEVICE_IO_CONTROL RShareVDisplayDeviceIoControl;
EVT_IDD_CX_ADAPTER_INIT_FINISHED RShareVDisplayAdapterInitFinished;
EVT_IDD_CX_ADAPTER_COMMIT_MODES RShareVDisplayAdapterCommitModes;
EVT_IDD_CX_PARSE_MONITOR_DESCRIPTION RShareVDisplayParseMonitorDescription;
EVT_IDD_CX_MONITOR_GET_DEFAULT_DESCRIPTION_MODES RShareVDisplayMonitorGetDefaultModes;
EVT_IDD_CX_MONITOR_QUERY_TARGET_MODES RShareVDisplayMonitorQueryModes;
EVT_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN RShareVDisplayMonitorAssignSwapChain;
EVT_IDD_CX_MONITOR_UNASSIGN_SWAPCHAIN RShareVDisplayMonitorUnassignSwapChain;

struct RShareDeviceContext
{
    RShareVirtualDisplayDevice* Device;
};

struct RShareMonitorContext
{
    RShareVirtualDisplayMonitor* Monitor;
};

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RShareDeviceContext, RShareGetDeviceContext)
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(RShareMonitorContext, RShareGetMonitorContext)

extern "C" BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved)
{
    UNREFERENCED_PARAMETER(instance);
    UNREFERENCED_PARAMETER(reason);
    UNREFERENCED_PARAMETER(reserved);
    return TRUE;
}

static void RShareDeviceContextCleanup(WDFOBJECT object)
{
    auto context = RShareGetDeviceContext(object);
    if (context != nullptr) {
        delete context->Device;
        context->Device = nullptr;
    }
}

static void RShareMonitorContextCleanup(WDFOBJECT object)
{
    auto context = RShareGetMonitorContext(object);
    if (context != nullptr) {
        delete context->Monitor;
        context->Monitor = nullptr;
    }
}

static void RShareFillSignalInfo(
    DISPLAYCONFIG_VIDEO_SIGNAL_INFO& signalInfo,
    DWORD width,
    DWORD height,
    DWORD refreshRateMillihz,
    bool monitorMode)
{
    signalInfo.totalSize.cx = width;
    signalInfo.totalSize.cy = height;
    signalInfo.activeSize.cx = width;
    signalInfo.activeSize.cy = height;
    signalInfo.vSyncFreq.Numerator = refreshRateMillihz;
    signalInfo.vSyncFreq.Denominator = 1000;
    signalInfo.hSyncFreq.Numerator = refreshRateMillihz * height;
    signalInfo.hSyncFreq.Denominator = 1000;
    signalInfo.pixelRate = (static_cast<UINT64>(refreshRateMillihz) * static_cast<UINT64>(width) * static_cast<UINT64>(height)) / 1000u;
    signalInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
    signalInfo.AdditionalSignalInfo.videoStandard = 255;
    signalInfo.AdditionalSignalInfo.vSyncFreqDivider = monitorMode ? 0 : 1;
}

static IDDCX_MONITOR_MODE RShareCreateMonitorMode(const RShareDisplayMode& mode)
{
    IDDCX_MONITOR_MODE monitorMode = {};
    monitorMode.Size = sizeof(monitorMode);
    monitorMode.Origin = IDDCX_MONITOR_MODE_ORIGIN_DRIVER;
    RShareFillSignalInfo(monitorMode.MonitorVideoSignalInfo, mode.Width, mode.Height, mode.RefreshRateMillihz, true);
    return monitorMode;
}

static IDDCX_TARGET_MODE RShareCreateTargetMode(const RShareDisplayMode& mode)
{
    IDDCX_TARGET_MODE targetMode = {};
    targetMode.Size = sizeof(targetMode);
    RShareFillSignalInfo(targetMode.TargetVideoSignalInfo.targetVideoSignalInfo, mode.Width, mode.Height, mode.RefreshRateMillihz, false);
    return targetMode;
}

_Use_decl_annotations_
extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT driverObject, PUNICODE_STRING registryPath)
{
    WDF_DRIVER_CONFIG config;
    WDF_OBJECT_ATTRIBUTES attributes;

    WDF_OBJECT_ATTRIBUTES_INIT(&attributes);
    WDF_DRIVER_CONFIG_INIT(&config, RShareVDisplayDeviceAdd);

    return WdfDriverCreate(driverObject, registryPath, &attributes, &config, WDF_NO_HANDLE);
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayDeviceAdd(WDFDRIVER driver, PWDFDEVICE_INIT deviceInit)
{
    UNREFERENCED_PARAMETER(driver);

    WDF_PNPPOWER_EVENT_CALLBACKS powerCallbacks;
    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&powerCallbacks);
    powerCallbacks.EvtDeviceD0Entry = RShareVDisplayDeviceD0Entry;
    WdfDeviceInitSetPnpPowerEventCallbacks(deviceInit, &powerCallbacks);

    IDD_CX_CLIENT_CONFIG iddConfig;
    IDD_CX_CLIENT_CONFIG_INIT(&iddConfig);
    iddConfig.EvtIddCxAdapterInitFinished = RShareVDisplayAdapterInitFinished;
    iddConfig.EvtIddCxDeviceIoControl = RShareVDisplayDeviceIoControl;
    iddConfig.EvtIddCxAdapterCommitModes = RShareVDisplayAdapterCommitModes;
    iddConfig.EvtIddCxParseMonitorDescription = RShareVDisplayParseMonitorDescription;
    iddConfig.EvtIddCxMonitorGetDefaultDescriptionModes = RShareVDisplayMonitorGetDefaultModes;
    iddConfig.EvtIddCxMonitorQueryTargetModes = RShareVDisplayMonitorQueryModes;
    iddConfig.EvtIddCxMonitorAssignSwapChain = RShareVDisplayMonitorAssignSwapChain;
    iddConfig.EvtIddCxMonitorUnassignSwapChain = RShareVDisplayMonitorUnassignSwapChain;

    NTSTATUS status = IddCxDeviceInitConfig(deviceInit, &iddConfig);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RShareDeviceContext);
    attributes.EvtCleanupCallback = RShareDeviceContextCleanup;

    WDFDEVICE device = nullptr;
    status = WdfDeviceCreate(&deviceInit, &attributes, &device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    status = IddCxDeviceInitialize(device);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    status = WdfDeviceCreateDeviceInterface(device, &GUID_DEVINTERFACE_RSHARE_VDISPLAY, nullptr);
    if (!NT_SUCCESS(status)) {
        return status;
    }

    auto context = RShareGetDeviceContext(device);
    context->Device = new RShareVirtualDisplayDevice(device);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayDeviceD0Entry(WDFDEVICE device, WDF_POWER_DEVICE_STATE previousState)
{
    UNREFERENCED_PARAMETER(previousState);

    auto context = RShareGetDeviceContext(device);
    if (context == nullptr || context->Device == nullptr) {
        return STATUS_DEVICE_NOT_READY;
    }

    context->Device->InitAdapter();
    return STATUS_SUCCESS;
}

RShareDirect3DDevice::RShareDirect3DDevice(LUID adapterLuid)
    : m_AdapterLuid(adapterLuid)
{
}

HRESULT RShareDirect3DDevice::Init()
{
    HRESULT hr = CreateDXGIFactory2(0, IID_PPV_ARGS(&m_DxgiFactory));
    if (FAILED(hr)) {
        return hr;
    }

    hr = m_DxgiFactory->EnumAdapterByLuid(m_AdapterLuid, IID_PPV_ARGS(&m_Adapter));
    if (FAILED(hr)) {
        return hr;
    }

    return D3D11CreateDevice(
        m_Adapter.Get(),
        D3D_DRIVER_TYPE_UNKNOWN,
        nullptr,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        nullptr,
        0,
        D3D11_SDK_VERSION,
        &Device,
        nullptr,
        &m_DeviceContext);
}

RShareSwapChainProcessor::RShareSwapChainProcessor(
    IDDCX_SWAPCHAIN swapChain,
    std::shared_ptr<RShareDirect3DDevice> device,
    HANDLE newFrameEvent)
    : m_SwapChain(swapChain),
      m_Device(device),
      m_NewFrameEvent(newFrameEvent)
{
    m_TerminateEvent.Attach(CreateEvent(nullptr, FALSE, FALSE, nullptr));
    m_Thread.Attach(CreateThread(nullptr, 0, RunThread, this, 0, nullptr));
}

RShareSwapChainProcessor::~RShareSwapChainProcessor()
{
    SetEvent(m_TerminateEvent.Get());
    if (m_Thread.Get() != nullptr) {
        WaitForSingleObject(m_Thread.Get(), INFINITE);
    }
}

DWORD CALLBACK RShareSwapChainProcessor::RunThread(LPVOID argument)
{
    reinterpret_cast<RShareSwapChainProcessor*>(argument)->Run();
    return 0;
}

void RShareSwapChainProcessor::Run()
{
    DWORD mmcssTask = 0;
    HANDLE mmcssHandle = AvSetMmThreadCharacteristicsW(L"Distribution", &mmcssTask);

    RunCore();

    if (m_SwapChain != nullptr) {
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(m_SwapChain));
        m_SwapChain = nullptr;
    }

    if (mmcssHandle != nullptr) {
        AvRevertMmThreadCharacteristics(mmcssHandle);
    }
}

void RShareSwapChainProcessor::RunCore()
{
    ComPtr<IDXGIDevice> dxgiDevice;
    HRESULT hr = m_Device->Device.As(&dxgiDevice);
    if (FAILED(hr)) {
        return;
    }

    IDARG_IN_SWAPCHAINSETDEVICE setDevice = {};
    setDevice.pDevice = dxgiDevice.Get();
    hr = IddCxSwapChainSetDevice(m_SwapChain, &setDevice);
    if (FAILED(hr)) {
        return;
    }

    for (;;) {
        IDARG_OUT_RELEASEANDACQUIREBUFFER buffer = {};
        hr = IddCxSwapChainReleaseAndAcquireBuffer(m_SwapChain, &buffer);
        if (hr == E_PENDING) {
            HANDLE waitHandles[] = {m_NewFrameEvent, m_TerminateEvent.Get()};
            DWORD waitResult = WaitForMultipleObjects(ARRAYSIZE(waitHandles), waitHandles, FALSE, 16);
            if (waitResult == WAIT_OBJECT_0 || waitResult == WAIT_TIMEOUT) {
                continue;
            }
            if (waitResult == WAIT_OBJECT_0 + 1) {
                break;
            }
            break;
        }

        if (FAILED(hr)) {
            break;
        }

        ComPtr<IDXGIResource> acquiredBuffer;
        acquiredBuffer.Attach(buffer.MetaData.pSurface);
        acquiredBuffer.Reset();

        hr = IddCxSwapChainFinishedProcessingFrame(m_SwapChain);
        if (FAILED(hr)) {
            break;
        }
    }
}

RShareVirtualDisplayMonitor::RShareVirtualDisplayMonitor(IDDCX_MONITOR monitor)
    : m_Monitor(monitor),
      m_State{}
{
    m_State.Abi = RSHARE_DRIVER_ABI;
    m_State.Width = RShareMonitorModes[0].Width;
    m_State.Height = RShareMonitorModes[0].Height;
    m_State.RefreshRateMillihz = RShareMonitorModes[0].RefreshRateMillihz;
}

RShareVirtualDisplayMonitor::~RShareVirtualDisplayMonitor()
{
    m_Processor.reset();
}

void RShareVirtualDisplayMonitor::UpdateMode(const RSHARE_VDISPLAY_STATE& state)
{
    m_State = state;
}

NTSTATUS RShareVirtualDisplayMonitor::CopyDefaultModes(
    const IDARG_IN_GETDEFAULTDESCRIPTIONMODES* inArgs,
    IDARG_OUT_GETDEFAULTDESCRIPTIONMODES* outArgs) const
{
    auto modes = RShareModesForState(m_State);
    outArgs->DefaultMonitorModeBufferOutputCount = static_cast<UINT>(modes.size());
    outArgs->PreferredMonitorModeIdx = 0;

    if (inArgs->DefaultMonitorModeBufferInputCount == 0) {
        return STATUS_SUCCESS;
    }

    if (inArgs->DefaultMonitorModeBufferInputCount < modes.size()) {
        return STATUS_BUFFER_TOO_SMALL;
    }

    for (size_t index = 0; index < modes.size(); index++) {
        inArgs->pDefaultMonitorModes[index] = RShareCreateMonitorMode(modes[index]);
    }

    return STATUS_SUCCESS;
}

NTSTATUS RShareVirtualDisplayMonitor::CopyTargetModes(
    const IDARG_IN_QUERYTARGETMODES* inArgs,
    IDARG_OUT_QUERYTARGETMODES* outArgs) const
{
    auto modes = RShareModesForState(m_State);
    outArgs->TargetModeBufferOutputCount = static_cast<UINT>(modes.size());

    if (inArgs->TargetModeBufferInputCount == 0) {
        return STATUS_SUCCESS;
    }

    if (inArgs->TargetModeBufferInputCount < modes.size()) {
        return STATUS_BUFFER_TOO_SMALL;
    }

    for (size_t index = 0; index < modes.size(); index++) {
        inArgs->pTargetModes[index] = RShareCreateTargetMode(modes[index]);
    }

    return STATUS_SUCCESS;
}

void RShareVirtualDisplayMonitor::AssignSwapChain(IDDCX_SWAPCHAIN swapChain, LUID renderAdapter, HANDLE newFrameEvent)
{
    m_Processor.reset();

    auto device = std::make_shared<RShareDirect3DDevice>(renderAdapter);
    if (FAILED(device->Init())) {
        WdfObjectDelete(reinterpret_cast<WDFOBJECT>(swapChain));
        return;
    }

    m_Processor.reset(new RShareSwapChainProcessor(swapChain, device, newFrameEvent));
}

void RShareVirtualDisplayMonitor::UnassignSwapChain()
{
    m_Processor.reset();
}

RShareVirtualDisplayDevice::RShareVirtualDisplayDevice(WDFDEVICE device)
    : m_Device(device),
      m_Adapter(nullptr),
      m_Monitor(nullptr),
      m_State{},
      m_MonitorRequested(false)
{
    m_State.Abi = RSHARE_DRIVER_ABI;
    m_State.Active = 0;
    m_State.Width = RShareMonitorModes[0].Width;
    m_State.Height = RShareMonitorModes[0].Height;
    m_State.RefreshRateMillihz = RShareMonitorModes[0].RefreshRateMillihz;
    m_State.ConnectorIndex = 0;
}

void RShareVirtualDisplayDevice::InitAdapter()
{
    if (m_Adapter != nullptr) {
        return;
    }

    IDDCX_ADAPTER_CAPS caps = {};
    caps.Size = sizeof(caps);
    caps.MaxMonitorsSupported = RSHARE_VDISPLAY_MONITOR_COUNT;
    caps.EndPointDiagnostics.Size = sizeof(caps.EndPointDiagnostics);
    caps.EndPointDiagnostics.GammaSupport = IDDCX_FEATURE_IMPLEMENTATION_NONE;
    caps.EndPointDiagnostics.TransmissionType = IDDCX_TRANSMISSION_TYPE_WIRED_OTHER;
    caps.EndPointDiagnostics.pEndPointFriendlyName = L"R-ShareMouse Virtual Display";
    caps.EndPointDiagnostics.pEndPointManufacturerName = L"R-ShareMouse";
    caps.EndPointDiagnostics.pEndPointModelName = L"RShareVDisplay";

    IDDCX_ENDPOINT_VERSION version = {};
    version.Size = sizeof(version);
    version.MajorVer = 1;
    caps.EndPointDiagnostics.pFirmwareVersion = &version;
    caps.EndPointDiagnostics.pHardwareVersion = &version;

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RShareDeviceContext);

    IDARG_IN_ADAPTER_INIT adapterInit = {};
    adapterInit.WdfDevice = m_Device;
    adapterInit.pCaps = &caps;
    adapterInit.ObjectAttributes = &attributes;

    IDARG_OUT_ADAPTER_INIT adapterInitOut = {};
    NTSTATUS status = IddCxAdapterInitAsync(&adapterInit, &adapterInitOut);
    if (NT_SUCCESS(status)) {
        m_Adapter = adapterInitOut.AdapterObject;
        auto context = RShareGetDeviceContext(adapterInitOut.AdapterObject);
        context->Device = this;
    }
}

void RShareVirtualDisplayDevice::ReportMonitorArrival(UINT connectorIndex)
{
    if (m_Adapter == nullptr || m_Monitor != nullptr) {
        return;
    }

    WDF_OBJECT_ATTRIBUTES attributes;
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attributes, RShareMonitorContext);
    attributes.EvtCleanupCallback = RShareMonitorContextCleanup;

    IDDCX_MONITOR_INFO monitorInfo = {};
    monitorInfo.Size = sizeof(monitorInfo);
    monitorInfo.MonitorType = DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI;
    monitorInfo.ConnectorIndex = connectorIndex;
    monitorInfo.MonitorDescription.Size = sizeof(monitorInfo.MonitorDescription);
    monitorInfo.MonitorDescription.Type = IDDCX_MONITOR_DESCRIPTION_TYPE_EDID;
    monitorInfo.MonitorDescription.DataSize = sizeof(s_RShareVirtualDisplayEdid);
    monitorInfo.MonitorDescription.pData = const_cast<BYTE*>(s_RShareVirtualDisplayEdid);
    monitorInfo.MonitorContainerId = RSHARE_VDISPLAY_MONITOR_CONTAINER_ID;

    IDARG_IN_MONITORCREATE monitorCreate = {};
    monitorCreate.ObjectAttributes = &attributes;
    monitorCreate.pMonitorInfo = &monitorInfo;

    IDARG_OUT_MONITORCREATE monitorCreateOut = {};
    NTSTATUS status = IddCxMonitorCreate(m_Adapter, &monitorCreate, &monitorCreateOut);
    if (!NT_SUCCESS(status)) {
        return;
    }

    m_Monitor = monitorCreateOut.MonitorObject;
    auto monitorContext = RShareGetMonitorContext(monitorCreateOut.MonitorObject);
    monitorContext->Monitor = new RShareVirtualDisplayMonitor(monitorCreateOut.MonitorObject);
    monitorContext->Monitor->UpdateMode(m_State);

    IDARG_OUT_MONITORARRIVAL arrivalOut = {};
    status = IddCxMonitorArrival(monitorCreateOut.MonitorObject, &arrivalOut);
    if (NT_SUCCESS(status)) {
        m_State.Active = 1;
        m_State.ConnectorIndex = connectorIndex;
    }
}

void RShareVirtualDisplayDevice::ReportPendingMonitorArrival()
{
    if (m_MonitorRequested) {
        ReportMonitorArrival(0);
    }
}

NTSTATUS RShareVirtualDisplayDevice::QueryState(PRSHARE_VDISPLAY_STATE state, size_t outputSize)
{
    if (state == nullptr || outputSize < sizeof(RSHARE_VDISPLAY_STATE)) {
        return STATUS_BUFFER_TOO_SMALL;
    }

    *state = m_State;
    return STATUS_SUCCESS;
}

NTSTATUS RShareVirtualDisplayDevice::CreateOrUpdateMonitor(const RSHARE_VDISPLAY_REQUEST& request)
{
    if (request.Width == 0 || request.Height == 0 || request.RefreshRateMillihz == 0) {
        return STATUS_INVALID_PARAMETER;
    }

    m_State.Width = request.Width;
    m_State.Height = request.Height;
    m_State.RefreshRateMillihz = request.RefreshRateMillihz;
    m_MonitorRequested = true;
    if (m_Monitor != nullptr) {
        auto monitorContext = RShareGetMonitorContext(m_Monitor);
        if (monitorContext != nullptr && monitorContext->Monitor != nullptr) {
            monitorContext->Monitor->UpdateMode(m_State);
        }
    }

    if (m_Adapter == nullptr) {
        return STATUS_SUCCESS;
    }

    ReportMonitorArrival(0);
    return STATUS_SUCCESS;
}

NTSTATUS RShareVirtualDisplayDevice::CommitModes(const IDARG_IN_COMMITMODES* inArgs)
{
    if (inArgs == nullptr || (inArgs->PathCount > 0 && inArgs->pPaths == nullptr)) {
        return STATUS_INVALID_PARAMETER;
    }

    if (m_Monitor == nullptr) {
        return STATUS_SUCCESS;
    }

    for (UINT index = 0; index < inArgs->PathCount; index++) {
        const auto& path = inArgs->pPaths[index];
        if (path.MonitorObject != m_Monitor) {
            continue;
        }

        if ((path.Flags & IDDCX_PATH_FLAGS_ACTIVE) == 0) {
            m_State.Active = 0;
            return STATUS_SUCCESS;
        }

        const auto mode = RShareModeFromSignalInfo(path.TargetVideoSignalInfo);
        const DWORD refreshMillihz = RShareRefreshMillihzFromSignalInfo(path.TargetVideoSignalInfo);
        if (mode.Width == 0 || mode.Height == 0 || refreshMillihz == 0) {
            return STATUS_INVALID_PARAMETER;
        }

        m_State.Active = 1;
        m_State.Width = mode.Width;
        m_State.Height = mode.Height;
        m_State.RefreshRateMillihz = refreshMillihz;

        auto monitorContext = RShareGetMonitorContext(m_Monitor);
        if (monitorContext != nullptr && monitorContext->Monitor != nullptr) {
            monitorContext->Monitor->UpdateMode(m_State);
        }

        return STATUS_SUCCESS;
    }

    return STATUS_SUCCESS;
}

NTSTATUS RShareVirtualDisplayDevice::RemoveMonitor()
{
    if (m_Monitor != nullptr) {
        IddCxMonitorDeparture(m_Monitor);
        m_Monitor = nullptr;
    }

    m_MonitorRequested = false;
    m_State.Active = 0;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayAdapterInitFinished(IDDCX_ADAPTER adapterObject, const IDARG_IN_ADAPTER_INIT_FINISHED* inArgs)
{
    if (!NT_SUCCESS(inArgs->AdapterInitStatus)) {
        return STATUS_SUCCESS;
    }

    auto context = RShareGetDeviceContext(adapterObject);
    if (context == nullptr || context->Device == nullptr) {
        return STATUS_DEVICE_NOT_READY;
    }

    context->Device->ReportPendingMonitorArrival();
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayDeviceIoControl(
    WDFDEVICE device,
    WDFREQUEST request,
    size_t outputBufferLength,
    size_t inputBufferLength,
    ULONG ioControlCode)
{
    auto context = RShareGetDeviceContext(device);
    if (context == nullptr || context->Device == nullptr) {
        WdfRequestComplete(request, STATUS_DEVICE_NOT_READY);
        return STATUS_DEVICE_NOT_READY;
    }

    NTSTATUS status = STATUS_INVALID_DEVICE_REQUEST;
    size_t bytesReturned = 0;

    switch (ioControlCode) {
    case IOCTL_RSHARE_QUERY_VERSION: {
        UNREFERENCED_PARAMETER(inputBufferLength);
        PRSHARE_DRIVER_VERSION version = nullptr;
        status = WdfRequestRetrieveOutputBuffer(request, sizeof(RSHARE_DRIVER_VERSION), reinterpret_cast<PVOID*>(&version), nullptr);
        if (NT_SUCCESS(status)) {
            version->Major = 0;
            version->Minor = 1;
            version->Patch = 0;
            version->Abi = RSHARE_DRIVER_ABI;
            bytesReturned = sizeof(RSHARE_DRIVER_VERSION);
        }
        break;
    }
    case IOCTL_RSHARE_QUERY_CAPABILITIES: {
        UNREFERENCED_PARAMETER(inputBufferLength);
        PRSHARE_DRIVER_CAPABILITIES capabilities = nullptr;
        status = WdfRequestRetrieveOutputBuffer(request, sizeof(RSHARE_DRIVER_CAPABILITIES), reinterpret_cast<PVOID*>(&capabilities), nullptr);
        if (NT_SUCCESS(status)) {
            capabilities->Abi = RSHARE_DRIVER_ABI;
            capabilities->Flags = RSHARE_CAP_VIRTUAL_DISPLAY;
            capabilities->MaxEventSize = sizeof(RSHARE_VDISPLAY_STATE);
            capabilities->Reserved = 0;
            bytesReturned = sizeof(RSHARE_DRIVER_CAPABILITIES);
        }
        break;
    }
    case IOCTL_RSHARE_VDISPLAY_QUERY_STATE: {
        UNREFERENCED_PARAMETER(inputBufferLength);
        PRSHARE_VDISPLAY_STATE state = nullptr;
        status = WdfRequestRetrieveOutputBuffer(request, sizeof(RSHARE_VDISPLAY_STATE), reinterpret_cast<PVOID*>(&state), nullptr);
        if (NT_SUCCESS(status)) {
            status = context->Device->QueryState(state, outputBufferLength);
            if (NT_SUCCESS(status)) {
                bytesReturned = sizeof(RSHARE_VDISPLAY_STATE);
            }
        }
        break;
    }
    case IOCTL_RSHARE_VDISPLAY_CREATE: {
        UNREFERENCED_PARAMETER(outputBufferLength);
        PRSHARE_VDISPLAY_REQUEST createRequest = nullptr;
        status = WdfRequestRetrieveInputBuffer(request, sizeof(RSHARE_VDISPLAY_REQUEST), reinterpret_cast<PVOID*>(&createRequest), nullptr);
        if (NT_SUCCESS(status)) {
            status = context->Device->CreateOrUpdateMonitor(*createRequest);
        }
        break;
    }
    case IOCTL_RSHARE_VDISPLAY_REMOVE: {
        UNREFERENCED_PARAMETER(outputBufferLength);
        UNREFERENCED_PARAMETER(inputBufferLength);
        status = context->Device->RemoveMonitor();
        break;
    }
    default:
        status = STATUS_INVALID_DEVICE_REQUEST;
        break;
    }

    if (bytesReturned > 0) {
        WdfRequestCompleteWithInformation(request, status, bytesReturned);
    } else {
        WdfRequestComplete(request, status);
    }
    return status;
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayAdapterCommitModes(IDDCX_ADAPTER adapterObject, const IDARG_IN_COMMITMODES* inArgs)
{
    auto context = RShareGetDeviceContext(adapterObject);
    if (context == nullptr || context->Device == nullptr) {
        return STATUS_DEVICE_NOT_READY;
    }

    return context->Device->CommitModes(inArgs);
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayParseMonitorDescription(
    const IDARG_IN_PARSEMONITORDESCRIPTION* inArgs,
    IDARG_OUT_PARSEMONITORDESCRIPTION* outArgs)
{
    outArgs->MonitorModeBufferOutputCount = ARRAYSIZE(RShareMonitorModes);

    if (inArgs->MonitorModeBufferInputCount == 0) {
        return STATUS_SUCCESS;
    }

    if (inArgs->MonitorModeBufferInputCount < ARRAYSIZE(RShareMonitorModes)) {
        return STATUS_BUFFER_TOO_SMALL;
    }

    for (DWORD index = 0; index < ARRAYSIZE(RShareMonitorModes); index++) {
        inArgs->pMonitorModes[index] = RShareCreateMonitorMode(RShareMonitorModes[index]);
    }

    outArgs->PreferredMonitorModeIdx = 0;
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayMonitorGetDefaultModes(
    IDDCX_MONITOR monitorObject,
    const IDARG_IN_GETDEFAULTDESCRIPTIONMODES* inArgs,
    IDARG_OUT_GETDEFAULTDESCRIPTIONMODES* outArgs)
{
    auto context = RShareGetMonitorContext(monitorObject);
    if (context == nullptr || context->Monitor == nullptr) {
        return STATUS_DEVICE_NOT_READY;
    }

    return context->Monitor->CopyDefaultModes(inArgs, outArgs);
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayMonitorQueryModes(
    IDDCX_MONITOR monitorObject,
    const IDARG_IN_QUERYTARGETMODES* inArgs,
    IDARG_OUT_QUERYTARGETMODES* outArgs)
{
    auto context = RShareGetMonitorContext(monitorObject);
    if (context == nullptr || context->Monitor == nullptr) {
        return STATUS_DEVICE_NOT_READY;
    }

    return context->Monitor->CopyTargetModes(inArgs, outArgs);
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayMonitorAssignSwapChain(IDDCX_MONITOR monitorObject, const IDARG_IN_SETSWAPCHAIN* inArgs)
{
    auto context = RShareGetMonitorContext(monitorObject);
    if (context == nullptr || context->Monitor == nullptr) {
        return STATUS_DEVICE_NOT_READY;
    }

    context->Monitor->AssignSwapChain(inArgs->hSwapChain, inArgs->RenderAdapterLuid, inArgs->hNextSurfaceAvailable);
    return STATUS_SUCCESS;
}

_Use_decl_annotations_
NTSTATUS RShareVDisplayMonitorUnassignSwapChain(IDDCX_MONITOR monitorObject)
{
    auto context = RShareGetMonitorContext(monitorObject);
    if (context != nullptr && context->Monitor != nullptr) {
        context->Monitor->UnassignSwapChain();
    }
    return STATUS_SUCCESS;
}
