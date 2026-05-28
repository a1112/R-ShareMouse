use std::{fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_root().join(path)).expect(path)
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
    assert!(driver.contains("RShareModesForState"));
    assert!(driver.contains("context->Monitor->CopyTargetModes"));
    assert!(driver.contains("context->Monitor->CopyDefaultModes"));
    assert!(driver.contains("RSHARE_VDISPLAY_MONITOR_CONTAINER_ID"));
    assert!(driver.contains("monitorInfo.MonitorContainerId = RSHARE_VDISPLAY_MONITOR_CONTAINER_ID"));
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
    assert!(header.contains("class RShareSwapChainProcessor"));
    assert!(trace.contains("WPP_CONTROL_GUIDS"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_DOS_DEVICE_NAME"));
    assert!(ioctls.contains("RSHARE_CAP_VIRTUAL_DISPLAY"));
    assert!(ioctls.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(ioctls.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(ioctls.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_REQUEST"));
    assert!(ioctls.contains("RSHARE_VDISPLAY_STATE"));

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
    assert!(validate_vdisplay_script.contains("vdisplay state abi="));
    assert!(validate_vdisplay_script.contains("cargo run -p rshare-cli -- display virtual verify"));
    assert!(validate_vdisplay_script.contains("VerifyDaemonDisplayTopology"));
    assert!(sign_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("ROOT\\RShareVDisplay"));
    assert!(install_script.contains("if (-not $devcon -and ($packages | Where-Object { $_.UseDevCon }))"));
    assert!(install_script.contains("devcon.exe is required to install root-enumerated driver packages"));
    assert!(uninstall_script.contains("rshare-vdisplay.inf"));

    assert!(probe.contains("probe_vdisplay"));
    assert!(probe.contains("GUID_DEVINTERFACE_RSHARE_VDISPLAY"));
    assert!(probe.contains("CM_Get_Device_Interface_List_SizeW"));
    assert!(probe.contains("CM_Get_Device_Interface_ListW"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(probe.contains("vdisplay create"));
    assert!(probe.contains("vdisplay remove"));
    assert!(build_script.contains("Cfgmgr32.lib"));
    assert!(driver_readme.contains("scripts\\driver\\check-wdk.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\validate-vdisplay.ps1"));
    assert!(driver_readme.contains("Microsoft.WindowsWDK.10.0.26100"));
    assert!(!driver_readme.contains("daemon still needs a Windows user-mode client"));
}
