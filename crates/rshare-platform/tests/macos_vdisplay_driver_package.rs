fn read_repo_file(path: &str) -> String {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(path)).expect(path)
}

fn assert_contains_in_order(haystack: &str, first: &str, second: &str) {
    let first_index = haystack
        .find(first)
        .unwrap_or_else(|| panic!("missing first marker: {first}"));
    let second_index = haystack
        .find(second)
        .unwrap_or_else(|| panic!("missing second marker: {second}"));
    assert!(
        first_index < second_index,
        "expected {first:?} to appear before {second:?}"
    );
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MacosVdisplayMode {
    width: u32,
    height: u32,
    refresh_rate_millihz: u32,
}

fn parse_driver_edid_bytes(driver: &str) -> Vec<u8> {
    let edid_body = driver
        .split("kRShareMacVdisplayEdid[128] = {")
        .nth(1)
        .and_then(|tail| tail.split("};").next())
        .expect("driver declares kRShareMacVdisplayEdid");

    edid_body
        .split(|ch: char| !(ch.is_ascii_hexdigit() || ch == 'x'))
        .filter(|token| token.starts_with("0x"))
        .map(|token| u8::from_str_radix(&token[2..], 16).expect("hex EDID byte"))
        .collect()
}

fn parse_driver_modes(driver: &str) -> Vec<MacosVdisplayMode> {
    let modes_body = driver
        .split("constexpr RShareMacVdisplayMode kRShareMacVdisplayModes[] = {")
        .nth(1)
        .and_then(|tail| tail.split("};").next())
        .expect("driver declares kRShareMacVdisplayModes");

    modes_body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let body = trimmed.strip_prefix('{')?.strip_suffix("},")?;
            let mut fields = body.split(',').map(str::trim);
            Some(MacosVdisplayMode {
                width: fields.next()?.parse().ok()?,
                height: fields.next()?.parse().ok()?,
                refresh_rate_millihz: fields.next()?.parse().ok()?,
            })
        })
        .collect()
}

fn parse_rust_modes(rust: &str) -> Vec<MacosVdisplayMode> {
    let modes_body = rust
        .split("const RSHARE_VDISPLAY_SUPPORTED_MODES: &[(u32, u32, u32)] = &[")
        .nth(1)
        .and_then(|tail| tail.split("];").next())
        .expect("Rust platform layer declares RSHARE_VDISPLAY_SUPPORTED_MODES");

    modes_body
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let body = trimmed.strip_prefix('(')?.strip_suffix("),")?;
            let mut fields = body.split(',').map(|field| field.trim().replace('_', ""));
            Some(MacosVdisplayMode {
                width: fields.next()?.parse().ok()?,
                height: fields.next()?.parse().ok()?,
                refresh_rate_millihz: fields.next()?.parse().ok()?,
            })
        })
        .collect()
}

fn parse_validate_script_modes(script: &str) -> Vec<MacosVdisplayMode> {
    let modes_body = script
        .split("SUPPORTED_MODES=(")
        .nth(1)
        .and_then(|tail| tail.split(')').next())
        .expect("validate script declares SUPPORTED_MODES");

    modes_body
        .lines()
        .filter_map(|line| {
            let mode = line.trim().trim_matches('"');
            if mode.is_empty() {
                return None;
            }

            let (resolution, refresh_rate_millihz) = mode.split_once('@')?;
            let (width, height) = resolution.split_once('x')?;
            Some(MacosVdisplayMode {
                width: width.parse().ok()?,
                height: height.parse().ok()?,
                refresh_rate_millihz: refresh_rate_millihz.parse().ok()?,
            })
        })
        .collect()
}

fn edid_manufacturer_id(edid: &[u8]) -> String {
    let encoded = u16::from_be_bytes([edid[8], edid[9]]);
    [10, 5, 0]
        .into_iter()
        .map(|shift| (((encoded >> shift) & 0x1f) as u8 + b'@') as char)
        .collect()
}

fn edid_preferred_timing(edid: &[u8]) -> (u32, u32, u32) {
    let timing = &edid[54..72];
    let pixel_clock_hz = u16::from_le_bytes([timing[0], timing[1]]) as u64 * 10_000;
    let horizontal_active = timing[2] as u32 | (((timing[4] >> 4) as u32) << 8);
    let horizontal_blanking = timing[3] as u32 | (((timing[4] & 0x0f) as u32) << 8);
    let vertical_active = timing[5] as u32 | (((timing[7] >> 4) as u32) << 8);
    let vertical_blanking = timing[6] as u32 | (((timing[7] & 0x0f) as u32) << 8);
    let refresh_millihz = (pixel_clock_hz * 1_000
        / ((horizontal_active + horizontal_blanking) * (vertical_active + vertical_blanking))
            as u64) as u32;

    (horizontal_active, vertical_active, refresh_millihz)
}

fn edid_range_limits(edid: &[u8]) -> (u32, u32, u32, u32, u64) {
    let descriptor = (54..126)
        .step_by(18)
        .map(|offset| &edid[offset..offset + 18])
        .find(|descriptor| descriptor[..5] == [0, 0, 0, 0xfd, 0])
        .expect("EDID declares a display range limits descriptor");

    (
        descriptor[5] as u32,
        descriptor[6] as u32,
        descriptor[7] as u32,
        descriptor[8] as u32,
        descriptor[9] as u64 * 10_000_000,
    )
}

