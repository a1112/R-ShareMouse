//! Platform enumeration uses persistent native IDs. A loaded driver is checked
//! independently from the network and from per-device registration.
use rshare_core::network_audio::*;
#[derive(Debug, Clone)]
pub struct BackendStatus {
    pub name: String,
    pub ready: bool,
    pub error: Option<String>,
}
pub fn status() -> BackendStatus {
    #[cfg(target_os = "macos")]
    {
        let mut id = 0;
        let result = unsafe { rsa_macos_plugin_id(&mut id) };
        return BackendStatus {
            name: "CoreAudio AudioServerPlugIn".into(),
            ready: result == 0 && id != 0,
            error: if result == 0 && id != 0 {
                None
            } else {
                Some("RShare audio HAL plug-in is not loaded".into())
            },
        };
    }
    #[cfg(target_os = "linux")]
    {
        return BackendStatus {
            name: "PipeWire".into(),
            ready: false,
            error: Some("PipeWire virtual device bridge is not attached".into()),
        };
    }
    #[cfg(windows)]
    {
        return BackendStatus {
            name: "WaveRT / ASIO".into(),
            ready: false,
            error: Some("RShare WaveRT / ASIO bridge is not attached".into()),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        BackendStatus {
            name: "Unsupported".into(),
            ready: false,
            error: Some("Unsupported operating system".into()),
        }
    }
}
pub fn enumerate() -> Result<Vec<Endpoint>, String> {
    #[cfg(target_os = "macos")]
    {
        let ptr = unsafe { rsa_macos_enumerate() };
        if ptr.is_null() {
            return Err("Core Audio endpoint enumeration failed".into());
        }
        let bytes = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_bytes();
        let result = serde_json::from_slice(bytes)
            .map_err(|e| format!("Invalid native endpoint catalog: {e}"));
        unsafe { rsa_macos_free(ptr) };
        result
    }
    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("pw-dump")
            .output()
            .map_err(|e| format!("PipeWire pw-dump is unavailable: {e}"))?;
        if !output.status.success() {
            return Err("PipeWire server is unavailable".into());
        }
        crate::pipewire::catalog(&output.stdout)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("Native audio endpoint enumeration backend is not attached".into())
    }
}
#[cfg(target_os = "macos")]
extern "C" {
    fn rsa_macos_enumerate() -> *mut std::ffi::c_char;
    fn rsa_macos_free(value: *mut std::ffi::c_char);
    fn rsa_macos_plugin_id(output: *mut u32) -> i32;
    fn rsa_macos_device_command(
        uid: *const std::ffi::c_char,
        name: *const std::ffi::c_char,
        ring: *const std::ffi::c_char,
        channels: u32,
        rate: u32,
        input: u32,
        remove: bool,
    ) -> i32;
}
#[cfg(target_os = "macos")]
pub fn register(
    device: &VirtualDevice,
    ring_path: &str,
    format: Format,
    remove: bool,
) -> Result<(), String> {
    use std::ffi::CString;
    format.validate().map_err(|e| e.to_string())?;
    let uid = CString::new(device.id.as_str()).map_err(|e| e.to_string())?;
    let name = CString::new(device.name.as_str()).map_err(|e| e.to_string())?;
    let ring = CString::new(ring_path).map_err(|e| e.to_string())?;
    let result = unsafe {
        rsa_macos_device_command(
            uid.as_ptr(),
            name.as_ptr(),
            ring.as_ptr(),
            format.channels as u32,
            format.sample_rate,
            (device.endpoint.direction == Direction::Input) as u32,
            remove,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(format!("Core Audio device registration failed: {result}"))
    }
}
