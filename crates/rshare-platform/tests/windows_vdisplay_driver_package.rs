use std::{fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).expect(path)
}

fn extract_rshare_vdisplay_edid(driver: &str) -> Vec<u8> {
    let start = driver
        .find("s_RShareVirtualDisplayEdid[] = {")
        .expect("driver should contain virtual display EDID");
    let body = &driver[start..];
    let body = body
        .split_once('{')
        .expect("EDID initializer should start with {")
        .1
        .split_once("};")
        .expect("EDID initializer should end with };")
        .0;

    body.split(',')
        .filter_map(|token| {
            let token = token.trim();
            if token.is_empty() {
                return None;
            }
            if let Some(hex) = token.strip_prefix("0x") {
                return Some(u8::from_str_radix(hex, 16).expect("valid EDID hex byte"));
            }
            if token.starts_with('\'') && token.ends_with('\'') {
                return token.as_bytes().get(1).copied();
            }
            panic!("unsupported EDID token {token}");
        })
        .collect()
}

#[test]
fn windows_virtual_display_driver_is_a_real_iddcx_package() {
    let driver = read_repo_file("drivers/windows/rshare-vdisplay/driver.cpp");
    let header = read_repo_file("drivers/windows/rshare-vdisplay/driver.h");
    let trace = read_repo_file("drivers/windows/rshare-vdisplay/trace.h");
    let ioctls = read_repo_file("drivers/windows/rshare-common/rshare_ioctls.h");
    let project = read_repo_file("drivers/windows/rshare-vdisplay/rshare-vdisplay.vcxproj");
    let inf = read_repo_file("drivers/windows/rshare-vdisplay/rshare-vdisplay.inf");
    let build_script = read_repo_file("scripts/driver/build.ps1");
    let check_wdk_script = read_repo_file("scripts/driver/check-wdk.ps1");
    let validate_vdisplay_script = read_repo_file("scripts/driver/validate-vdisplay.ps1");
    let sign_script = read_repo_file("scripts/driver/sign-test-driver.ps1");
    let install_script = read_repo_file("scripts/driver/install-test-driver.ps1");
    let uninstall_script = read_repo_file("scripts/driver/uninstall-test-driver.ps1");
    let probe = read_repo_file("drivers/windows/tools/rshare-driver-probe.c");
    let driver_readme = read_repo_file("drivers/windows/README.md");

    assert!(driver.contains("DriverEntry"));
    assert!(driver.contains("IddCxDeviceInitConfig"));
    assert!(driver.contains("IddCxDeviceInitialize"));
    assert!(driver.contains("IddCxAdapterInitAsync"));
    assert!(driver.contains("IddCxMonitorCreate"));
    assert!(driver.contains("IddCxMonitorArrival"));
    assert!(driver.contains("IddCxSwapChainSetDevice"));
    assert!(driver.contains("IddCxSwapChainFinishedProcessingFrame"));
    assert!(driver.contains("RShareVDisplayMonitorQueryModes"));
    assert!(driver.contains("RShareVirtualDisplayMonitor::CopyDefaultModes"));
    assert!(driver.contains("RShareVirtualDisplayMonitor::CopyTargetModes"));
    assert!(driver.contains("RShareVirtualDisplayMonitor::UpdateMode"));
    assert!(driver.contains("RShareVirtualDisplayMonitor::UpdateTargetModes"));
    assert!(driver.contains("IDARG_IN_UPDATEMODES"));
    assert!(driver.contains("IDDCX_UPDATE_REASON_OTHER"));
    assert!(driver.contains("IddCxMonitorUpdateModes(m_Monitor"));
    assert!(driver.contains("monitorContext->Monitor->UpdateTargetModes()"));
    assert!(driver.contains("m_MonitorRequested"));
    assert!(driver.contains("RShareVirtualDisplayDevice::ReportPendingMonitorArrival"));
    assert!(driver.contains("context->Device->ReportPendingMonitorArrival()"));
    assert!(driver.contains("m_MonitorRequested = true"));
    assert!(driver.contains("RSHARE_VDISPLAY_ACTIVITY_PENDING"));
    assert!(driver.contains("m_State.Active = RSHARE_VDISPLAY_ACTIVITY_PENDING"));
    assert!(driver.contains("m_State.Active = RSHARE_VDISPLAY_ACTIVITY_ACTIVE"));
    assert!(driver.contains("m_State.Active = RSHARE_VDISPLAY_ACTIVITY_REMOVED"));
    assert!(driver.contains("if (m_Adapter == nullptr)"));
    assert!(driver.contains("return STATUS_SUCCESS;"));
    assert!(driver.contains("s_RShareVirtualDisplayEdid"));
    assert!(driver.contains("R-SHAREMOUSE"));
    assert!(driver.contains("RSM00000001"));
    assert!(driver.contains("MonitorDescription.DataSize = sizeof(s_RShareVirtualDisplayEdid)"));
    assert!(
        driver.contains("MonitorDescription.pData = const_cast<BYTE*>(s_RShareVirtualDisplayEdid)")
    );
    assert!(!driver.contains("monitorInfo.MonitorDescription.DataSize = 0"));
    assert!(!driver.contains("monitorInfo.MonitorDescription.pData = nullptr"));
    assert!(driver.contains("RShareModesForState"));
    assert!(driver.contains("context->Monitor->CopyTargetModes"));
    assert!(driver.contains("context->Monitor->CopyDefaultModes"));
    assert!(driver.contains("RSHARE_VDISPLAY_MONITOR_CONTAINER_ID"));
    assert!(
        driver.contains("monitorInfo.MonitorContainerId = RSHARE_VDISPLAY_MONITOR_CONTAINER_ID")
    );
    assert!(!driver.contains("CoCreateGuid(&monitorInfo.MonitorContainerId)"));
    assert!(driver.contains("RShareVirtualDisplayDevice::CommitModes"));
    assert!(driver.contains("RShareModeFromSignalInfo"));
    assert!(driver.contains("IDDCX_PATH_FLAGS_ACTIVE"));
    assert!(driver.contains("context->Device->CommitModes(inArgs)"));
    assert!(driver.contains("RefreshRateMillihz"));
    assert!(driver.contains("RShareFillSignalInfo("));
    assert!(driver.contains("DWORD refreshRateMillihz"));
    assert!(driver.contains("signalInfo.vSyncFreq.Numerator = refreshRateMillihz"));
    assert!(driver.contains("signalInfo.vSyncFreq.Denominator = 1000"));
    assert!(driver.contains("{3840, 2160, 60000}"));
    assert!(driver.contains("{2560, 1440, 144000}"));
    assert!(driver.contains("{2560, 1440, 90000}"));
    assert!(driver.contains("{1920, 1080, 144000}"));
    assert!(driver.contains("{1920, 1080, 90000}"));
    assert!(driver.contains("{1024, 768, 75000}"));
    assert!(driver.contains(
        "const DWORD refreshMillihz = RShareRefreshMillihzFromSignalInfo(path.TargetVideoSignalInfo)"
    ));
    assert!(driver.contains("m_State.RefreshRateMillihz = refreshMillihz"));
    assert!(driver.contains("EvtIddCxDeviceIoControl"));
    assert!(driver.contains("WdfDeviceCreateDeviceInterface"));
    assert!(driver.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(driver.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(driver.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(!driver.contains("intentionally stops before registering an IddCx adapter"));

    assert!(header.contains("#include <iddcx.h>"));
    assert!(header.contains("GUID_DEVINTERFACE_RSHARE_VDISPLAY"));
    assert!(header.contains("class RShareVirtualDisplayDevice"));
    assert!(header.contains("NTSTATUS UpdateTargetModes();"));
    assert!(header.contains("void ReportPendingMonitorArrival();"));
    assert!(header.contains("class RShareSwapChainProcessor"));
    assert!(trace.contains("WPP_CONTROL_GUIDS"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_DOS_DEVICE_NAME"));
    assert!(ioctls.contains("RSHARE_CAP_VIRTUAL_DISPLAY"));
    assert!(ioctls.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(ioctls.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(ioctls.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_REQUEST"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_STATE"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_ACTIVITY_REMOVED"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_ACTIVITY_ACTIVE"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_ACTIVITY_PENDING"));

    assert!(project.contains("<IndirectDisplayDriver>true</IndirectDisplayDriver>"));
    assert!(project.contains("<IDDCX_VERSION_MAJOR>1</IDDCX_VERSION_MAJOR>"));
    assert!(project.contains("<IDDCX_VERSION_MINOR>6</IDDCX_VERSION_MINOR>"));
    assert!(project.contains("<UMDF_VERSION_MAJOR>2</UMDF_VERSION_MAJOR>"));
    assert!(project.contains("<ClInclude Include=\"driver.h\" />"));
    assert!(project.contains("<ClInclude Include=\"trace.h\" />"));
    assert!(project.contains("<FilesToPackage Include=\"$(TargetPath)\" />"));

    assert!(inf.contains("DeviceGroupId"));
    assert!(inf.contains("RShareMouseVirtualDisplay"));
    assert!(inf.contains("UmdfService=RShareVDisplay,RShareVDisplay_UmdfService"));
    assert!(inf.contains("rshare-vdisplay.dll"));

    assert!(build_script.contains("drivers\\windows\\rshare-vdisplay\\rshare-vdisplay.vcxproj"));
    assert!(build_script.contains("IddCxStub.lib"));
    assert!(build_script.contains("check-wdk.ps1"));
    assert!(check_wdk_script.contains("Microsoft.WindowsWDK.10.0.26100"));
    assert!(check_wdk_script.contains("iddcx.h"));
    assert!(check_wdk_script.contains("IddCxStub.lib"));
    assert!(check_wdk_script.contains("WdfDriverEntry.lib"));
    assert!(check_wdk_script.contains("WindowsUserModeDriver10.0"));
    assert!(check_wdk_script.contains("winget install"));
    assert!(validate_vdisplay_script.contains("check-wdk.ps1"));
    assert!(validate_vdisplay_script.contains("build.ps1"));
    assert!(validate_vdisplay_script.contains("install-test-driver.ps1"));
    assert!(validate_vdisplay_script.contains("rshare-driver-probe.exe"));
    assert!(validate_vdisplay_script.contains("@(\"vdisplay\", \"create\""));
    assert!(validate_vdisplay_script.contains("ms-settings:display"));
    assert!(validate_vdisplay_script.contains("WaitForManualModeChange"));
    assert!(validate_vdisplay_script.contains("WaitForVirtualDisplayActive"));
    assert!(validate_vdisplay_script.contains("WaitForVirtualDisplayRemoved"));
    assert!(validate_vdisplay_script.contains("Test-RefreshRateMatch"));
    assert!(validate_vdisplay_script.contains("$RefreshRateToleranceMillihz = 1000"));
    assert!(validate_vdisplay_script.contains("$state.Active -eq 2"));
    assert!(validate_vdisplay_script.contains("$state.Active -eq 0"));
    assert!(validate_vdisplay_script.contains("vdisplay state abi="));
    assert!(validate_vdisplay_script.contains("cargo run -p rshare-cli -- display virtual verify"));
    assert!(validate_vdisplay_script.contains("cargo run -p rshare-cli -- display virtual create"));
    assert!(validate_vdisplay_script.contains("cargo run -p rshare-cli -- display virtual remove"));
    assert!(validate_vdisplay_script.contains("Invoke-DaemonVirtualDisplayCreate"));
    assert!(validate_vdisplay_script.contains("Invoke-DaemonVirtualDisplayRemove"));
    assert!(validate_vdisplay_script.contains("VerifyDaemonDisplayTopology"));
    assert!(validate_vdisplay_script.contains("EnsureDaemonForTopologyVerification"));
    assert!(validate_vdisplay_script.contains("cargo build -p rshare-daemon -p rshare-cli"));
    assert!(validate_vdisplay_script.contains("cargo run -p rshare-cli -- start --daemon"));
    assert!(validate_vdisplay_script.contains("Invoke-DaemonDisplayTopologyVerification -ExpectedWidth $Width -ExpectedHeight $Height -ExpectedRefreshRateMillihz $RefreshRateMillihz"));
    assert!(validate_vdisplay_script.contains("Invoke-DaemonDisplayTopologyVerification -ExpectedWidth $state.Width -ExpectedHeight $state.Height -ExpectedRefreshRateMillihz $state.RefreshRateMillihz"));
    assert!(sign_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("ROOT\\RShareVDisplay"));
    assert!(install_script
        .contains("if (-not $devcon -and ($packages | Where-Object { $_.UseDevCon }))"));
    assert!(install_script
        .contains("devcon.exe is required to install root-enumerated driver packages"));
    assert!(uninstall_script.contains("rshare-vdisplay.inf"));

    assert!(probe.contains("probe_vdisplay"));
    assert!(probe.contains("GUID_DEVINTERFACE_RSHARE_VDISPLAY"));
    assert!(probe.contains("CM_Get_Device_Interface_List_SizeW"));
    assert!(probe.contains("CM_Get_Device_Interface_ListW"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(probe.contains("vdisplay_activity_name"));
    assert!(probe.contains("RSHARE_VDISPLAY_ACTIVITY_PENDING"));
    assert!(probe.contains("activity=%s"));
    assert!(probe.contains("vdisplay create"));
    assert!(probe.contains("vdisplay remove"));
    assert!(build_script.contains("Cfgmgr32.lib"));
    assert!(driver_readme.contains("scripts\\driver\\check-wdk.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\validate-vdisplay.ps1"));
    assert!(driver_readme.contains("Microsoft.WindowsWDK.10.0.26100"));
    assert!(driver_readme.contains("EDID-backed monitor"));
    assert!(driver_readme.contains("pending IddCx arrival"));
    assert!(!driver_readme.contains("daemon still needs a Windows user-mode client"));
    assert!(!driver_readme.contains("EDID-less monitor"));
}

#[test]
fn windows_virtual_display_edid_has_stable_identity_and_checksum() {
    let driver = read_repo_file("drivers/windows/rshare-vdisplay/driver.cpp");
    let edid = extract_rshare_vdisplay_edid(&driver);

    assert_eq!(edid.len(), 128);
    assert_eq!(
        &edid[0..8],
        &[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]
    );
    assert_eq!(
        edid.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        0
    );

    let edid_text = String::from_utf8_lossy(&edid);
    assert!(edid_text.contains("R-SHAREMOUSE"));
    assert!(edid_text.contains("RSM00000001"));
}
