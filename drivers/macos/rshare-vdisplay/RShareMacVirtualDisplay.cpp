#include "RShareMacVirtualDisplay.h"

#include <IOKit/IOLib.h>
#include <IOKit/IOTypes.h>
#include <libkern/c++/OSDictionary.h>
#include <libkern/c++/OSString.h>
#include <libkern/OSByteOrder.h>
#include <libkern/libkern.h>
#include <mach/kmod.h>

extern "C" {

static kern_return_t RShareMacVdisplayModuleStart(kmod_info_t*, void*)
{
    return KERN_SUCCESS;
}

static kern_return_t RShareMacVdisplayModuleStop(kmod_info_t*, void*)
{
    return KERN_SUCCESS;
}

kmod_info_t kmod_info = {
    nullptr,
    KMOD_INFO_VERSION,
    -1U,
    "io.rshare.mouse.vdisplay",
    "0.1.0",
    -1,
    nullptr,
    0,
    0,
    0,
    RShareMacVdisplayModuleStart,
    RShareMacVdisplayModuleStop,
};

}

OSDefineMetaClassAndStructors(RShareMacVirtualDisplay, IOFramebuffer);
OSDefineMetaClassAndStructors(RShareMacVirtualDisplayUserClient, IOUserClient);

namespace {

constexpr uint32_t kRShareMacVdisplayDefaultModeIndex = 0;
constexpr uint32_t kRShareMacVdisplayBytesPerPixel = 4;
constexpr uint32_t kRShareMacVdisplayVendorId = 0x4a6d;
constexpr uint32_t kRShareMacVdisplayProductId = 0x0001;
constexpr uint32_t kRShareMacVdisplaySerialNumber = 0x00000001;
constexpr uint16_t kRShareMacVdisplayImageWidthMillimeters = 520;
constexpr uint16_t kRShareMacVdisplayImageHeightMillimeters = 290;
constexpr const char* kRShareMacVdisplayFriendlyName = "R-SHAREMOUSE";
constexpr const char* kRShareMacVdisplaySerial = "RSM00000001";

static_assert(RSHARE_MACOS_SELECTOR_QUERY_VERSION == 0, "selector ABI drift");
static_assert(RSHARE_MACOS_SELECTOR_QUERY_CAPABILITIES == 1, "selector ABI drift");
static_assert(RSHARE_MACOS_SELECTOR_VDISPLAY_QUERY_STATE == 2, "selector ABI drift");
static_assert(RSHARE_MACOS_SELECTOR_VDISPLAY_CREATE == 3, "selector ABI drift");
static_assert(RSHARE_MACOS_SELECTOR_VDISPLAY_REMOVE == 4, "selector ABI drift");
static_assert(RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE != kIOFBServerConnectType, "control user client type collides with WindowServer");
static_assert(RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE != kIOFBSharedConnectType, "control user client type collides with shared framebuffer clients");

constexpr RShareMacVdisplayMode kRShareMacVdisplayModes[] = {
    {1920, 1080, 60000, 1},
    {1920, 1080, 144000, 2},
    {1920, 1080, 90000, 3},
    {2560, 1440, 144000, 4},
    {2560, 1440, 90000, 5},
    {2560, 1440, 60000, 6},
    {3840, 2160, 60000, 7},
    {1600, 900, 60000, 8},
    {1280, 720, 90000, 9},
    {1280, 720, 60000, 10},
    {1024, 768, 75000, 11},
    {1024, 768, 60000, 12},
};

constexpr uint8_t kRShareMacVdisplayEdid[128] = {
    0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
    0x4a, 0x6d, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    0x01, 0x24, 0x01, 0x04, 0xa5, 0x34, 0x1d, 0x78,
    0x22, 0xee, 0x95, 0xa3, 0x54, 0x4c, 0x99, 0x26,
    0x0f, 0x50, 0x54, 0x00, 0x00, 0x00, 0x81, 0x80,
    0x90, 0x40, 0x95, 0x00, 0xa9, 0x40, 0xb3, 0x00,
    0xd1, 0xc0, 0x01, 0x01, 0x01, 0x01, 0x02, 0x3a,
    0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c,
    0x45, 0x00, 0x55, 0x50, 0x21, 0x00, 0x00, 0x1e,
    0x00, 0x00, 0x00, 0xfc, 0x00, 0x52, 0x2d, 0x53,
    0x48, 0x41, 0x52, 0x45, 0x4d, 0x4f, 0x55, 0x53,
    0x45, 0x0a, 0x00, 0x00, 0x00, 0xff, 0x00, 0x52,
    0x53, 0x4d, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30,
    0x30, 0x31, 0x0a, 0x20, 0x00, 0x00, 0x00, 0xfd,
    0x00, 0x3c, 0x90, 0x1e, 0xdc, 0x46, 0x00, 0x0a,
    0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x3d,
};

constexpr uint32_t modeCount()
{
    return static_cast<uint32_t>(sizeof(kRShareMacVdisplayModes) / sizeof(kRShareMacVdisplayModes[0]));
}

IOFixed1616 refreshToFixed1616(uint32_t refreshRateMillihz)
{
    return static_cast<IOFixed1616>((static_cast<uint64_t>(refreshRateMillihz) << 16) / 1000u);
}

uint32_t rowBytesForMode(const RShareMacVdisplayMode* mode)
{
    return mode->width * kRShareMacVdisplayBytesPerPixel;
}

uint32_t backingSizeForMode(const RShareMacVdisplayMode* mode)
{
    return rowBytesForMode(mode) * mode->height;
}

uint32_t horizontalBlankingForMode(const RShareMacVdisplayMode* mode)
{
    const uint32_t proportionalBlanking = mode->width / 5u;
    return proportionalBlanking >= 160u ? proportionalBlanking : 160u;
}

uint32_t verticalBlankingForMode(const RShareMacVdisplayMode*)
{
    return 45u;
}

uint64_t pixelClockForMode(const RShareMacVdisplayMode* mode)
{
    const uint64_t horizontalTotal = static_cast<uint64_t>(mode->width) + horizontalBlankingForMode(mode);
    const uint64_t verticalTotal = static_cast<uint64_t>(mode->height) + verticalBlankingForMode(mode);
    return horizontalTotal * verticalTotal * mode->refresh_rate_millihz / 1000u;
}

void fillDetailedTiming(const RShareMacVdisplayMode* mode, IODetailedTimingInformationV2* timing)
{
    bzero(timing, sizeof(*timing));
    timing->pixelClock = pixelClockForMode(mode);
    timing->minPixelClock = timing->pixelClock;
    timing->maxPixelClock = timing->pixelClock;
    timing->horizontalActive = mode->width;
    timing->horizontalBlanking = horizontalBlankingForMode(mode);
    timing->horizontalSyncOffset = timing->horizontalBlanking / 4u;
    timing->horizontalSyncPulseWidth = timing->horizontalBlanking / 8u;
    timing->verticalActive = mode->height;
    timing->verticalBlanking = verticalBlankingForMode(mode);
    timing->verticalSyncOffset = 3u;
    timing->verticalSyncPulseWidth = 5u;
    timing->numLinks = 1u;
    timing->bitsPerColorComponent = 8u;
}

bool publishDisplayProductName(IORegistryEntry* entry)
{
    auto productNames = OSDictionary::withCapacity(1);
    if (productNames == nullptr) {
        return false;
    }

    auto productName = OSString::withCString(kRShareMacVdisplayFriendlyName);
    if (productName == nullptr) {
        productNames->release();
        return false;
    }

    const bool inserted = productNames->setObject("en", productName);
    productName->release();
    if (!inserted) {
        productNames->release();
        return false;
    }

    const bool published = entry->setProperty(kDisplayProductName, productNames);
    productNames->release();
    return published;
}

} // namespace

