use std::{fs, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VDisplayMode {
    width: u32,
    height: u32,
    refresh_rate_millihz: u32,
}

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

fn expected_vdisplay_modes() -> Vec<VDisplayMode> {
    vec![
        VDisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_millihz: 60_000,
        },
        VDisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_millihz: 144_000,
        },
        VDisplayMode {
            width: 1920,
            height: 1080,
            refresh_rate_millihz: 90_000,
        },
        VDisplayMode {
            width: 2560,
            height: 1440,
            refresh_rate_millihz: 144_000,
        },
        VDisplayMode {
            width: 2560,
            height: 1440,
            refresh_rate_millihz: 90_000,
        },
        VDisplayMode {
            width: 2560,
            height: 1440,
            refresh_rate_millihz: 60_000,
        },
        VDisplayMode {
            width: 3840,
            height: 2160,
            refresh_rate_millihz: 60_000,
        },
        VDisplayMode {
            width: 1600,
            height: 900,
            refresh_rate_millihz: 60_000,
        },
        VDisplayMode {
            width: 1280,
            height: 720,
            refresh_rate_millihz: 90_000,
        },
        VDisplayMode {
            width: 1280,
            height: 720,
            refresh_rate_millihz: 60_000,
        },
        VDisplayMode {
            width: 1024,
            height: 768,
            refresh_rate_millihz: 75_000,
        },
        VDisplayMode {
            width: 1024,
            height: 768,
            refresh_rate_millihz: 60_000,
        },
    ]
}

fn extract_vdisplay_modes_from_cpp_table(source: &str, marker: &str) -> Vec<VDisplayMode> {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("source should contain {marker}"));
    let body = source[start..]
        .split_once('{')
        .expect("mode table should start with {")
        .1
        .split_once("};")
        .expect("mode table should end with };")
        .0;

    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with('{') {
                return None;
            }
            let values = line
                .trim_matches(|ch| matches!(ch, '{' | '}' | ',' | ' '))
                .split(',')
                .map(|value| value.trim().replace('_', ""))
                .map(|value| value.parse::<u32>().expect("valid mode integer"))
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 3, "mode row should have three values");
            Some(VDisplayMode {
                width: values[0],
                height: values[1],
                refresh_rate_millihz: values[2],
            })
        })
        .collect()
}

fn extract_vdisplay_modes_from_rust_table(source: &str, marker: &str) -> Vec<VDisplayMode> {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("source should contain {marker}"));
    let body = source[start..]
        .split_once("= &[")
        .expect("Rust mode table should start with = &[")
        .1
        .split_once("];")
        .expect("Rust mode table should end with ];")
        .0;

    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with('(') {
                return None;
            }
            let values = line
                .trim_matches(|ch| matches!(ch, '(' | ')' | ',' | ' '))
                .split(',')
                .map(|value| value.trim().replace('_', ""))
                .map(|value| value.parse::<u32>().expect("valid mode integer"))
                .collect::<Vec<_>>();
            assert_eq!(values.len(), 3, "mode row should have three values");
            Some(VDisplayMode {
                width: values[0],
                height: values[1],
                refresh_rate_millihz: values[2],
            })
        })
        .collect()
}

fn extract_vdisplay_modes_from_powershell_table(source: &str, marker: &str) -> Vec<VDisplayMode> {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("source should contain {marker}"));
    let body = source[start..]
        .split_once("@(")
        .expect("PowerShell mode table should start with @(")
        .1
        .split_once(')')
        .expect("PowerShell mode table should end with )")
        .0;

    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("@{") {
                return None;
            }
            let mut width = None;
            let mut height = None;
            let mut refresh = None;
            for part in line
                .trim_matches(|ch| matches!(ch, '@' | '{' | '}' | ',' | ' '))
                .split(';')
            {
                let (key, value) = part
                    .split_once('=')
                    .expect("PowerShell mode entry should use key=value");
                let value = value.trim().replace('_', "");
                match key.trim() {
                    "Width" => width = Some(value.parse::<u32>().expect("valid width")),
                    "Height" => height = Some(value.parse::<u32>().expect("valid height")),
                    "RefreshRateMillihz" => {
                        refresh = Some(value.parse::<u32>().expect("valid refresh rate"))
                    }
                    other => panic!("unexpected PowerShell mode key {other}"),
                }
            }
            Some(VDisplayMode {
                width: width.expect("mode should include width"),
                height: height.expect("mode should include height"),
                refresh_rate_millihz: refresh.expect("mode should include refresh rate"),
            })
        })
        .collect()
}

