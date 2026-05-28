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
    let sign_script = read_repo_file("scripts/driver/sign-test-driver.ps1");
    let install_script = read_repo_file("scripts/driver/install-test-driver.ps1");

    assert!(driver.contains("DriverEntry"));
    assert!(driver.contains("IddCxDeviceInitConfig"));
    assert!(driver.contains("IddCxDeviceInitialize"));
    assert!(driver.contains("IddCxAdapterInitAsync"));
    assert!(driver.contains("IddCxMonitorCreate"));
    assert!(driver.contains("IddCxMonitorArrival"));
    assert!(driver.contains("IddCxSwapChainSetDevice"));
    assert!(driver.contains("IddCxSwapChainFinishedProcessingFrame"));
    assert!(driver.contains("RShareVDisplayMonitorQueryModes"));
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
    assert!(sign_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("drivers\\windows\\rshare-vdisplay"));
    assert!(install_script.contains("ROOT\\RShareVDisplay"));
}
