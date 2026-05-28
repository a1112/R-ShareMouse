#pragma once

#define NOMINMAX

#include <windows.h>
#include <bugcodes.h>
#include <wudfwdm.h>
#include <wdf.h>
#include <iddcx.h>

#include <avrt.h>
#include <d3d11_2.h>
#include <dxgi1_5.h>
#include <wrl.h>

#include <memory>
#include <vector>

#include "..\rshare-common\rshare_ioctls.h"
#include "trace.h"

// {8c1fd719-6fb8-4f82-a4d2-07c6fd490875}
EXTERN_C const GUID GUID_DEVINTERFACE_RSHARE_VDISPLAY;

namespace Microsoft::WRL::Wrappers
{
    using Thread = HandleT<HandleTraits::HANDLENullTraits>;
}

namespace RShare::VirtualDisplay
{
    struct RShareDisplayMode
    {
        DWORD Width;
        DWORD Height;
        DWORD RefreshRateMillihz;
    };

    class RShareDirect3DDevice
    {
    public:
        explicit RShareDirect3DDevice(LUID adapterLuid);
        HRESULT Init();

        Microsoft::WRL::ComPtr<ID3D11Device> Device;

    private:
        LUID m_AdapterLuid;
        Microsoft::WRL::ComPtr<IDXGIFactory5> m_DxgiFactory;
        Microsoft::WRL::ComPtr<IDXGIAdapter1> m_Adapter;
        Microsoft::WRL::ComPtr<ID3D11DeviceContext> m_DeviceContext;
    };

    class RShareSwapChainProcessor
    {
    public:
        RShareSwapChainProcessor(IDDCX_SWAPCHAIN swapChain, std::shared_ptr<RShareDirect3DDevice> device, HANDLE newFrameEvent);
        ~RShareSwapChainProcessor();

    private:
        static DWORD CALLBACK RunThread(LPVOID argument);
        void Run();
        void RunCore();

        IDDCX_SWAPCHAIN m_SwapChain;
        std::shared_ptr<RShareDirect3DDevice> m_Device;
        HANDLE m_NewFrameEvent;
        Microsoft::WRL::Wrappers::Thread m_Thread;
        Microsoft::WRL::Wrappers::Event m_TerminateEvent;
    };

    class RShareVirtualDisplayMonitor
    {
    public:
        explicit RShareVirtualDisplayMonitor(IDDCX_MONITOR monitor);
        ~RShareVirtualDisplayMonitor();

        void UpdateMode(const RSHARE_VDISPLAY_STATE& state);
        NTSTATUS UpdateTargetModes();
        NTSTATUS CopyDefaultModes(const IDARG_IN_GETDEFAULTDESCRIPTIONMODES* inArgs, IDARG_OUT_GETDEFAULTDESCRIPTIONMODES* outArgs) const;
        NTSTATUS CopyTargetModes(const IDARG_IN_QUERYTARGETMODES* inArgs, IDARG_OUT_QUERYTARGETMODES* outArgs) const;
        void AssignSwapChain(IDDCX_SWAPCHAIN swapChain, LUID renderAdapter, HANDLE newFrameEvent);
        void UnassignSwapChain();

    private:
        IDDCX_MONITOR m_Monitor;
        RSHARE_VDISPLAY_STATE m_State;
        std::unique_ptr<RShareSwapChainProcessor> m_Processor;
    };

    class RShareVirtualDisplayDevice
    {
    public:
        explicit RShareVirtualDisplayDevice(WDFDEVICE device);
        void InitAdapter();
        NTSTATUS QueryState(PRSHARE_VDISPLAY_STATE state, size_t outputSize);
        NTSTATUS CreateOrUpdateMonitor(const RSHARE_VDISPLAY_REQUEST& request);
        NTSTATUS CommitModes(const IDARG_IN_COMMITMODES* inArgs);
        NTSTATUS RemoveMonitor();
        void ReportMonitorArrival(UINT connectorIndex);
        void ReportPendingMonitorArrival();

    private:
        WDFDEVICE m_Device;
        IDDCX_ADAPTER m_Adapter;
        IDDCX_MONITOR m_Monitor;
        RSHARE_VDISPLAY_STATE m_State;
        bool m_MonitorRequested;
    };
}