bool RShareMacVirtualDisplay::init(OSDictionary* dictionary)
{
    if (!IOFramebuffer::init(dictionary)) {
        return false;
    }

    m_framebuffer = nullptr;
    m_vramRange = nullptr;
    m_retiredFramebuffers = OSArray::withCapacity(modeCount());
    m_lock = IOLockAlloc();
    m_currentMode = kRShareMacVdisplayModes[kRShareMacVdisplayDefaultModeIndex].mode_id;
    m_currentDepth = 0;
    m_requestedMode = m_currentMode;
    m_requestedDepth = m_currentDepth;
    m_startupMode = m_currentMode;
    m_startupDepth = m_currentDepth;
    m_state = {
        RSHARE_DRIVER_ABI,
        RSHARE_VDISPLAY_ACTIVITY_REMOVED,
        kRShareMacVdisplayModes[kRShareMacVdisplayDefaultModeIndex].width,
        kRShareMacVdisplayModes[kRShareMacVdisplayDefaultModeIndex].height,
        kRShareMacVdisplayModes[kRShareMacVdisplayDefaultModeIndex].refresh_rate_millihz,
        0,
    };
    m_connectInterruptProc = nullptr;
    m_connectInterruptTarget = nullptr;
    m_connectInterruptRef = nullptr;
    m_connectInterruptEnabled = false;
    m_connectInterruptPending = false;
    m_connectionDetected = false;
    m_stopping = false;

    return m_lock != nullptr && m_retiredFramebuffers != nullptr;
}

bool RShareMacVirtualDisplay::start(IOService* provider)
{
    setName(RSHARE_MACOS_VDISPLAY_SERVICE_CLASS);
    if (!setProperty("IOProviderClass", "IOResources")
        || !setProperty("RShareVirtualDisplay", true)
        || !setProperty(kIODisplayEDIDKey, const_cast<uint8_t*>(kRShareMacVdisplayEdid), sizeof(kRShareMacVdisplayEdid))
        || !setProperty(kIODisplayEDIDOriginalKey, const_cast<uint8_t*>(kRShareMacVdisplayEdid), sizeof(kRShareMacVdisplayEdid))
        || !setProperty(kDisplayVendorID, kRShareMacVdisplayVendorId, 32)
        || !setProperty(kDisplayProductID, kRShareMacVdisplayProductId, 32)
        || !setProperty(kDisplaySerialNumber, kRShareMacVdisplaySerialNumber, 32)
        || !setProperty(kDisplaySerialString, kRShareMacVdisplaySerial)
        || !publishDisplayProductName(this)) {
        return false;
    }

    return IOFramebuffer::start(provider);
}