#[test]
fn macos_virtual_display_driver_package_declares_framebuffer_user_client() {
    let header = read_repo_file("drivers/macos/rshare-vdisplay/rshare_vdisplay_shared.h");
    let readme = read_repo_file("drivers/macos/README.md");
    let driver_readme = read_repo_file("drivers/macos/rshare-vdisplay/README.md");
    let driver_header = read_repo_file("drivers/macos/rshare-vdisplay/RShareMacVirtualDisplay.h");
    let driver = read_repo_file("drivers/macos/rshare-vdisplay/RShareMacVirtualDisplay.cpp");
    let plist = read_repo_file("drivers/macos/rshare-vdisplay/Info.plist");
    let check_script = read_repo_file("scripts/driver/check-macos-vdisplay.sh");
    let build_script = read_repo_file("scripts/driver/build-macos-vdisplay.sh");
    let load_script = read_repo_file("scripts/driver/load-macos-vdisplay.sh");
    let unload_script = read_repo_file("scripts/driver/unload-macos-vdisplay.sh");
    let validate_script = read_repo_file("scripts/driver/validate-macos-vdisplay.sh");
    let rust = read_repo_file("crates/rshare-platform/src/virtual_display.rs");
    let macos = read_repo_file("crates/rshare-platform/src/macos.rs");

    assert!(
        header.contains("#define RSHARE_MACOS_VDISPLAY_SERVICE_CLASS \"RShareMacVirtualDisplay\"")
    );
    assert!(header.contains("#define RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE 0x52534d56u"));
    assert!(header.contains("RSHARE_MACOS_SELECTOR_QUERY_VERSION = 0"));
    assert!(header.contains("RSHARE_MACOS_SELECTOR_QUERY_CAPABILITIES = 1"));
    assert!(header.contains("RSHARE_MACOS_SELECTOR_VDISPLAY_QUERY_STATE = 2"));
    assert!(header.contains("RSHARE_MACOS_SELECTOR_VDISPLAY_CREATE = 3"));
    assert!(header.contains("RSHARE_MACOS_SELECTOR_VDISPLAY_REMOVE = 4"));
    assert!(header.contains("RSHARE_CAP_VIRTUAL_DISPLAY 0x00000010u"));
    assert!(header.contains("sizeof(RShareVdisplayRequest) == 16"));
    assert!(header.contains("sizeof(RShareVdisplayState) == 20"));
    assert!(header.contains("offsetof(RShareDriverCapabilities, reserved0) == 2"));
    assert!(header.contains("offsetof(RShareDriverCapabilities, flags) == 4"));
    assert!(header.contains("offsetof(RShareVdisplayRequest, flags) == 12"));
    assert!(header.contains("offsetof(RShareVdisplayState, connector_index) == 16"));

    assert!(rust.contains(
        "const RSHARE_MACOS_VDISPLAY_SERVICE_CLASS: &str = \"RShareMacVirtualDisplay\";"
    ));
    assert!(rust.contains("const RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE: u32 = 0x5253_4d56;"));
    assert!(rust.contains("const RSHARE_MACOS_SELECTOR_QUERY_VERSION: u32 = 0;"));
    assert!(rust.contains("const RSHARE_MACOS_SELECTOR_QUERY_CAPABILITIES: u32 = 1;"));
    assert!(rust.contains("const RSHARE_MACOS_SELECTOR_VDISPLAY_QUERY_STATE: u32 = 2;"));
    assert!(rust.contains("const RSHARE_MACOS_SELECTOR_VDISPLAY_CREATE: u32 = 3;"));
    assert!(rust.contains("const RSHARE_MACOS_SELECTOR_VDISPLAY_REMOVE: u32 = 4;"));
    assert!(rust.contains("IOServiceOpen("));
    assert!(rust.contains("IOConnectCallStructMethod("));
    assert!(rust.contains("enum MacosVirtualDisplayOpenError"));
    assert!(rust.contains("ServiceNotFound(String)"));
    assert!(rust.contains("UserClientOpenFailed(String)"));
    assert!(rust.contains("fn macos_probe_open_error(error: MacosVirtualDisplayOpenError)"));
    assert!(rust.contains("service_available: error.service_available()"));
    assert!(
        rust.contains("macos_probe_open_error_reports_loaded_service_when_user_client_open_fails")
    );
    assert!(rust.contains("macos_probe_open_error_reports_missing_service_when_service_not_found"));

    assert!(readme.contains("IOServiceOpen"));
    assert!(readme.contains("IOConnectCallStructMethod"));
    assert!(readme.contains("DriverUnavailable"));
    assert!(readme.contains("validate-macos-vdisplay.sh"));
    assert!(readme.contains("display virtual driver-status"));
    assert!(readme.contains("approval-required and reboot-required"));
    assert!(readme.contains("kmutil rebuild"));

    assert!(driver_header.contains("class RShareMacVirtualDisplay : public IOFramebuffer"));
    assert!(driver_header.contains("class RShareMacVirtualDisplayUserClient : public IOUserClient"));
    assert!(driver_header.contains("bool start(IOService* provider) override;"));
    assert!(driver_header.contains("void stop(IOService* provider) override;"));
    assert!(
        driver_header.contains("IOReturn createOrUpdate(const RShareVdisplayRequest* request);")
    );
    assert!(driver_header.contains("IOReturn removeVirtualDisplay();"));
    assert!(driver_header.contains("IOReturn enableController() override;"));
    assert!(driver_header.contains("void stop(IOService* provider) override;"));
    assert!(driver_header.contains("IOItemCount getConnectionCount() override;"));
    assert!(driver_header.contains(
        "IOReturn setStartupDisplayMode(IODisplayModeID displayMode, IOIndex depth) override;"
    ));
    assert!(driver_header.contains(
        "IOReturn getStartupDisplayMode(IODisplayModeID* displayMode, IOIndex* depth) override;"
    ));
    assert!(driver_header.contains(
        "IOReturn setAttributeForConnection(IOIndex connectIndex, IOSelect attribute, uintptr_t value) override;"
    ));
    assert!(driver_header.contains("IOFBInterruptProc m_connectInterruptProc;"));
    assert!(driver_header.contains("IODisplayModeID m_requestedMode;"));
    assert!(driver_header.contains("IOIndex m_requestedDepth;"));
    assert!(driver_header.contains("IODisplayModeID m_startupMode;"));
    assert!(driver_header.contains("IOIndex m_startupDepth;"));
    assert!(driver_header.contains("OSArray* m_retiredFramebuffers;"));
    assert!(driver_header.contains("bool m_connectionDetected;"));
    assert!(driver_header.contains("bool m_connectInterruptPending;"));
    assert!(driver_header.contains("bool m_stopping;"));
    assert!(driver_header
        .contains("bool ensureBackingStoreForMode(const RShareMacVdisplayMode* mode);"));

    assert!(
        driver.contains("OSDefineMetaClassAndStructors(RShareMacVirtualDisplay, IOFramebuffer)")
    );
    assert!(driver.contains(
        "OSDefineMetaClassAndStructors(RShareMacVirtualDisplayUserClient, IOUserClient)"
    ));
    assert!(driver.contains("#include <mach/kmod.h>"));
    assert!(driver.contains("kmod_info_t kmod_info = {"));
    assert!(driver.contains("\"io.rshare.mouse.vdisplay\""));
    assert!(driver.contains("\"0.1.0\""));
    assert!(driver.contains("RShareMacVdisplayModuleStart"));
    assert!(driver.contains("RShareMacVdisplayModuleStop"));
    assert!(driver.contains(
        "static_assert(RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE != kIOFBServerConnectType"
    ));
    assert!(driver.contains(
        "static_assert(RSHARE_MACOS_VDISPLAY_USER_CLIENT_TYPE != kIOFBSharedConnectType"
    ));
    assert!(driver
        .contains("return IOFramebuffer::newUserClient(owningTask, securityID, type, clientH);"));
    assert!(driver.contains("constexpr RShareMacVdisplayMode kRShareMacVdisplayModes[]"));
    assert!(driver.contains("{3840, 2160, 60000"));
    assert!(driver.contains("{2560, 1440, 144000"));
    assert!(driver.contains("{1920, 1080, 144000"));
    assert!(driver.contains("R-SHAREMOUSE"));
    assert!(driver.contains("RSM00000001"));
    assert!(driver.contains("kRShareMacVdisplayVendorId = 0x4a6d"));
    assert!(driver.contains("kRShareMacVdisplayProductId = 0x0001"));
    assert!(driver.contains("kRShareMacVdisplaySerialNumber = 0x00000001"));
    assert!(driver.contains("kRShareMacVdisplayImageWidthMillimeters = 520"));
    assert!(driver.contains("kRShareMacVdisplayImageHeightMillimeters = 290"));
    assert!(driver.contains("info->imageWidth = kRShareMacVdisplayImageWidthMillimeters;"));
    assert!(driver.contains("info->imageHeight = kRShareMacVdisplayImageHeightMillimeters;"));
    assert!(driver.contains("OSDictionary::withCapacity(1)"));
    assert!(driver.contains("OSString::withCString(kRShareMacVdisplayFriendlyName)"));
    assert!(driver.contains("productNames->setObject(\"en\", productName)"));
    assert!(driver.contains("entry->setProperty(kDisplayProductName, productNames)"));
    assert_contains_in_order(
        &driver,
        "setName(RSHARE_MACOS_VDISPLAY_SERVICE_CLASS);",
        "return IOFramebuffer::start(provider);",
    );
    assert!(!driver.contains("    registerService();"));
    assert!(driver.contains("IOReturn RShareMacVirtualDisplay::enableController()"));
    assert!(driver.contains("void RShareMacVirtualDisplay::stop(IOService* provider)"));
    assert!(driver.contains("m_connectInterruptProc = nullptr;"));
    assert!(driver.contains("m_connectInterruptEnabled = false;"));
    assert!(driver.contains(
        "m_connectInterruptEnabled = false;\n        m_connectInterruptPending = false;"
    ));
    assert!(driver.contains("m_connectionDetected = false;"));
    assert!(driver.contains("IOFramebuffer::stop(provider);"));
    assert_contains_in_order(
        &driver,
        "IOFramebuffer::stop(provider);",
        "OSSafeReleaseNULL(fVramMap);",
    );
    assert_contains_in_order(
        &driver,
        "OSSafeReleaseNULL(fVramMap);",
        "releaseBackingStore();",
    );
    assert!(driver.contains("m_requestedMode = m_currentMode;"));
    assert!(driver.contains("m_requestedDepth = m_currentDepth;"));
    assert!(driver.contains("m_startupMode = m_currentMode;"));
    assert!(driver.contains("m_startupDepth = m_currentDepth;"));
    assert!(driver.contains("m_connectionDetected = false;"));
    assert!(driver.contains("IOReturn RShareMacVirtualDisplay::setStartupDisplayMode"));
    assert!(driver.contains("IOReturn RShareMacVirtualDisplay::getStartupDisplayMode"));
    assert!(driver.contains("m_startupMode = displayMode;"));
    assert!(driver.contains("*displayMode = m_startupMode;"));
    assert!(driver.contains("m_connectionDetected = true;"));
    assert!(driver.contains("setProperty(kIODisplayEDIDKey"));
    assert!(driver.contains("setProperty(kIODisplayEDIDOriginalKey"));
    assert!(driver.contains("setProperty(kDisplayVendorID, kRShareMacVdisplayVendorId, 32)"));
    assert!(driver.contains("setProperty(kDisplayProductID, kRShareMacVdisplayProductId, 32)"));
    assert!(
        driver.contains("setProperty(kDisplaySerialNumber, kRShareMacVdisplaySerialNumber, 32)")
    );
    assert!(driver.contains("setProperty(kDisplaySerialString, kRShareMacVdisplaySerial)"));
    assert!(driver.contains("publishDisplayProductName(this)"));
    assert!(driver.contains("return ensureBackingStoreForMode(mode) ? kIOReturnSuccess"));
    assert!(driver.contains("if (!connected) {"));
    assert!(driver.contains("Defer the discouraged contiguous allocation"));
    assert!(driver.contains("const IODisplayModeID requestedMode = m_requestedMode;"));
    assert!(!driver.contains("uint32_t maximumBackingSize()"));
    assert!(driver.contains("const uint32_t bytes = backingSizeForMode(mode);"));
    assert!(!driver_header.contains("bool backingStoreVisible() const;"));
    assert!(!driver.contains("bool RShareMacVirtualDisplay::backingStoreVisible() const"));
    assert!(driver.contains("if (m_vramRange != nullptr) {"));
    assert!(driver.contains("IOBufferMemoryDescriptor::withOptions"));
    assert!(driver.contains("kIOMemoryPhysicallyContiguous"));
    assert!(driver.contains("kIOMapWriteCombineCache"));
    assert!(driver.contains("void* framebufferBytes = framebuffer->getBytesNoCopy();"));
    assert!(driver.contains("bzero(framebufferBytes, bytes);"));
    assert!(driver.contains("if (framebuffer->prepare(kIODirectionInOut) != kIOReturnSuccess)"));
    assert!(driver.contains("getPhysicalSegment(0, &segmentLength)"));
    assert!(driver.contains("IODeviceMemory::withRange"));
    assert!(driver.contains("const bool isRequestedMode = displayMode == m_requestedMode;"));
    assert!(driver.contains("info->flags |= kDisplayModeDefaultFlag;"));
    assert!(driver.contains("*flags |= kDisplayModeDefaultFlag;"));
    assert!(driver.contains("range->retain();"));
    assert!(driver.contains("IOBufferMemoryDescriptor* oldFramebuffer = nullptr;"));
    assert!(driver.contains("IODeviceMemory* oldRange = nullptr;"));
    assert!(driver.contains("m_retiredFramebuffers->setObject(m_framebuffer)"));
    assert!(driver.contains("m_retiredFramebuffers owns the prepared allocation"));
    assert!(!driver.contains("oldFramebuffer->complete(kIODirectionInOut)"));
    assert!(driver.contains("retired->complete(kIODirectionInOut);"));
    assert!(driver.contains("m_framebuffer = framebuffer;"));
    assert!(driver.contains("m_vramRange = range;"));
    assert!(driver.contains("m_framebuffer = nullptr;"));
    assert!(driver.contains("m_vramRange = nullptr;"));
    assert!(driver.contains("return kIOReturnOffline;"));
    assert!(driver.contains("pixelInfo->bytesPerPlane = 0;"));
    assert!(driver.contains("const bool connected = m_connectionDetected;"));
    assert!(driver.contains("IOReturn RShareMacVirtualDisplay::setAttributeForConnection"));
    assert!(driver.contains("case kConnectionEnable:"));
    assert!(driver.contains("case kConnectionCheckEnable:"));
    assert!(driver.contains("case kConnectionProbe:"));
    assert!(driver.contains("case kConnectionPower:"));
    assert!(driver.contains("attribute == kConnectionChanged"));
    assert!(driver.contains("case kConnectionPostWake:"));
    assert!(driver.contains("*value = m_connectionDetected ? 1 : 0;"));
    assert!(!driver.contains("m_connectionDetected = nextEnabled;"));
    assert!(driver.contains("m_connectInterruptEnabled"));
    assert!(driver.contains("m_connectInterruptPending = true;"));
    assert_contains_in_order(
        &driver,
        "*interruptRef = this;",
        "dispatchPendingConnectInterrupt();",
    );
    assert!(
        driver.contains("m_connectInterruptProc(m_connectInterruptTarget, m_connectInterruptRef);")
    );
    assert!(!driver.contains("proc(target, ref);"));
    assert!(driver.contains("m_requestedMode = mode->mode_id;"));
    assert!(driver.contains("m_requestedDepth = 0;"));
    assert!(driver.contains("m_state.active = RSHARE_VDISPLAY_ACTIVITY_PENDING;"));
    assert!(driver.contains("case kConnectionCheckEnable:"));
    assert!(driver.contains("m_currentMode == m_requestedMode"));
    assert!(driver.contains("m_currentDepth == m_requestedDepth"));
    assert!(driver.contains("m_state.active == RSHARE_VDISPLAY_ACTIVITY_PENDING"));
    assert!(driver.contains("const bool commitsRequestedMode"));
    assert!(driver.contains("displayMode == m_requestedMode"));
    assert!(driver.contains("depth == m_requestedDepth"));
    assert!(driver
        .contains("m_state.active != RSHARE_VDISPLAY_ACTIVITY_PENDING || commitsRequestedMode"));
    assert!(driver.contains("RSHARE_VDISPLAY_ACTIVITY_PENDING"));
    assert!(driver.contains("m_connectionDetected = true;"));
    assert!(driver.contains("m_state.active = RSHARE_VDISPLAY_ACTIVITY_ACTIVE"));
    assert!(driver.contains("m_state.active = RSHARE_VDISPLAY_ACTIVITY_REMOVED"));
    assert!(driver.contains(
        "m_state.active = RSHARE_VDISPLAY_ACTIVITY_REMOVED;\n    m_connectionDetected = false;"
    ));
    assert!(driver.contains("notifyConnectionChange();"));
    assert!(driver.contains("IOFramebuffer owns the online/mode notification ordering after this"));
    assert!(!driver.contains("deliverFramebufferNotification(kIOFBNotifyOnlineChange)"));
    assert!(!driver.contains("deliverFramebufferNotification(kIOFBNotifyProbed)"));
    assert!(!driver.contains("deliverFramebufferNotification(kIOFBNotifyDisplayDimsChange)"));
    assert!(!driver.contains("deliverFramebufferNotification(kIOFBNotifyDisplayModeDidChange)"));
    assert!(driver.contains("getTimingInfoForDisplayMode"));
    assert!(driver.contains("uint64_t pixelClockForMode(const RShareMacVdisplayMode* mode)"));
    assert!(driver.contains("timing->pixelClock = pixelClockForMode(mode);"));
    assert!(driver.contains("IOItemCount RShareMacVirtualDisplay::getConnectionCount()"));
    assert!(driver.contains("return 1;"));
    assert!(driver.contains("getDDCBlock"));
    assert!(driver.contains("if (!hasDDCConnect(connectIndex))"));
    assert!(driver.contains("return kIOReturnNoDevice;"));
    assert!(driver.contains("RSHARE_MACOS_SELECTOR_QUERY_VERSION"));
    assert!(driver.contains("RSHARE_MACOS_SELECTOR_QUERY_CAPABILITIES"));
    assert!(driver.contains("RSHARE_MACOS_SELECTOR_VDISPLAY_QUERY_STATE"));
    assert!(driver.contains("RSHARE_MACOS_SELECTOR_VDISPLAY_CREATE"));
    assert!(driver.contains("RSHARE_MACOS_SELECTOR_VDISPLAY_REMOVE"));
    assert!(driver.contains("IOUserClient::externalMethod(selector, arguments"));
    assert!(driver.contains(
        "clientHasPrivilege(securityToken, kIOClientPrivilegeLocalUser) == kIOReturnSuccess"
    ));
    assert!(driver.contains(
        "clientHasPrivilege(securityToken, kIOClientPrivilegeAdministrator) == kIOReturnSuccess"
    ));
    assert!(driver.contains("if (!localUser && !administrator)"));
    assert!(driver.contains("bool RShareMacVirtualDisplayUserClient::start(IOService* provider)"));
    assert!(driver.contains("void RShareMacVirtualDisplayUserClient::stop(IOService* provider)"));
    assert!(driver.contains("!IOUserClient::start(provider)"));
    assert!(driver.contains("IOUserClient::stop(provider);"));
    assert!(driver.contains("arguments->structureOutputSize = sizeof(RShareDriverVersion);"));
    assert!(driver.contains("arguments->structureOutputSize = sizeof(RShareDriverCapabilities);"));
    assert!(driver.contains("arguments->structureOutputSize = sizeof(RShareVdisplayState);"));
    assert!(driver.contains("arguments->structureInputSize != sizeof(RShareVdisplayRequest)"));
    assert!(driver.contains("request == nullptr || request->flags != 0"));
    let edid = parse_driver_edid_bytes(&driver);
    assert_eq!(edid.len(), 128, "RShare EDID block must be 128 bytes");
    assert_eq!(
        edid.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        0,
        "RShare EDID block checksum must make the byte sum wrap to zero"
    );
    assert_eq!(edid[126], 0, "base EDID must not declare extension blocks");
    for (offset, tag) in [(72, 0xfc), (90, 0xff), (108, 0xfd)] {
        assert_eq!(&edid[offset..offset + 3], &[0, 0, 0]);
        assert_eq!(edid[offset + 3], tag);
        assert_eq!(edid[offset + 4], 0);
    }
    for offset in [72, 90] {
        let payload = &edid[offset + 5..offset + 18];
        let newline = payload
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("EDID text descriptor must terminate with a newline");
        assert!(
            payload[newline + 1..].iter().all(|byte| *byte == b' '),
            "EDID text descriptor must use space padding"
        );
    }
    assert_eq!(edid_manufacturer_id(&edid), "RSM");
    assert_eq!(u16::from_le_bytes([edid[10], edid[11]]), 0x0001);
    assert_eq!(
        u32::from_le_bytes([edid[12], edid[13], edid[14], edid[15]]),
        0x0000_0001
    );
    assert_eq!(edid_preferred_timing(&edid), (1920, 1080, 60_000));
    let (
        minimum_vertical_hz,
        maximum_vertical_hz,
        minimum_horizontal_khz,
        maximum_horizontal_khz,
        maximum_pixel_clock_hz,
    ) = edid_range_limits(&edid);
    for mode in parse_driver_modes(&driver) {
        let horizontal_blanking = (mode.width / 5).max(160);
        let vertical_blanking = 45;
        let horizontal_frequency_millihz =
            (mode.height + vertical_blanking) as u64 * mode.refresh_rate_millihz as u64;
        let pixel_clock_hz = (mode.width + horizontal_blanking) as u64
            * (mode.height + vertical_blanking) as u64
            * mode.refresh_rate_millihz as u64
            / 1_000;

        assert!(
            mode.refresh_rate_millihz >= minimum_vertical_hz * 1_000
                && mode.refresh_rate_millihz <= maximum_vertical_hz * 1_000,
            "mode {mode:?} falls outside EDID vertical range"
        );
        assert!(
            horizontal_frequency_millihz >= minimum_horizontal_khz as u64 * 1_000_000
                && horizontal_frequency_millihz <= maximum_horizontal_khz as u64 * 1_000_000,
            "mode {mode:?} falls outside EDID horizontal range"
        );
        assert!(
            pixel_clock_hz <= maximum_pixel_clock_hz,
            "mode {mode:?} falls outside EDID pixel clock range"
        );
    }
    assert!(macos.contains("const RSHARE_MACOS_EDID_VENDOR_ID: u32 = 0x4a6d;"));

    assert!(plist.contains("<string>KEXT</string>"));
    assert!(plist.contains("<string>RShareMacVirtualDisplay</string>"));
    assert!(!plist.contains("<key>IOUserClientClass</key>"));
    assert!(plist.contains("<key>com.apple.iokit.IOGraphicsFamily</key>"));

    assert!(driver_readme.contains("subclasses `IOFramebuffer`"));
    assert!(driver_readme.contains("IOUserClient::externalMethod"));
    assert!(driver_readme.contains("local logged-in user or an administrator"));
    assert!(driver_readme.contains("An offline controller does not allocate VRAM"));
    assert!(driver_readme.contains("remaining aperture map is then released"));
    assert!(driver_readme.contains("cannot leave a mapping to recycled memory"));
    assert!(driver_readme.contains("only after IOGraphics performs its connection check"));
    assert!(driver_readme.contains("build-macos-vdisplay.sh"));
    assert!(driver_readme.contains("validate-macos-vdisplay.sh"));
    assert!(driver_readme.contains("display virtual driver-status --strict"));
    assert!(driver_readme.contains("load-macos-vdisplay.sh"));
    assert!(driver_readme.contains("spctl -a -vv -t install"));
    assert!(driver_readme.contains("/Library/Extensions/RShareMacVirtualDisplay.kext"));
    assert!(driver_readme.contains("Auxiliary Kernel Collection"));
    assert!(driver_readme.contains("exit code 27"));
    assert!(driver_readme.contains("exit code 28"));
    assert!(driver_readme.contains("kmutil rebuild"));
    assert!(driver_readme.contains("rshare_kext_state=reboot_required"));
    assert!(driver_readme.contains("unload-macos-vdisplay.sh"));
    assert!(driver_readme.contains("ALLOW_UNSIGNED_KEXT=1"));
    assert!(driver_readme.contains("SIP, Startup Security, or KEXT consent"));
    assert!(driver_readme.contains("`x86_64` and `arm64e`"));
    assert!(driver_readme.contains("Apple silicon"));
    assert!(driver_readme.contains("not a validated shipping kext"));
    assert!(driver_readme.contains("check-macos-vdisplay.sh"));

    assert!(check_script.contains("xcrun --sdk macosx --show-sdk-path"));
    assert!(check_script.contains("Kernel.framework/Versions/A/Headers"));
    assert!(check_script.contains("RShareMacVirtualDisplay.cpp"));
    assert!(check_script.contains("-fsyntax-only"));
    assert!(check_script.contains("--analyze"));
    assert!(check_script.contains("-analyzer-output=text"));
    assert!(check_script.contains("-fapple-kext"));
    assert!(check_script.contains("-Wconversion"));
    assert!(check_script.contains("-Wsign-conversion"));
    assert!(check_script.contains("-Wshadow"));
    assert!(check_script.contains("-Wcast-align"));
    assert!(check_script.contains("-Werror"));

    assert!(build_script.contains("RShareMacVirtualDisplay.kext"));
    assert!(build_script.contains("-fapple-kext"));
    assert!(build_script.contains("-fno-exceptions"));
    assert!(build_script.contains("-fno-rtti"));
    assert!(build_script.contains("ld"));
    assert!(build_script.contains("        -kext \\"));
    assert!(!build_script.contains("        -r \\"));
    assert!(build_script.contains("otool -hv -arch all"));
    assert!(build_script.contains("KEXTBUNDLE"));
    assert!(build_script.contains("_kmod_info"));
    assert!(build_script.contains("ARCHS=\"${ARCHS:-x86_64 arm64e}\""));
    assert!(build_script.contains("lipo \"$bundle_executable\" -verify_arch"));
    assert!(build_script.contains("nm -arch \"$arch\" -g"));
    assert!(build_script.contains("__mod_init_func"));
    assert!(build_script.contains("__mod_term_func"));
    assert!(!build_script.contains("nm -g \"$KEXT_DIR/Contents/MacOS/$EXECUTABLE\" | grep -q"));
    assert!(build_script.contains("SIGN_IDENTITY"));

    assert!(load_script.contains("otool -hv -arch all"));
    assert!(load_script.contains("KEXTBUNDLE"));
    assert!(load_script.contains("_kmod_info"));
    assert!(load_script.contains("REQUIRED_ARCHS=\"${REQUIRED_ARCHS:-x86_64 arm64e}\""));
    assert!(load_script.contains("lipo \"$executable\" -verify_arch"));
    assert!(load_script.contains("nm -arch \"$arch\" -g"));
    assert!(load_script.contains("__mod_init_func"));
    assert!(load_script.contains("__mod_term_func"));
    assert!(!load_script.contains("nm -g \"$executable\" | grep -q"));
    assert!(!load_script.contains("| grep -q"));
    assert!(load_script.contains("codesign --verify --strict"));
    assert!(load_script.contains("spctl -a -vv -t install"));
    assert!(load_script.contains("Gatekeeper install assessment"));
    assert!(load_script.contains("ALLOW_UNSIGNED_KEXT=\"${ALLOW_UNSIGNED_KEXT:-0}\""));
    assert!(load_script.contains("unsigned_development_kext_allowed()"));
    assert!(load_script.contains("System Integrity Protection status: disabled."));
    assert!(load_script.contains("[[ \"$ALLOW_UNSIGNED_KEXT\" == \"1\" ]] || return 1"));
    assert!(load_script.contains(
        "INSTALL_PATH=\"${INSTALL_PATH:-/Library/Extensions/RShareMacVirtualDisplay.kext}\""
    ));
    assert!(load_script.contains("stage_kext_for_auxkc()"));
    assert!(load_script.contains("/usr/bin/ditto --rsrc --extattr --noqtn"));
    assert!(load_script.contains("normalize_kext_permissions()"));
    assert!(load_script.contains("chown -R root:wheel"));
    assert!(load_script.contains("chmod -R go-w"));
    assert!(load_script.contains("codesign --verify --strict \"$INSTALL_PATH\""));
    assert!(load_script.contains("kmutil print-diagnostics -z -p \"$INSTALL_PATH\""));
    assert!(load_script.contains("kmutil load -p \"$INSTALL_PATH\""));
    assert!(load_script.contains("kextload"));
    assert!(load_script.contains("rshare_kext_state=loaded"));
    assert!(load_script.contains("rshare_kext_state=approval_required"));
    assert!(load_script.contains("rshare_kext_state=reboot_required"));
    assert!(load_script.contains("case \"$kmutil_status\" in"));
    assert!(load_script.contains("27|28) ;;"));
    assert_contains_in_order(
        &load_script,
        "source_is_signed=0\nif codesign --verify --strict \"$KEXT_PATH\"",
        "stage_kext_for_auxkc\nif [[ \"$source_is_signed\" -eq 1 ]]; then\n    if ! codesign --verify --strict \"$INSTALL_PATH\"",
    );
    assert_contains_in_order(
        &load_script,
        "normalize_kext_permissions \"$INSTALL_PATH\"",
        "kmutil print-diagnostics -z -p \"$INSTALL_PATH\"",
    );
    assert!(unload_script.contains("io.rshare.mouse.vdisplay"));
    assert!(unload_script.contains("/Library/Extensions/RShareMacVirtualDisplay.kext"));
    assert!(unload_script.contains("kmutil unload -b"));
    assert!(unload_script.contains("kextunload -b"));
    assert!(unload_script.contains("rm -rf \"$INSTALL_PATH\""));
    assert!(unload_script.contains("kmutil rebuild"));
    assert!(unload_script.contains("rshare_kext_state=removal_approval_required"));
    assert!(unload_script.contains("rshare_kext_state=reboot_required"));
    assert!(unload_script.contains("Restart macOS"));

    assert!(validate_script.contains("check-macos-vdisplay.sh"));
    assert!(validate_script.contains("build-macos-vdisplay.sh"));
    assert!(validate_script.contains("OUT_ROOT=\"${OUT_ROOT:-$ROOT/target/macos-vdisplay}\""));
    assert!(validate_script
        .contains("KEXT_PATH=\"${KEXT_PATH:-$OUT_ROOT/RShareMacVirtualDisplay.kext}\""));
    assert!(validate_script.contains("codesign --verify --strict"));
    assert!(validate_script.contains("otool -hv -arch all"));
    assert!(validate_script.contains("KEXTBUNDLE"));
    assert!(validate_script.contains("_kmod_info"));
    assert!(validate_script.contains("REQUIRED_ARCHS=\"${REQUIRED_ARCHS:-x86_64 arm64e}\""));
    assert!(validate_script.contains("lipo \"$executable\" -verify_arch"));
    assert!(validate_script.contains("nm -arch \"$arch\" -g"));
    assert!(validate_script.contains("__mod_init_func"));
    assert!(validate_script.contains("__mod_term_func"));
    assert!(
        !validate_script.contains("nm -g \"$KEXT_PATH/Contents/MacOS/rshare-vdisplay\" | rg -q")
    );
    assert!(!validate_script.contains("| rg -q"));
    assert!(validate_script.contains("spctl -a -vv -t install"));
    assert!(validate_script.contains("spctl install assessment rejected"));
    assert!(validate_script.contains("ALLOW_UNSIGNED_KEXT=\"${ALLOW_UNSIGNED_KEXT:-0}\""));
    assert!(validate_script.contains("unsigned_development_kext_allowed()"));
    assert!(validate_script.contains("System Integrity Protection status: disabled."));
    assert!(validate_script.contains("[[ \"$ALLOW_UNSIGNED_KEXT\" == \"1\" ]] || return 1"));
    assert!(validate_script.contains("sudo env ALLOW_UNSIGNED_KEXT=\"$ALLOW_UNSIGNED_KEXT\""));
    assert!(validate_script.contains("SERVICE_CLASS=\"RShareMacVirtualDisplay\""));
    assert!(validate_script.contains("FRIENDLY_NAME=\"R-SHAREMOUSE\""));
    assert!(validate_script.contains("SERIAL_STRING=\"RSM00000001\""));
    assert!(validate_script.contains("require_command nm"));
    assert!(validate_script.contains("require_command ioreg"));
    assert!(validate_script.contains("Check kext symbol hygiene"));
    assert!(validate_script.contains("nm -m \"$KEXT_PATH/Contents/MacOS/rshare-vdisplay\""));
    assert!(validate_script.contains("__cxa"));
    assert!(validate_script.contains("___gxx_personality"));
    assert!(validate_script.contains("__ZGV"));
    assert!(validate_script.contains("_objc_"));
    assert!(validate_script.contains("_swift_"));
    assert!(validate_script.contains("dyld_stub_binder"));
    assert!(validate_script.contains("kmutil print-diagnostics -z -p"));
    assert!(validate_script.contains("Dependencies: OK"));
    assert!(validate_script.contains("Invalid ownership"));
    assert!(validate_script.contains("kmutil showloaded --bundle-identifier"));
    assert!(validate_script.contains("com.apple.iokit.IOGraphicsFamily"));
    assert!(validate_script.contains("--verify-daemon-display-topology"));
    assert!(validate_script.contains("validate_ioreg_service_output()"));
    assert!(validate_script.contains("wait_for_ioreg_service()"));
    assert!(validate_script.contains("ioreg -r -c \"$SERVICE_CLASS\" -d 2"));
    assert!(validate_script.contains("IODisplayEDID"));
    assert!(validate_script.contains("IODisplayEDIDOriginal"));
    assert!(validate_script.contains("DisplayVendorID"));
    assert!(validate_script.contains("DisplayProductID"));
    assert!(validate_script.contains("DisplaySerialNumber"));
    assert!(validate_script
        .contains("Timed out waiting for $SERVICE_CLASS IORegistry identity properties."));
    assert!(validate_script.contains("wait_for_loaded_kext()"));
    assert!(validate_script.contains("wait_for_driver_status()"));
    assert!(validate_script.contains("wait_for_daemon_virtual_display_api()"));
    assert!(validate_script.contains("Timed out waiting for macOS virtual display user client."));
    assert!(validate_script.contains("Timed out waiting for daemon virtual display IPC."));
    assert!(validate_script.contains("wait_for_loaded_kext"));
    assert!(validate_script.contains("if ! wait_for_loaded_kext; then"));
    assert!(validate_script.contains("Kext installation pending approval or restart"));
    assert!(validate_script.contains("Auxiliary Kernel Collection"));
    assert!(validate_script.contains("exit 3"));
    assert!(validate_script.contains("Verify IORegistry service identity"));
    assert!(validate_script.contains("wait_for_ioreg_service\n    wait_for_driver_status"));
    assert!(validate_script.contains(
        "cargo build -p rshare-daemon -p rshare-cli\n    wait_for_ioreg_service\n    wait_for_driver_status"
    ));
    assert!(validate_script.contains("wait_for_driver_status"));
    assert!(validate_script.contains("cargo run -p rshare-cli -- display virtual list"));
    assert!(validate_script.contains(
        "cargo run -p rshare-cli -- start --daemon\n    wait_for_daemon_virtual_display_api\n    cargo run -p rshare-cli -- display virtual create"
    ));
    assert!(validate_script.contains("cargo run -p rshare-cli -- display virtual driver-status"));
    assert!(validate_script.contains("driver-status --strict"));
    assert!(validate_script.contains("cargo run -p rshare-cli -- display virtual create"));
    assert!(validate_script.contains("cargo run -p rshare-cli -- display virtual verify"));
    assert!(validate_script.contains("cargo run -p rshare-cli -- display virtual remove"));
}

