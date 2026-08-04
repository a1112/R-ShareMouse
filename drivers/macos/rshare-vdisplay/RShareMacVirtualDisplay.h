#pragma once

#include <IOKit/IOBufferMemoryDescriptor.h>
#include <IOKit/IODeviceMemory.h>
#include <IOKit/IOUserClient.h>
#include <IOKit/graphics/IOFramebuffer.h>
#include <libkern/c++/OSArray.h>

#include "rshare_vdisplay_shared.h"

struct RShareMacVdisplayMode {
    uint32_t width;
    uint32_t height;
    uint32_t refresh_rate_millihz;
    IODisplayModeID mode_id;
};

class RShareMacVirtualDisplay;

class RShareMacVirtualDisplayUserClient : public IOUserClient {
    OSDeclareDefaultStructors(RShareMacVirtualDisplayUserClient);

public:
    bool initWithTask(task_t owningTask, void* securityToken, UInt32 type, OSDictionary* properties) override;
    bool start(IOService* provider) override;
    void stop(IOService* provider) override;
    IOReturn clientClose() override;
    IOReturn externalMethod(uint32_t selector, IOExternalMethodArguments* arguments, IOExternalMethodDispatch* dispatch, OSObject* target, void* reference) override;

private:
    RShareMacVirtualDisplay* m_owner;

    static IOReturn QueryVersion(OSObject* target, void* reference, IOExternalMethodArguments* arguments);
    static IOReturn QueryCapabilities(OSObject* target, void* reference, IOExternalMethodArguments* arguments);
    static IOReturn QueryState(OSObject* target, void* reference, IOExternalMethodArguments* arguments);
    static IOReturn CreateOrUpdate(OSObject* target, void* reference, IOExternalMethodArguments* arguments);
    static IOReturn Remove(OSObject* target, void* reference, IOExternalMethodArguments* arguments);
};

class RShareMacVirtualDisplay : public IOFramebuffer {
    OSDeclareDefaultStructors(RShareMacVirtualDisplay);

public:
    bool init(OSDictionary* dictionary = nullptr) override;
    bool start(IOService* provider) override;
    void stop(IOService* provider) override;
    void free() override;

    IOReturn newUserClient(task_t owningTask, void* securityID, UInt32 type, IOUserClient** clientH) override;

    IOReturn enableController() override;
    IODeviceMemory* getApertureRange(IOPixelAperture aperture) override;
    IODeviceMemory* getVRAMRange() override;
    const char* getPixelFormats() override;
    IOItemCount getDisplayModeCount() override;
    IOReturn getDisplayModes(IODisplayModeID* allDisplayModes) override;
    IOReturn getInformationForDisplayMode(IODisplayModeID displayMode, IODisplayModeInformation* info) override;
    UInt64 getPixelFormatsForDisplayMode(IODisplayModeID displayMode, IOIndex depth) override;
    IOReturn getPixelInformation(IODisplayModeID displayMode, IOIndex depth, IOPixelAperture aperture, IOPixelInformation* pixelInfo) override;
    IOReturn getCurrentDisplayMode(IODisplayModeID* displayMode, IOIndex* depth) override;
    IOReturn setDisplayMode(IODisplayModeID displayMode, IOIndex depth) override;
    IOReturn setStartupDisplayMode(IODisplayModeID displayMode, IOIndex depth) override;
    IOReturn getStartupDisplayMode(IODisplayModeID* displayMode, IOIndex* depth) override;
    IOReturn getTimingInfoForDisplayMode(IODisplayModeID displayMode, IOTimingInformation* info) override;
    IOItemCount getConnectionCount() override;
    IOReturn connectFlags(IOIndex connectIndex, IODisplayModeID displayMode, IOOptionBits* flags) override;
    IOReturn setAttributeForConnection(IOIndex connectIndex, IOSelect attribute, uintptr_t value) override;
    IOReturn getAttributeForConnection(IOIndex connectIndex, IOSelect attribute, uintptr_t* value) override;
    bool hasDDCConnect(IOIndex connectIndex) override;
    IOReturn getDDCBlock(IOIndex connectIndex, UInt32 blockNumber, IOSelect blockType, IOOptionBits options, UInt8* data, IOByteCount* length) override;
    IOReturn registerForInterruptType(IOSelect interruptType, IOFBInterruptProc proc, OSObject* target, void* ref, void** interruptRef) override;
    IOReturn unregisterInterrupt(void* interruptRef) override;
    IOReturn setInterruptState(void* interruptRef, UInt32 state) override;

    IOReturn copyVersion(RShareDriverVersion* version) const;
    IOReturn copyCapabilities(RShareDriverCapabilities* capabilities) const;
    IOReturn copyState(RShareVdisplayState* state) const;
    IOReturn createOrUpdate(const RShareVdisplayRequest* request);
    IOReturn removeVirtualDisplay();

private:
    bool ensureBackingStoreForMode(const RShareMacVdisplayMode* mode);
    void releaseBackingStore();
    void notifyConnectionChange();
    void dispatchPendingConnectInterrupt();

    static const RShareMacVdisplayMode* modeForId(IODisplayModeID modeId);
    static const RShareMacVdisplayMode* modeForRequest(const RShareVdisplayRequest* request);

    IOBufferMemoryDescriptor* m_framebuffer;
    IODeviceMemory* m_vramRange;
    OSArray* m_retiredFramebuffers;
    IOLock* m_lock;
    IODisplayModeID m_currentMode;
    IOIndex m_currentDepth;
    IODisplayModeID m_requestedMode;
    IOIndex m_requestedDepth;
    IODisplayModeID m_startupMode;
    IOIndex m_startupDepth;
    RShareVdisplayState m_state;
    IOFBInterruptProc m_connectInterruptProc;
    OSObject* m_connectInterruptTarget;
    void* m_connectInterruptRef;
    bool m_connectInterruptEnabled;
    bool m_connectInterruptPending;
    bool m_connectionDetected;
    bool m_stopping;
};