fn extract_vdisplay_modes_from_frontend_table(source: &str, marker: &str) -> Vec<VDisplayMode> {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("source should contain {marker}"));
    let body = source[start..]
        .split_once('[')
        .expect("frontend mode table should start with [")
        .1
        .split_once("];")
        .expect("frontend mode table should end with ];")
        .0;

    body.lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with('{') {
                return None;
            }
            let mut width = None;
            let mut height = None;
            let mut refresh = None;
            for part in line
                .trim_matches(|ch| matches!(ch, '{' | '}' | ',' | ' '))
                .split(',')
            {
                let (key, value) = part
                    .split_once(':')
                    .expect("frontend mode entry should use key: value");
                let value = value.trim().replace('_', "");
                match key.trim() {
                    "width" => width = Some(value.parse::<u32>().expect("valid width")),
                    "height" => height = Some(value.parse::<u32>().expect("valid height")),
                    "refreshRateMillihz" => {
                        refresh = Some(value.parse::<u32>().expect("valid refresh rate"))
                    }
                    other => panic!("unexpected frontend mode key {other}"),
                }
            }
            Some(VDisplayMode {
                width: width.expect("mode should include width"),
                height: height.expect("mode should include height"),
                refresh_rate_millihz: refresh.expect("mode should include refresh rate"),
            })
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
    let preflight_vdisplay_script = read_repo_file("scripts/driver/preflight-vdisplay.ps1");
    let start_preflight_script = read_repo_file("scripts/driver/start-vdisplay-preflight.ps1");
    let validate_vdisplay_script = read_repo_file("scripts/driver/validate-vdisplay.ps1");
    let sign_script = read_repo_file("scripts/driver/sign-test-driver.ps1");
    let install_script = read_repo_file("scripts/driver/install-test-driver.ps1");
    let uninstall_script = read_repo_file("scripts/driver/uninstall-test-driver.ps1");
    let start_validation_script = read_repo_file("scripts/driver/start-vdisplay-validation.ps1");
    let probe = read_repo_file("drivers/windows/tools/rshare-driver-probe.c");
    let driver_readme = read_repo_file("drivers/windows/README.md");

    assert!(driver.contains("DriverEntry"));
    assert!(driver.contains("#include \"driver.h\"\n#include <initguid.h>"));
    assert!(header.contains("#ifndef UMDF_USING_NTSTATUS"));
    assert!(header.contains("#define UMDF_USING_NTSTATUS"));
    assert!(!header.contains("#include <bugcodes.h>"));
    assert!(driver.contains("IddCxDeviceInitConfig"));
    assert!(driver.contains("IddCxDeviceInitialize"));
    assert!(driver.contains("IddCxAdapterInitAsync"));
    assert!(driver.contains("IddCxMonitorCreate"));
    assert!(driver.contains("IddCxMonitorArrival"));
    assert!(driver.contains("RShareVirtualDisplayDevice::ClearMonitorAfterFailedArrival"));
    assert!(driver.contains("ClearMonitorAfterFailedArrival(monitorCreateOut.MonitorObject)"));
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
    assert!(driver.contains("outArgs->PreferredMonitorModeIdx = NO_PREFERRED_MODE;"));
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
    assert!(driver.contains("RShareIsSupportedMode"));
    assert!(driver.contains(
        "!RShareIsSupportedMode(request.Width, request.Height, request.RefreshRateMillihz)"
    ));
    assert!(driver.contains("return STATUS_NOT_SUPPORTED"));
    assert!(driver.contains(
        "const DWORD refreshMillihz = RShareRefreshMillihzFromSignalInfo(path.TargetVideoSignalInfo)"
    ));
    assert!(driver.contains("m_State.RefreshRateMillihz = refreshMillihz"));
    assert!(driver.contains("EvtIddCxDeviceIoControl"));
    assert!(driver.contains("VOID RShareVDisplayDeviceIoControl("));
    assert!(!driver.contains("NTSTATUS RShareVDisplayDeviceIoControl("));
    assert!(driver.contains("WdfDeviceCreateDeviceInterface"));
    assert!(driver.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(driver.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(driver.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(!driver.contains("intentionally stops before registering an IddCx adapter"));

    assert!(header.contains("#include <iddcx.h>"));
    assert!(header.contains("GUID_DEVINTERFACE_RSHARE_VDISPLAY"));
    assert!(header.contains("class RShareVirtualDisplayDevice"));
    assert!(header.contains("void ClearMonitorAfterFailedArrival(IDDCX_MONITOR monitor);"));
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
    assert!(project.contains("<IDDCX_VERSION_MINOR>2</IDDCX_VERSION_MINOR>"));
    assert!(project
        .contains("<WindowsTargetPlatformVersion>10.0.26100.0</WindowsTargetPlatformVersion>"));
    assert!(project.contains("<UMDF_VERSION_MAJOR>2</UMDF_VERSION_MAJOR>"));
    assert!(project.contains("<UMDF_VERSION_MINOR>25</UMDF_VERSION_MINOR>"));
    assert!(project.contains("<ClInclude Include=\"driver.h\" />"));
    assert!(project.contains("<ClInclude Include=\"trace.h\" />"));
    assert!(project.contains("<FilesToPackage Include=\"$(TargetPath)\" />"));
    assert!(project.contains("Include\\wdf\\umdf\\$(UMDF_VERSION_MAJOR).$(UMDF_VERSION_MINOR)"));
    assert!(project.contains(
        "$(WindowsTargetPlatformVersion)\\um\\iddcx\\$(IDDCX_VERSION_MAJOR).$(IDDCX_VERSION_MINOR)"
    ));
    assert!(project
        .contains("Lib\\wdf\\umdf\\$(Platform)\\$(UMDF_VERSION_MAJOR).$(UMDF_VERSION_MINOR)"));
    assert!(project.contains("Lib\\$(WindowsTargetPlatformVersion)\\um\\$(Platform)\\iddcx\\$(IDDCX_VERSION_MAJOR).$(IDDCX_VERSION_MINOR)"));

    assert!(inf.contains("DeviceGroupId"));
    assert!(inf.contains("RShareMouseVirtualDisplay"));
    assert!(inf.contains("HKR,,Security,,\"D:P(A;;GA;;;BA)(A;;GA;;;SY)(A;;GA;;;UD)(A;;GA;;;BU)\""));
    assert!(inf.contains("UmdfService=RShareVDisplay,RShareVDisplay_UmdfService"));
    assert!(inf.contains("UmdfLibraryVersion=2.25"));
    assert!(project.contains("/DIDDCX_MINIMUM_VERSION_REQUIRED=2"));
    assert!(inf.contains("UmdfExtensions=IddCx0102"));
    assert!(inf.contains("rshare-vdisplay.dll"));

    assert!(build_script.contains("drivers\\windows\\rshare-vdisplay\\rshare-vdisplay.vcxproj"));
    assert!(build_script.contains("IddCxStub.lib"));
    assert!(build_script.contains("check-wdk.ps1"));
    assert!(check_wdk_script.contains("Microsoft.WindowsWDK.10.0.26100"));
    assert!(check_wdk_script.contains("iddcx.h"));
    assert!(check_wdk_script.contains("IddCxStub.lib"));
    assert!(check_wdk_script.contains("WdfDriverEntry.lib"));
    assert!(check_wdk_script.contains("Find-PlatformFile"));
    assert!(check_wdk_script.contains("*\\$TargetPlatform\\*"));
    assert!(check_wdk_script.contains("WindowsUserModeDriver10.0"));
    assert!(check_wdk_script.contains("winget install"));
    assert!(check_wdk_script.contains("[switch]$Quiet"));
    assert!(check_wdk_script.contains("if (-not $Quiet)"));
    assert!(preflight_vdisplay_script.contains("check-wdk.ps1"));
    assert!(preflight_vdisplay_script.contains("-Quiet"));
    assert!(preflight_vdisplay_script.contains("Confirm-SecureBootUEFI"));
    assert!(preflight_vdisplay_script.contains("bcdedit.exe"));
    assert!(preflight_vdisplay_script.contains("testsigning"));
    assert!(preflight_vdisplay_script.contains("rshare-driver-probe.exe"));
    assert!(preflight_vdisplay_script.contains("vdisplay status"));
    assert!(preflight_vdisplay_script.contains("[switch]$Strict"));
    assert!(start_preflight_script.contains("Start-Process"));
    assert!(start_preflight_script.contains("-Verb RunAs"));
    assert!(start_preflight_script.contains("Start-Transcript"));
    assert!(start_preflight_script.contains("target\\driver-validation"));
    assert!(start_preflight_script.contains("preflight-vdisplay.ps1"));
    assert!(start_preflight_script.contains("-Strict"));
    assert!(validate_vdisplay_script.contains("check-wdk.ps1"));
    assert!(validate_vdisplay_script.contains("build.ps1"));
    assert!(validate_vdisplay_script.contains("install-test-driver.ps1"));
    assert!(validate_vdisplay_script.contains("[switch]$EnableTestSigning"));
    assert!(validate_vdisplay_script.contains("$installArgs.EnableTestSigning = $true"));
    assert!(validate_vdisplay_script.contains("rshare-driver-probe.exe"));
    assert!(validate_vdisplay_script.contains("@(\"vdisplay\", \"create\""));
    assert!(validate_vdisplay_script.contains("ms-settings:display"));
    assert!(validate_vdisplay_script.contains("WaitForManualModeChange"));
    assert!(validate_vdisplay_script.contains("WaitForVirtualDisplayActive"));
    assert!(validate_vdisplay_script.contains("WaitForVirtualDisplayRemoved"));
    assert!(validate_vdisplay_script.contains("Test-RefreshRateMatch"));
    assert!(validate_vdisplay_script.contains("$SupportedVirtualDisplayModes"));
    assert!(validate_vdisplay_script.contains("Test-SupportedVirtualDisplayMode"));
    assert!(validate_vdisplay_script.contains("Assert-SupportedVirtualDisplayMode"));
    assert!(validate_vdisplay_script.contains("Assert-SupportedVirtualDisplayMode -Width $Width -Height $Height -RefreshRateMillihz $RefreshRateMillihz"));
    assert!(validate_vdisplay_script.contains("Build-VDisplayModeString"));
    assert!(validate_vdisplay_script.contains("Build-VDisplayModeString -Width $Width -Height $Height -RefreshRateMillihz $RefreshRateMillihz"));
    assert!(validate_vdisplay_script.contains("Build-VDisplayModeString -Width $state.Width -Height $state.Height -RefreshRateMillihz $state.RefreshRateMillihz"));
    assert!(validate_vdisplay_script.contains("\"--mode\""));
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
    assert!(validate_vdisplay_script
        .contains("Invoke-DaemonDisplayTopologyVerification -Mode $requestedMode"));
    assert!(validate_vdisplay_script
        .contains("Invoke-DaemonDisplayTopologyVerification -Mode $observedMode"));
    assert!(sign_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("[switch]$EnableTestSigning"));
    assert!(install_script.contains("Confirm-SecureBootUEFI"));
    assert!(install_script.contains("Secure Boot is enabled"));
    assert!(install_script.contains("& $bcdEdit /set testsigning on"));
    assert!(install_script.contains("Reboot Windows, then re-run this script."));
    assert!(install_script.contains("ROOT\\RShareVDisplay"));
    assert!(install_script
        .contains("if (-not $devcon -and ($packages | Where-Object { $_.UseDevCon }))"));
    assert!(install_script
        .contains("devcon.exe is required to install root-enumerated driver packages"));
    assert!(uninstall_script.contains("rshare-vdisplay.inf"));
    assert!(start_validation_script.contains("Start-Process"));
    assert!(start_validation_script.contains("-Verb RunAs"));
    assert!(start_validation_script.contains("Start-Transcript"));
    assert!(start_validation_script.contains("target\\driver-validation"));
    assert!(start_validation_script.contains("validate-vdisplay.ps1"));
    assert!(start_validation_script.contains("[switch]$EnableTestSigning"));
    assert!(start_validation_script
        .contains("$(if ($EnableTestSigning) { '-EnableTestSigning' } else { '' })"));
    assert!(start_validation_script.contains("-VerifyDaemonDisplayTopology"));
    assert!(start_validation_script.contains("-WaitForManualModeChange"));

    assert!(probe.contains("probe_vdisplay"));
    assert!(probe.contains("GUID_DEVINTERFACE_RSHARE_VDISPLAY"));
    assert!(probe.contains("CM_Get_Device_Interface_List_SizeW"));
    assert!(probe.contains("CM_Get_Device_Interface_ListW"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_QUERY_STATE"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_CREATE"));
    assert!(probe.contains("IOCTL_RSHARE_VDISPLAY_REMOVE"));
    assert!(probe.contains("vdisplay_activity_name"));
    assert!(probe.contains("vdisplay_is_supported_mode"));
    assert!(probe.contains("unsupported vdisplay mode"));
    assert!(probe.contains("return 19"));
    assert!(probe.contains("RSHARE_VDISPLAY_ACTIVITY_PENDING"));
    assert!(probe.contains("activity=%s"));
    assert!(probe.contains("vdisplay create"));
    assert!(probe.contains("vdisplay remove"));
    assert!(build_script.contains("Cfgmgr32.lib"));
    assert!(driver_readme.contains("scripts\\driver\\check-wdk.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\preflight-vdisplay.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\start-vdisplay-preflight.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\validate-vdisplay.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\start-vdisplay-validation.ps1"));
    assert!(driver_readme.contains("-EnableTestSigning"));
    assert!(driver_readme.contains("Disable Secure Boot"));
    assert!(driver_readme.contains("Microsoft.WindowsWDK.10.0.26100"));
    assert!(driver_readme.contains("EDID-backed monitor"));
    assert!(driver_readme.contains("pending IddCx arrival"));
    assert!(!driver_readme.contains("daemon still needs a Windows user-mode client"));
    assert!(!driver_readme.contains("EDID-less monitor"));
}

#[test]
fn windows_hid_drivers_cover_keyboard_mouse_capture_and_injection_package() {
    let vhid = read_repo_file("drivers/windows/rshare-vhid/driver.c");
    let filter = read_repo_file("drivers/windows/rshare-filter/driver.c");
    let ioctls = read_repo_file("drivers/windows/rshare-common/rshare_ioctls.h");
    let probe = read_repo_file("drivers/windows/tools/rshare-driver-probe.c");
    let install_script = read_repo_file("scripts/driver/install-test-driver.ps1");
    let sign_script = read_repo_file("scripts/driver/sign-test-driver.ps1");
    let uninstall_script = read_repo_file("scripts/driver/uninstall-test-driver.ps1");
    let validate_hid_script = read_repo_file("scripts/driver/validate-hid.ps1");
    let start_hid_validation_script = read_repo_file("scripts/driver/start-hid-validation.ps1");
    let driver_readme = read_repo_file("drivers/windows/README.md");

    assert!(ioctls.contains("RSHARE_CAP_FILTER_EVENTS"));
    assert!(ioctls.contains("RSHARE_CAP_VIRTUAL_KEYBOARD"));
    assert!(ioctls.contains("RSHARE_CAP_VIRTUAL_MOUSE"));
    assert!(ioctls.contains("IOCTL_RSHARE_READ_EVENT"));
    assert!(ioctls.contains("IOCTL_RSHARE_INJECT_REPORT"));
    assert!(ioctls.contains("IOCTL_RSHARE_QUERY_STATS"));
    assert!(ioctls.contains("RSHARE_DRIVER_STATS"));
    assert!(ioctls.contains("KeyboardConnectCount"));
    assert!(ioctls.contains("MouseConnectCount"));
    assert!(ioctls.contains("DroppedEventCount"));

    assert!(filter.contains("IOCTL_INTERNAL_KEYBOARD_CONNECT"));
    assert!(filter.contains("version->Minor = 3"));
    assert!(filter.contains("IOCTL_INTERNAL_MOUSE_CONNECT"));
    assert!(filter.contains("RShareFilterKeyboardServiceCallback"));
    assert!(filter.contains("RShareFilterMouseServiceCallback"));
    assert!(filter.contains("RShareIncrementStatsCounter(&context->KeyboardConnectCount)"));
    assert!(filter.contains("RShareIncrementStatsCounter(&context->MouseConnectCount)"));
    assert!(filter.contains("RShareSnapshotStats"));
    assert!(filter.contains("RSHARE_EVENT_QUEUE_CAPACITY"));
    assert!(filter.contains("context->Tail = (context->Tail + 1u) % RSHARE_EVENT_QUEUE_CAPACITY"));
    assert!(filter.contains("RShareIncrementStatsCounter(&context->DroppedEventCount)"));
    assert!(filter.contains("RSHARE_EVENT_MOUSE_WHEEL"));

    assert!(vhid.contains("g_RShareKeyboardModifiers"));
    assert!(vhid.contains("g_RShareKeyboardKeys[6]"));
    assert!(vhid.contains("RShareAddKeyboardUsage"));
    assert!(vhid.contains("RShareRemoveKeyboardUsage"));
    assert!(vhid.contains("report[1] = g_RShareKeyboardModifiers"));
    assert!(vhid.contains("RtlCopyMemory(&report[3], g_RShareKeyboardKeys"));
    assert!(vhid.contains("case 0x70:"));
    assert!(vhid.contains("return 0x3A"));
    assert!(vhid.contains("case 0x25:"));
    assert!(vhid.contains("return 0x50"));
    assert!(vhid.contains("case 0x2E:"));
    assert!(vhid.contains("return 0x4C"));
    assert!(vhid.contains("case 0x60:"));
    assert!(vhid.contains("return 0x62"));
    assert!(vhid.contains("g_RShareMouseButtons"));
    assert!(vhid.contains("RSHARE_REPORT_MOUSE_WHEEL"));

    assert!(probe.contains("rshare-driver-probe filter status"));
    assert!(probe.contains("rshare-driver-probe filter stats"));
    assert!(probe.contains("rshare-driver-probe filter test"));
    assert!(probe.contains("rshare-driver-probe filter watch [timeout_seconds]"));
    assert!(probe.contains("rshare-driver-probe filter drain [quiet_ms] [timeout_seconds]"));
    assert!(probe.contains("rshare-driver-probe filter watch-keyboard [timeout_seconds]"));
    assert!(probe.contains("rshare-driver-probe filter watch-mouse [timeout_seconds]"));
    assert!(probe.contains("probe_filter_watch"));
    assert!(probe.contains("probe_filter_drain"));
    assert!(probe.contains("probe_filter_stats"));
    assert!(probe.contains("RSHARE_DEVICE_KEYBOARD"));
    assert!(probe.contains("RSHARE_DEVICE_MOUSE"));
    assert!(probe.contains("RSHARE_SOURCE_HARDWARE"));
    assert!(probe.contains("rshare-driver-probe vhid status"));
    assert!(probe.contains("rshare-driver-probe vhid inject-smoke"));

    assert!(install_script.contains("[switch]$EnableInputClassFilters"));
    assert!(install_script.contains("[switch]$HidOnly"));
    assert!(install_script.contains("Normalize-RShareUpperFilters"));
    assert!(install_script.contains("Get-RShareClassDriverName"));
    assert!(install_script.contains("Insert-RShareFilterBeforeClassDriver"));
    assert!(install_script.contains(
        "rshare-filter must be inserted before the keyboard/mouse class driver"
    ));
    assert!(!install_script.contains("rshare-filter must be below the keyboard/mouse class driver"));
    assert!(install_script.contains("Test-PnPUtilDriverInstallSucceeded"));
    assert!(install_script.contains("Driver package is up-to-date"));
    assert!(install_script.contains("Test-DevConDriverInstallSucceeded"));
    assert!(install_script.contains("Drivers installed successfully"));
    assert!(install_script.contains("[string[]]$updated"));
    assert!(install_script.contains("Ensure-RShareClassFilterService"));
    assert!(install_script.contains("Copy-Item -LiteralPath $DriverPath"));
    assert!(install_script.contains("sc.exe"));
    assert!(install_script.contains("type= kernel"));
    assert!(install_script.contains("$signArgs.HidOnly = $true"));
    assert!(install_script.contains("$signArgs.IncludeFilter = $true"));
    assert!(install_script.contains("Add-RShareClassUpperFilter"));
    assert!(install_script.contains("{4D36E96B-E325-11CE-BFC1-08002BE10318}"));
    assert!(install_script.contains("{4D36E96F-E325-11CE-BFC1-08002BE10318}"));
    assert!(install_script.contains("UpperFilters"));
    assert!(
        install_script.contains("Restart or reboot Windows before validating real filter capture")
    );
    assert!(validate_hid_script.contains("filter\", \"drain\""));
    assert!(validate_hid_script.contains("filter\", \"watch-keyboard\""));
    assert!(validate_hid_script.contains("filter\", \"watch-mouse\""));
    assert!(validate_hid_script.contains("Drain filter queue after virtual HID injection"));
    assert!(validate_hid_script.contains("Assert-FilterDriverVersion"));
    assert!(validate_hid_script.contains("Assert-FilterDriverStats"));
    assert!(validate_hid_script.contains("MinFilterMinorVersion"));
    assert!(validate_hid_script.contains("@(\"filter\", \"stats\")"));
    assert!(validate_hid_script.contains("keyboard_connect"));
    assert!(validate_hid_script.contains("mouse_connect"));
    assert!(validate_hid_script.contains("The loaded rshare-filter driver is older than expected"));
    assert!(validate_hid_script.contains("The filter driver has not attached to a keyboard class stack"));
    assert!(validate_hid_script.contains("The filter driver has not attached to a mouse class stack"));
    assert!(uninstall_script.contains("Remove-RShareClassUpperFilter"));
    assert!(uninstall_script.contains("Normalize-RShareUpperFilters"));
    assert!(uninstall_script.contains("[string[]]$updated"));
    assert!(uninstall_script.contains("Remove-RShareClassFilterService"));
    assert!(uninstall_script.contains("delete rshare-filter"));
    assert!(uninstall_script.contains("rshare-filter"));
    assert!(sign_script.contains("[switch]$HidOnly"));
    assert!(sign_script.contains("-not $HidOnly"));
    assert!(validate_hid_script.contains("install-test-driver.ps1"));
    assert!(validate_hid_script.contains("HidOnly = $true"));
    assert!(validate_hid_script.contains("-EnableInputClassFilters"));
    assert!(validate_hid_script.contains("return"));
    assert!(validate_hid_script.contains("Re-run with -SkipBuild -SkipInstall after reboot"));
    assert!(validate_hid_script.contains("@(\"filter\", \"status\")"));
    assert!(validate_hid_script.contains("@(\"vhid\", \"status\")"));
    assert!(validate_hid_script.contains("@(\"vhid\", \"inject-smoke\")"));
    assert!(validate_hid_script.contains("Press and release a keyboard key"));
    assert!(validate_hid_script.contains("Move the mouse or click a mouse button"));
    assert!(start_hid_validation_script.contains("Start-Transcript"));
    assert!(start_hid_validation_script.contains("target\\driver-validation"));
    assert!(start_hid_validation_script.contains("validate-hid.ps1"));
    assert!(driver_readme.contains("-EnableInputClassFilters"));
    assert!(driver_readme.contains("scripts\\driver\\validate-hid.ps1"));
    assert!(driver_readme.contains("scripts\\driver\\start-hid-validation.ps1"));
    assert!(driver_readme.contains("rshare-driver-probe filter stats"));
    assert!(driver_readme.contains("watch-keyboard"));
    assert!(driver_readme.contains("watch-mouse"));
    assert!(driver_readme.contains("target\\driver-validation"));
}

#[test]
fn virtual_display_supported_mode_tables_stay_synchronized() {
    let driver = read_repo_file("drivers/windows/rshare-vdisplay/driver.cpp");
    let platform = read_repo_file("crates/rshare-platform/src/virtual_display.rs");
    let probe = read_repo_file("drivers/windows/tools/rshare-driver-probe.c");
    let validate_script = read_repo_file("scripts/driver/validate-vdisplay.ps1");
    let frontend = read_repo_file("apps/rshare-desktop-frontend/src/app/desktop-model.mjs");
    let cli = read_repo_file("apps/rshare-cli/src/commands/display.rs");
    let expected = expected_vdisplay_modes();

    assert_eq!(
        extract_vdisplay_modes_from_cpp_table(&driver, "RShareMonitorModes"),
        expected
    );
    assert_eq!(
        extract_vdisplay_modes_from_rust_table(&platform, "RSHARE_VDISPLAY_SUPPORTED_MODES"),
        expected
    );
    assert_eq!(
        extract_vdisplay_modes_from_cpp_table(&probe, "RSHARE_PROBE_VDISPLAY_MODES"),
        expected
    );
    assert_eq!(
        extract_vdisplay_modes_from_powershell_table(
            &validate_script,
            "$SupportedVirtualDisplayModes"
        ),
        expected
    );
    assert_eq!(
        extract_vdisplay_modes_from_frontend_table(&frontend, "VIRTUAL_DISPLAY_CREATE_MODES"),
        expected
    );
    assert_eq!(
        extract_vdisplay_modes_from_rust_table(&cli, "SUPPORTED_VIRTUAL_DISPLAY_MODES"),
        expected
    );
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