#[test]
fn macos_virtual_display_supported_modes_stay_in_sync() {
    let driver = read_repo_file("drivers/macos/rshare-vdisplay/RShareMacVirtualDisplay.cpp");
    let rust = read_repo_file("crates/rshare-platform/src/virtual_display.rs");
    let validate_script = read_repo_file("scripts/driver/validate-macos-vdisplay.sh");

    let expected = vec![
        MacosVdisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_millihz: 60_000,
        },
        MacosVdisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_millihz: 144_000,
        },
        MacosVdisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_millihz: 90_000,
        },
        MacosVdisplayMode {
            width: 2560,
            height: 1440,
            refresh_rate_millihz: 144_000,
        },
        MacosVdisplayMode {
            width: 2560,
            height: 1440,
            refresh_rate_millihz: 90_000,
        },
        MacosVdisplayMode {
            width: 2560,
            height: 1440,
            refresh_rate_millihz: 60_000,
        },
        MacosVdisplayMode {
            width: 3840,
            height: 2160,
            refresh_rate_millihz: 60_000,
        },
        MacosVdisplayMode {
            width: 1600,
            height: 900,
            refresh_rate_millihz: 60_000,
        },
        MacosVdisplayMode {
            width: 1280,
            height: 720,
            refresh_rate_millihz: 90_000,
        },
        MacosVdisplayMode {
            width: 1280,
            height: 720,
            refresh_rate_millihz: 60_000,
        },
        MacosVdisplayMode {
            width: 1024,
            height: 768,
            refresh_rate_millihz: 75_000,
        },
        MacosVdisplayMode {
            width: 1024,
            height: 768,
            refresh_rate_millihz: 60_000,
        },
    ];

    assert_eq!(parse_driver_modes(&driver), expected);
    assert_eq!(parse_rust_modes(&rust), expected);
    assert_eq!(parse_validate_script_modes(&validate_script), expected);
}