void RShareMacVirtualDisplay::stop(IOService* provider)
{
    if (m_lock != nullptr) {
        IOLockLock(m_lock);
        m_stopping = true;
        m_state.active = RSHARE_VDISPLAY_ACTIVITY_REMOVED;
        m_connectInterruptProc = nullptr;
        m_connectInterruptTarget = nullptr;
        m_connectInterruptRef = nullptr;
        m_connectInterruptEnabled = false;
        m_connectInterruptPending = false;
        m_connectionDetected = false;
        IOLockUnlock(m_lock);
    }

    IOFramebuffer::stop(provider);
    OSSafeReleaseNULL(fVramMap);
    fFrameBuffer = nullptr;
    fVramMapOffset = 0;
    releaseBackingStore();
}

void RShareMacVirtualDisplay::free()
{
    releaseBackingStore();
    if (m_lock != nullptr) {
        IOLockFree(m_lock);
        m_lock = nullptr;
    }
    IOFramebuffer::free();
}

IOReturn RShareMacVirtualDisplay::newUserClient(task_t owningTask, void* securityID, UInt32 type, IOUserClient** clientH)
{
    if (clientH == nullptr) {
        return kIOReturnBadArgument;
    }
    *clientH = nullptr;

    if (type != RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE) {
        return IOFramebuffer::newUserClient(owningTask, securityID, type, clientH);
    }

    auto* client = OSTypeAlloc(RShareMacVirtualDisplayUserClient);
    if (client == nullptr) {
        return kIOReturnNoMemory;
    }

    if (!client->initWithTask(owningTask, securityID, type, nullptr)) {
        client->release();
        return kIOReturnError;
    }
    if (!client->attach(this)) {
        client->release();
        return kIOReturnError;
    }
    if (!client->start(this)) {
        client->detach(this);
        client->release();
        return kIOReturnError;
    }

    *clientH = client;
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::enableController()
{
    IOLockLock(m_lock);
    const bool connected = m_connectionDetected;
    const IODisplayModeID requestedMode = m_requestedMode;
    IOLockUnlock(m_lock);

    // IOGraphics does not map an aperture while this framebuffer is offline.
    // Defer the discouraged contiguous allocation until the first create request.
    if (!connected) {
        return kIOReturnSuccess;
    }

    const auto* mode = modeForId(requestedMode);
    if (mode == nullptr) {
        return kIOReturnUnsupportedMode;
    }
    return ensureBackingStoreForMode(mode) ? kIOReturnSuccess : kIOReturnNoMemory;
}

IODeviceMemory* RShareMacVirtualDisplay::getApertureRange(IOPixelAperture aperture)
{
    if (aperture != kIOFBSystemAperture) {
        return nullptr;
    }

    IODeviceMemory* range = nullptr;
    IOLockLock(m_lock);
    if (m_vramRange != nullptr) {
        range = m_vramRange;
        range->retain();
    }
    IOLockUnlock(m_lock);
    return range;
}

IODeviceMemory* RShareMacVirtualDisplay::getVRAMRange()
{
    IODeviceMemory* range = nullptr;
    IOLockLock(m_lock);
    if (m_vramRange != nullptr) {
        range = m_vramRange;
        range->retain();
    }
    IOLockUnlock(m_lock);
    return range;
}

const char* RShareMacVirtualDisplay::getPixelFormats()
{
    return IO32BitDirectPixels "\0";
}

IOItemCount RShareMacVirtualDisplay::getDisplayModeCount()
{
    return modeCount();
}

IOReturn RShareMacVirtualDisplay::getDisplayModes(IODisplayModeID* allDisplayModes)
{
    if (allDisplayModes == nullptr) {
        return kIOReturnBadArgument;
    }
    for (uint32_t index = 0; index < modeCount(); ++index) {
        allDisplayModes[index] = kRShareMacVdisplayModes[index].mode_id;
    }
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::getInformationForDisplayMode(IODisplayModeID displayMode, IODisplayModeInformation* info)
{
    const auto* mode = modeForId(displayMode);
    if (mode == nullptr || info == nullptr) {
        return kIOReturnUnsupportedMode;
    }

    IOLockLock(m_lock);
    const bool isRequestedMode = displayMode == m_requestedMode;
    IOLockUnlock(m_lock);

    bzero(info, sizeof(*info));
    info->nominalWidth = mode->width;
    info->nominalHeight = mode->height;
    info->refreshRate = refreshToFixed1616(mode->refresh_rate_millihz);
    info->maxDepthIndex = 0;
    info->flags = kDisplayModeValidFlag | kDisplayModeSafeFlag | kDisplayModeAlwaysShowFlag;
    if (isRequestedMode) {
        info->flags |= kDisplayModeDefaultFlag;
    }
    info->imageWidth = kRShareMacVdisplayImageWidthMillimeters;
    info->imageHeight = kRShareMacVdisplayImageHeightMillimeters;
    return kIOReturnSuccess;
}

UInt64 RShareMacVirtualDisplay::getPixelFormatsForDisplayMode(IODisplayModeID displayMode, IOIndex depth)
{
    if (modeForId(displayMode) == nullptr || depth != 0) {
        return 0;
    }
    return 0;
}

IOReturn RShareMacVirtualDisplay::getPixelInformation(IODisplayModeID displayMode, IOIndex depth, IOPixelAperture aperture, IOPixelInformation* pixelInfo)
{
    const auto* mode = modeForId(displayMode);
    if (mode == nullptr || pixelInfo == nullptr || depth != 0 || aperture != kIOFBSystemAperture) {
        return kIOReturnUnsupportedMode;
    }

    bzero(pixelInfo, sizeof(*pixelInfo));
    pixelInfo->bytesPerRow = rowBytesForMode(mode);
    pixelInfo->bytesPerPlane = 0;
    pixelInfo->bitsPerPixel = 32;
    pixelInfo->pixelType = kIORGBDirectPixels;
    pixelInfo->componentCount = 3;
    pixelInfo->bitsPerComponent = 8;
    pixelInfo->componentMasks[0] = 0x00ff0000;
    pixelInfo->componentMasks[1] = 0x0000ff00;
    pixelInfo->componentMasks[2] = 0x000000ff;
    strlcpy(pixelInfo->pixelFormat, IO32BitDirectPixels, sizeof(pixelInfo->pixelFormat));
    pixelInfo->activeWidth = mode->width;
    pixelInfo->activeHeight = mode->height;
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::getCurrentDisplayMode(IODisplayModeID* displayMode, IOIndex* depth)
{
    if (displayMode == nullptr || depth == nullptr) {
        return kIOReturnBadArgument;
    }
    IOLockLock(m_lock);
    *displayMode = m_currentMode;
    *depth = m_currentDepth;
    IOLockUnlock(m_lock);
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::setDisplayMode(IODisplayModeID displayMode, IOIndex depth)
{
    const auto* mode = modeForId(displayMode);
    if (mode == nullptr || depth != 0) {
        return kIOReturnUnsupportedMode;
    }

    if (!ensureBackingStoreForMode(mode)) {
        return kIOReturnNoMemory;
    }

    IOLockLock(m_lock);
    if (m_stopping) {
        IOLockUnlock(m_lock);
        return kIOReturnOffline;
    }
    m_currentMode = displayMode;
    m_currentDepth = depth;
    const bool commitsRequestedMode = m_state.active == RSHARE_VDISPLAY_ACTIVITY_PENDING
        && displayMode == m_requestedMode
        && depth == m_requestedDepth;
    if (m_state.active != RSHARE_VDISPLAY_ACTIVITY_PENDING || commitsRequestedMode) {
        m_requestedMode = displayMode;
        m_requestedDepth = depth;
        m_state.width = mode->width;
        m_state.height = mode->height;
        m_state.refresh_rate_millihz = mode->refresh_rate_millihz;
        if (commitsRequestedMode) {
            m_state.active = RSHARE_VDISPLAY_ACTIVITY_ACTIVE;
        }
    }
    IOLockUnlock(m_lock);

    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::setStartupDisplayMode(IODisplayModeID displayMode, IOIndex depth)
{
    if (modeForId(displayMode) == nullptr || depth != 0) {
        return kIOReturnUnsupportedMode;
    }

    IOLockLock(m_lock);
    m_startupMode = displayMode;
    m_startupDepth = depth;
    IOLockUnlock(m_lock);
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::getStartupDisplayMode(IODisplayModeID* displayMode, IOIndex* depth)
{
    if (displayMode == nullptr || depth == nullptr) {
        return kIOReturnBadArgument;
    }

    IOLockLock(m_lock);
    *displayMode = m_startupMode;
    *depth = m_startupDepth;
    IOLockUnlock(m_lock);
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::getTimingInfoForDisplayMode(IODisplayModeID displayMode, IOTimingInformation* info)
{
    const auto* mode = modeForId(displayMode);
    if (mode == nullptr || info == nullptr) {
        return kIOReturnUnsupportedMode;
    }

    bzero(info, sizeof(*info));
    info->appleTimingID = 0;
    info->flags = kIODetailedTimingValid;
    fillDetailedTiming(mode, &info->detailedInfo.v2);
    return kIOReturnSuccess;
}

IOItemCount RShareMacVirtualDisplay::getConnectionCount()
{
    return 1;
}

IOReturn RShareMacVirtualDisplay::connectFlags(IOIndex connectIndex, IODisplayModeID displayMode, IOOptionBits* flags)
{
    if (connectIndex != 0 || modeForId(displayMode) == nullptr || flags == nullptr) {
        return kIOReturnUnsupportedMode;
    }
    IOLockLock(m_lock);
    const bool isRequestedMode = displayMode == m_requestedMode;
    IOLockUnlock(m_lock);

    *flags = kDisplayModeValidFlag | kDisplayModeSafeFlag;
    if (isRequestedMode) {
        *flags |= kDisplayModeDefaultFlag;
    }
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::setAttributeForConnection(IOIndex connectIndex, IOSelect attribute, uintptr_t value)
{
    if (connectIndex != 0) {
        return kIOReturnBadArgument;
    }

    switch (attribute) {
    case kConnectionEnable:
    case kConnectionCheckEnable:
        return kIOReturnSuccess;

    case kConnectionProbe:
        notifyConnectionChange();
        return kIOReturnSuccess;

    case kConnectionChanged:
    case kConnectionPower:
        return kIOReturnSuccess;

    default:
        return IOFramebuffer::setAttributeForConnection(connectIndex, attribute, value);
    }
}

IOReturn RShareMacVirtualDisplay::getAttributeForConnection(IOIndex connectIndex, IOSelect attribute, uintptr_t* value)
{
    if (connectIndex != 0) {
        return kIOReturnBadArgument;
    }
    if (attribute == kConnectionSupportsHLDDCSense) {
        return kIOReturnSuccess;
    }
    if (attribute == kConnectionChanged) {
        if (value != nullptr) {
            *value = 0;
        }
        return kIOReturnSuccess;
    }
    if (value == nullptr) {
        return kIOReturnBadArgument;
    }

    switch (attribute) {
    case kConnectionFlags:
        *value = 0;
        return kIOReturnSuccess;

    case kConnectionEnable:
        IOLockLock(m_lock);
        *value = m_connectionDetected ? 1 : 0;
        IOLockUnlock(m_lock);
        return kIOReturnSuccess;

    case kConnectionCheckEnable:
        IOLockLock(m_lock);
        *value = m_connectionDetected ? 1 : 0;
        if (m_connectionDetected
            && m_currentMode == m_requestedMode
            && m_currentDepth == m_requestedDepth
            && m_state.active == RSHARE_VDISPLAY_ACTIVITY_PENDING) {
            m_state.active = RSHARE_VDISPLAY_ACTIVITY_ACTIVE;
        }
        IOLockUnlock(m_lock);
        return kIOReturnSuccess;

    case kConnectionProbe:
        IOLockLock(m_lock);
        *value = m_connectionDetected ? 1 : 0;
        IOLockUnlock(m_lock);
        return kIOReturnSuccess;

    case kConnectionPower:
        *value = 1;
        return kIOReturnSuccess;

    case kConnectionPostWake:
        *value = 0;
        return kIOReturnSuccess;

    default:
        return IOFramebuffer::getAttributeForConnection(connectIndex, attribute, value);
    }
}

bool RShareMacVirtualDisplay::hasDDCConnect(IOIndex connectIndex)
{
    if (connectIndex != 0) {
        return false;
    }

    IOLockLock(m_lock);
    const bool connected = m_connectionDetected;
    IOLockUnlock(m_lock);
    return connected;
}

IOReturn RShareMacVirtualDisplay::getDDCBlock(IOIndex connectIndex, UInt32 blockNumber, IOSelect blockType, IOOptionBits, UInt8* data, IOByteCount* length)
{
    if (connectIndex != 0 || blockNumber != 1 || blockType != kIODDCBlockTypeEDID || data == nullptr || length == nullptr) {
        return kIOReturnBadArgument;
    }
    if (!hasDDCConnect(connectIndex)) {
        return kIOReturnNoDevice;
    }
    if (*length < sizeof(kRShareMacVdisplayEdid)) {
        return kIOReturnNoSpace;
    }
    bcopy(kRShareMacVdisplayEdid, data, sizeof(kRShareMacVdisplayEdid));
    *length = sizeof(kRShareMacVdisplayEdid);
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::registerForInterruptType(IOSelect interruptType, IOFBInterruptProc proc, OSObject* target, void* ref, void** interruptRef)
{
    if (interruptType != kIOFBConnectInterruptType) {
        return kIOReturnUnsupported;
    }
    if (proc == nullptr || target == nullptr || interruptRef == nullptr) {
        return kIOReturnBadArgument;
    }

    IOLockLock(m_lock);
    if (m_connectInterruptProc != nullptr) {
        IOLockUnlock(m_lock);
        return kIOReturnBusy;
    }
    m_connectInterruptProc = proc;
    m_connectInterruptTarget = target;
    m_connectInterruptRef = ref;
    m_connectInterruptEnabled = true;
    *interruptRef = this;
    IOLockUnlock(m_lock);
    dispatchPendingConnectInterrupt();
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::unregisterInterrupt(void* interruptRef)
{
    if (interruptRef != this) {
        return kIOReturnBadArgument;
    }

    IOLockLock(m_lock);
    m_connectInterruptProc = nullptr;
    m_connectInterruptTarget = nullptr;
    m_connectInterruptRef = nullptr;
    m_connectInterruptEnabled = false;
    m_connectInterruptPending = false;
    IOLockUnlock(m_lock);
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::setInterruptState(void* interruptRef, UInt32 state)
{
    if (interruptRef != this) {
        return kIOReturnBadArgument;
    }
    IOLockLock(m_lock);
    m_connectInterruptEnabled = state == kEnabledInterruptState;
    const bool enabled = m_connectInterruptEnabled;
    IOLockUnlock(m_lock);
    if (enabled) {
        dispatchPendingConnectInterrupt();
    }
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::copyVersion(RShareDriverVersion* version) const
{
    if (version == nullptr) {
        return kIOReturnBadArgument;
    }
    *version = {0, 1, 0, RSHARE_DRIVER_ABI};
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::copyCapabilities(RShareDriverCapabilities* capabilities) const
{
    if (capabilities == nullptr) {
        return kIOReturnBadArgument;
    }
    *capabilities = {RSHARE_DRIVER_ABI, 0, RSHARE_CAP_VIRTUAL_DISPLAY, 0, 0};
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::copyState(RShareVdisplayState* state) const
{
    if (state == nullptr) {
        return kIOReturnBadArgument;
    }

    IOLockLock(m_lock);
    *state = m_state;
    IOLockUnlock(m_lock);
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::createOrUpdate(const RShareVdisplayRequest* request)
{
    const auto* mode = modeForRequest(request);
    if (mode == nullptr) {
        return kIOReturnUnsupportedMode;
    }
    if (!ensureBackingStoreForMode(mode)) {
        return kIOReturnNoMemory;
    }

    IOLockLock(m_lock);
    if (m_stopping) {
        IOLockUnlock(m_lock);
        return kIOReturnOffline;
    }
    m_requestedMode = mode->mode_id;
    m_requestedDepth = 0;
    m_state.abi = RSHARE_DRIVER_ABI;
    m_state.active = RSHARE_VDISPLAY_ACTIVITY_PENDING;
    m_state.width = mode->width;
    m_state.height = mode->height;
    m_state.refresh_rate_millihz = mode->refresh_rate_millihz;
    m_state.connector_index = 0;
    m_connectionDetected = true;
    IOLockUnlock(m_lock);

    notifyConnectionChange();
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplay::removeVirtualDisplay()
{
    IOLockLock(m_lock);
    if (m_stopping) {
        IOLockUnlock(m_lock);
        return kIOReturnOffline;
    }
    m_state.active = RSHARE_VDISPLAY_ACTIVITY_REMOVED;
    m_connectionDetected = false;
    IOLockUnlock(m_lock);

    notifyConnectionChange();
    return kIOReturnSuccess;
}

bool RShareMacVirtualDisplay::ensureBackingStoreForMode(const RShareMacVdisplayMode* mode)
{
    if (mode == nullptr) {
        return false;
    }
    const uint32_t bytes = backingSizeForMode(mode);

    IOLockLock(m_lock);
    const bool stopping = m_stopping;
    const bool hasExistingBackingStore = m_framebuffer != nullptr && m_vramRange != nullptr && m_framebuffer->getLength() >= bytes;
    IOLockUnlock(m_lock);
    if (stopping) {
        return false;
    }
    if (hasExistingBackingStore) {
        return true;
    }

    auto* framebuffer = IOBufferMemoryDescriptor::withOptions(
        kIODirectionInOut | kIOMemoryKernelUserShared | kIOMemoryPhysicallyContiguous | kIOMapWriteCombineCache,
        bytes,
        page_size);
    if (framebuffer == nullptr) {
        return false;
    }
    void* framebufferBytes = framebuffer->getBytesNoCopy();
    if (framebufferBytes == nullptr) {
        framebuffer->release();
        return false;
    }
    bzero(framebufferBytes, bytes);
    if (framebuffer->prepare(kIODirectionInOut) != kIOReturnSuccess) {
        framebuffer->release();
        return false;
    }

    IOByteCount segmentLength = 0;
    const IOPhysicalAddress physical = framebuffer->getPhysicalSegment(0, &segmentLength);
    if (physical == 0 || segmentLength < bytes) {
        framebuffer->complete(kIODirectionInOut);
        framebuffer->release();
        return false;
    }

    auto range = IODeviceMemory::withRange(physical, bytes);
    if (!range) {
        framebuffer->complete(kIODirectionInOut);
        framebuffer->release();
        return false;
    }

    IOBufferMemoryDescriptor* oldFramebuffer = nullptr;
    IODeviceMemory* oldRange = nullptr;
    bool installed = false;
    bool installRejected = false;

    IOLockLock(m_lock);
    if (m_stopping) {
        installRejected = true;
    } else if (m_framebuffer != nullptr && m_vramRange != nullptr && m_framebuffer->getLength() >= bytes) {
        installed = false;
    } else if (m_framebuffer != nullptr
        && (m_retiredFramebuffers == nullptr || !m_retiredFramebuffers->setObject(m_framebuffer))) {
        installRejected = true;
    } else {
        oldFramebuffer = m_framebuffer;
        oldRange = m_vramRange;
        m_framebuffer = framebuffer;
        m_vramRange = range;
        installed = true;
    }
    IOLockUnlock(m_lock);

    if (!installed) {
        range->release();
        framebuffer->complete(kIODirectionInOut);
        framebuffer->release();
        return !installRejected;
    }

    if (oldRange != nullptr) {
        oldRange->release();
    }
    if (oldFramebuffer != nullptr) {
        // m_retiredFramebuffers owns the prepared allocation until IOFramebuffer
        // has stopped using any aperture map backed by its physical pages.
        oldFramebuffer->release();
    }
    return true;
}

void RShareMacVirtualDisplay::releaseBackingStore()
{
    IOBufferMemoryDescriptor* framebuffer = nullptr;
    IODeviceMemory* vramRange = nullptr;
    OSArray* retiredFramebuffers = nullptr;

    if (m_lock != nullptr) {
        IOLockLock(m_lock);
        framebuffer = m_framebuffer;
        vramRange = m_vramRange;
        retiredFramebuffers = m_retiredFramebuffers;
        m_framebuffer = nullptr;
        m_vramRange = nullptr;
        m_retiredFramebuffers = nullptr;
        IOLockUnlock(m_lock);
    } else {
        framebuffer = m_framebuffer;
        vramRange = m_vramRange;
        retiredFramebuffers = m_retiredFramebuffers;
        m_framebuffer = nullptr;
        m_vramRange = nullptr;
        m_retiredFramebuffers = nullptr;
    }

    if (vramRange != nullptr) {
        vramRange->release();
    }
    if (framebuffer != nullptr) {
        framebuffer->complete(kIODirectionInOut);
        framebuffer->release();
    }
    if (retiredFramebuffers != nullptr) {
        const unsigned int count = retiredFramebuffers->getCount();
        for (unsigned int index = 0; index < count; ++index) {
            auto* retired = OSDynamicCast(IOBufferMemoryDescriptor, retiredFramebuffers->getObject(index));
            if (retired != nullptr) {
                retired->complete(kIODirectionInOut);
            }
        }
        retiredFramebuffers->release();
    }
}

void RShareMacVirtualDisplay::notifyConnectionChange()
{
    IOLockLock(m_lock);
    m_connectInterruptPending = true;
    IOLockUnlock(m_lock);

    // IOFramebuffer owns the online/mode notification ordering after this
    // interrupt drives processConnectChange() and setupForCurrentConfig().
    dispatchPendingConnectInterrupt();
}

void RShareMacVirtualDisplay::dispatchPendingConnectInterrupt()
{
    IOLockLock(m_lock);
    if (m_connectInterruptEnabled && m_connectInterruptPending && m_connectInterruptProc != nullptr && m_connectInterruptTarget != nullptr) {
        m_connectInterruptPending = false;
        // Keep unregisterInterrupt()/stop() behind the in-flight callback so
        // IOGraphics cannot tear down its target after we capture a raw pointer.
        m_connectInterruptProc(m_connectInterruptTarget, m_connectInterruptRef);
    }
    IOLockUnlock(m_lock);
}

const RShareMacVdisplayMode* RShareMacVirtualDisplay::modeForId(IODisplayModeID modeId)
{
    for (uint32_t index = 0; index < modeCount(); ++index) {
        if (kRShareMacVdisplayModes[index].mode_id == modeId) {
            return &kRShareMacVdisplayModes[index];
        }
    }
    return nullptr;
}

const RShareMacVdisplayMode* RShareMacVirtualDisplay::modeForRequest(const RShareVdisplayRequest* request)
{
    if (request == nullptr || request->flags != 0) {
        return nullptr;
    }
    for (uint32_t index = 0; index < modeCount(); ++index) {
        const auto& mode = kRShareMacVdisplayModes[index];
        if (mode.width == request->width && mode.height == request->height && mode.refresh_rate_millihz == request->refresh_rate_millihz) {
            return &mode;
        }
    }
    return nullptr;
}

bool RShareMacVirtualDisplayUserClient::initWithTask(task_t owningTask, void* securityToken, UInt32 type, OSDictionary* properties)
{
    const bool localUser = clientHasPrivilege(securityToken, kIOClientPrivilegeLocalUser) == kIOReturnSuccess;
    const bool administrator = clientHasPrivilege(securityToken, kIOClientPrivilegeAdministrator) == kIOReturnSuccess;
    if (!localUser && !administrator) {
        return false;
    }
    if (!IOUserClient::initWithTask(owningTask, securityToken, type, properties)) {
        return false;
    }
    m_owner = nullptr;
    return true;
}

bool RShareMacVirtualDisplayUserClient::start(IOService* provider)
{
    auto* owner = OSDynamicCast(RShareMacVirtualDisplay, provider);
    if (owner == nullptr || !IOUserClient::start(provider)) {
        return false;
    }
    m_owner = owner;
    return true;
}

void RShareMacVirtualDisplayUserClient::stop(IOService* provider)
{
    m_owner = nullptr;
    IOUserClient::stop(provider);
}

IOReturn RShareMacVirtualDisplayUserClient::clientClose()
{
    terminate();
    return kIOReturnSuccess;
}

IOReturn RShareMacVirtualDisplayUserClient::externalMethod(uint32_t selector, IOExternalMethodArguments* arguments, IOExternalMethodDispatch*, OSObject*, void*)
{
    static const IOExternalMethodDispatch dispatch[] = {
        {&RShareMacVirtualDisplayUserClient::QueryVersion, 0, 0, 0, sizeof(RShareDriverVersion)},
        {&RShareMacVirtualDisplayUserClient::QueryCapabilities, 0, 0, 0, sizeof(RShareDriverCapabilities)},
        {&RShareMacVirtualDisplayUserClient::QueryState, 0, 0, 0, sizeof(RShareVdisplayState)},
        {&RShareMacVirtualDisplayUserClient::CreateOrUpdate, 0, sizeof(RShareVdisplayRequest), 0, 0},
        {&RShareMacVirtualDisplayUserClient::Remove, 0, 0, 0, 0},
    };

    if (selector >= static_cast<uint32_t>(sizeof(dispatch) / sizeof(dispatch[0]))) {
        return kIOReturnUnsupported;
    }

    if (m_owner == nullptr) {
        return kIOReturnNoDevice;
    }

    return IOUserClient::externalMethod(selector, arguments, const_cast<IOExternalMethodDispatch*>(&dispatch[selector]), this, nullptr);
}

IOReturn RShareMacVirtualDisplayUserClient::QueryVersion(OSObject* target, void*, IOExternalMethodArguments* arguments)
{
    auto* client = OSDynamicCast(RShareMacVirtualDisplayUserClient, target);
    if (client == nullptr || client->m_owner == nullptr || arguments == nullptr || arguments->structureOutput == nullptr) {
        return kIOReturnBadArgument;
    }
    if (arguments->structureOutputSize < sizeof(RShareDriverVersion)) {
        return kIOReturnNoSpace;
    }

    const IOReturn result = client->m_owner->copyVersion(static_cast<RShareDriverVersion*>(arguments->structureOutput));
    if (result == kIOReturnSuccess) {
        arguments->structureOutputSize = sizeof(RShareDriverVersion);
    }
    return result;
}

IOReturn RShareMacVirtualDisplayUserClient::QueryCapabilities(OSObject* target, void*, IOExternalMethodArguments* arguments)
{
    auto* client = OSDynamicCast(RShareMacVirtualDisplayUserClient, target);
    if (client == nullptr || client->m_owner == nullptr || arguments == nullptr || arguments->structureOutput == nullptr) {
        return kIOReturnBadArgument;
    }
    if (arguments->structureOutputSize < sizeof(RShareDriverCapabilities)) {
        return kIOReturnNoSpace;
    }

    const IOReturn result = client->m_owner->copyCapabilities(static_cast<RShareDriverCapabilities*>(arguments->structureOutput));
    if (result == kIOReturnSuccess) {
        arguments->structureOutputSize = sizeof(RShareDriverCapabilities);
    }
    return result;
}

IOReturn RShareMacVirtualDisplayUserClient::QueryState(OSObject* target, void*, IOExternalMethodArguments* arguments)
{
    auto* client = OSDynamicCast(RShareMacVirtualDisplayUserClient, target);
    if (client == nullptr || client->m_owner == nullptr || arguments == nullptr || arguments->structureOutput == nullptr) {
        return kIOReturnBadArgument;
    }
    if (arguments->structureOutputSize < sizeof(RShareVdisplayState)) {
        return kIOReturnNoSpace;
    }

    const IOReturn result = client->m_owner->copyState(static_cast<RShareVdisplayState*>(arguments->structureOutput));
    if (result == kIOReturnSuccess) {
        arguments->structureOutputSize = sizeof(RShareVdisplayState);
    }
    return result;
}

IOReturn RShareMacVirtualDisplayUserClient::CreateOrUpdate(OSObject* target, void*, IOExternalMethodArguments* arguments)
{
    auto* client = OSDynamicCast(RShareMacVirtualDisplayUserClient, target);
    if (client == nullptr || client->m_owner == nullptr || arguments == nullptr || arguments->structureInput == nullptr) {
        return kIOReturnBadArgument;
    }
    if (arguments->structureInputSize != sizeof(RShareVdisplayRequest)) {
        return kIOReturnBadArgument;
    }

    return client->m_owner->createOrUpdate(static_cast<const RShareVdisplayRequest*>(arguments->structureInput));
}

IOReturn RShareMacVirtualDisplayUserClient::Remove(OSObject* target, void*, IOExternalMethodArguments*)
{
    auto* client = OSDynamicCast(RShareMacVirtualDisplayUserClient, target);
    if (client == nullptr || client->m_owner == nullptr) {
        return kIOReturnBadArgument;
    }
    return client->m_owner->removeVirtualDisplay();
}
