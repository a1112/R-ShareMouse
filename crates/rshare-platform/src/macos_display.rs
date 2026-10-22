//! Process-owned macOS virtual displays. WindowServer removes them on process exit.
use anyhow::{bail, Result};
use core_graphics::display::CGDisplay;
use rshare_core::{
    LocalDisplayInfo, LocalDisplayState, VirtualDisplayCreateRequest,
    VirtualDisplayOperationResult, VirtualDisplayOperationStatus as Operation,
    VirtualDisplayRemoveRequest, VirtualDisplaySnapshot, VirtualDisplayStatus,
};
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CStr, CString},
    sync::{Mutex, OnceLock},
};

unsafe extern "C" {
    fn rshare_mac_display_create(
        width: u32,
        height: u32,
        hz: f64,
        name: *const c_char,
        display_id: *mut u32,
        error: *mut c_char,
        error_size: usize,
    ) -> *mut c_void;
    fn rshare_mac_display_remove(handle: *mut c_void);
    fn rshare_mac_display_online(display_id: u32) -> i32;
    fn rshare_mac_capture_start(
        display_id: u32,
        width: u32,
        height: u32,
        fps: u32,
        sink: extern "C" fn(*mut c_void, *const u8, usize) -> bool,
        context: *mut c_void,
        error: *mut c_char,
        error_size: usize,
    ) -> *mut c_void;
    fn rshare_mac_capture_error(handle: *mut c_void) -> i32;
    fn rshare_mac_capture_stop(handle: *mut c_void);
    fn rshare_mac_session_unlocked() -> bool;
}

pub struct H264Capture {
    handle: *mut c_void,
    _sender: Box<std::sync::mpsc::SyncSender<Vec<u8>>>,
    pub frames: std::sync::mpsc::Receiver<Vec<u8>>,
    safety_check: std::cell::Cell<std::time::Instant>,
}
extern "C" fn frame_sink(context: *mut c_void, data: *const u8, length: usize) -> bool {
    if length > rshare_core::usb_display::MAX_PACKET - 8 {
        return false;
    }
    let sender = unsafe { &*(context as *const std::sync::mpsc::SyncSender<Vec<u8>>) };
    sender
        .try_send(unsafe { std::slice::from_raw_parts(data, length) }.to_vec())
        .is_ok()
}
impl H264Capture {
    pub fn start(display_id: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        let id: u32 = display_id
            .strip_prefix("macos:")
            .ok_or_else(|| anyhow::anyhow!("Not a macOS display"))?
            .parse()?;
        let (sender, frames) = std::sync::mpsc::sync_channel(2);
        let mut sender = Box::new(sender);
        let mut error = [0 as c_char; 512];
        let handle = unsafe {
            rshare_mac_capture_start(
                id,
                width,
                height,
                fps,
                frame_sink,
                (&mut *sender as *mut std::sync::mpsc::SyncSender<Vec<u8>>).cast(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if handle.is_null() {
            bail!(
                "{}",
                unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
            );
        }
        Ok(Self {
            handle,
            _sender: sender,
            frames,
            safety_check: std::cell::Cell::new(std::time::Instant::now()),
        })
    }
    pub fn check(&self) -> Result<()> {
        if self.safety_check.get().elapsed() >= std::time::Duration::from_millis(500) {
            anyhow::ensure!(
                unsafe { rshare_mac_session_unlocked() },
                "Mac session locked; USB display stopped"
            );
            self.safety_check.set(std::time::Instant::now());
        }
        let error = unsafe { rshare_mac_capture_error(self.handle) };
        anyhow::ensure!(
            error == 0,
            "Screen capture or H.264 encoder stopped: {error}"
        );
        Ok(())
    }
}
impl Drop for H264Capture {
    fn drop(&mut self) {
        unsafe { rshare_mac_capture_stop(self.handle) }
    }
}

struct OwnedDisplay {
    handle: usize,
    snapshot: VirtualDisplaySnapshot,
}
impl Drop for OwnedDisplay {
    fn drop(&mut self) {
        unsafe { rshare_mac_display_remove(self.handle as *mut c_void) }
    }
}
// Native ownership is retained explicitly; all access is serialized by this mutex.
static DISPLAYS: OnceLock<Mutex<BTreeMap<String, OwnedDisplay>>> = OnceLock::new();
fn displays() -> &'static Mutex<BTreeMap<String, OwnedDisplay>> {
    DISPLAYS.get_or_init(Mutex::default)
}

pub fn list() -> Result<Vec<VirtualDisplaySnapshot>> {
    Ok(displays()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|d| d.snapshot.clone())
        .collect())
}

pub fn create(request: &VirtualDisplayCreateRequest) -> Result<VirtualDisplayOperationResult> {
    let id = super::virtual_display::virtual_display_id(request.id.as_deref());
    let mut owned = displays().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(display) = owned.get(&id) {
        return Ok(VirtualDisplayOperationResult {
            status: Operation::AlreadyExists,
            display: Some(display.snapshot.clone()),
            message: Some("Display already belongs to this daemon".into()),
        });
    }
    let name = request
        .name
        .clone()
        .unwrap_or_else(|| "R-ShareMouse USB Display".into());
    let name_c = CString::new(name.as_str())?;
    let mut error = [0 as c_char; 512];
    let mut display_id = 0;
    let hz = request.refresh_rate_millihz.unwrap_or(60000);
    let handle = unsafe {
        rshare_mac_display_create(
            request.width,
            request.height,
            hz as f64 / 1000.,
            name_c.as_ptr(),
            &mut display_id,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if handle.is_null() {
        bail!(
            "{}",
            unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
        );
    }
    let snapshot = VirtualDisplaySnapshot {
        id: id.clone(),
        width: request.width,
        height: request.height,
        refresh_rate_millihz: Some(hz),
        name: Some(name),
        status: VirtualDisplayStatus::Active,
        display_id: Some(format!("macos:{display_id}")),
        message: Some("Independent macOS desktop (experimental CGVirtualDisplay API)".into()),
    };
    owned.insert(
        id,
        OwnedDisplay {
            handle: handle as usize,
            snapshot: snapshot.clone(),
        },
    );
    Ok(VirtualDisplayOperationResult {
        status: Operation::Created,
        display: Some(snapshot),
        message: None,
    })
}

pub fn remove(request: &VirtualDisplayRemoveRequest) -> Result<VirtualDisplayOperationResult> {
    let mut owned = displays().lock().unwrap_or_else(|e| e.into_inner());
    let Some(display) = owned.remove(request.id.trim()) else {
        bail!(
            "No virtual display owned by this daemon with id {}",
            request.id
        );
    };
    let mut snapshot = display.snapshot.clone();
    drop(display);
    if let Some(id) = snapshot
        .display_id
        .as_deref()
        .and_then(|id| id.strip_prefix("macos:"))
        .and_then(|id| id.parse::<u32>().ok())
    {
        anyhow::ensure!(
            unsafe { rshare_mac_display_online(id) } == 0,
            "WindowServer has not yet removed the virtual display"
        );
    }
    snapshot.status = VirtualDisplayStatus::Removed;
    Ok(VirtualDisplayOperationResult {
        status: Operation::Removed,
        display: Some(snapshot),
        message: None,
    })
}

pub fn query() -> Result<LocalDisplayState> {
    let names = list()?;
    let ids =
        CGDisplay::active_displays().map_err(|e| anyhow::anyhow!("CGGetActiveDisplayList: {e}"))?;
    let mut state = LocalDisplayState::default();
    let mut right: i32 = 0;
    let mut bottom: i32 = 0;
    for id in ids {
        let display = CGDisplay::new(id);
        let bounds = display.bounds();
        let display_id = format!("macos:{id}");
        let mode = display.display_mode();
        let width = bounds.size.width as u32;
        let height = bounds.size.height as u32;
        let x = bounds.origin.x as i32;
        let y = bounds.origin.y as i32;
        let primary = display.is_main();
        if primary {
            state.primary_width = width;
            state.primary_height = height;
        }
        state.virtual_x = state.virtual_x.min(x);
        state.virtual_y = state.virtual_y.min(y);
        right = right.max(x + width as i32);
        bottom = bottom.max(y + height as i32);
        state.displays.push(LocalDisplayInfo {
            friendly_name: Some(
                names
                    .iter()
                    .find(|d| d.display_id.as_deref() == Some(&display_id))
                    .and_then(|d| d.name.clone())
                    .unwrap_or_else(|| format!("macOS Display {id}")),
            ),
            display_id,
            width,
            height,
            x,
            y,
            work_x: x,
            work_y: y,
            work_width: width,
            work_height: height,
            primary,
            active: true,
            scale_percent: Some((display.pixels_wide() as f64 / width.max(1) as f64 * 100.) as u32),
            refresh_rate_millihz: mode
                .as_ref()
                .map(|m| (m.refresh_rate() * 1000.) as u32)
                .filter(|hz| *hz > 0),
            ..Default::default()
        });
    }
    state.display_count = state.displays.len();
    state.layout_width = (right - state.virtual_x).max(0) as u32;
    state.layout_height = (bottom - state.virtual_y).max(0) as u32;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" {
        fn rshare_mac_encoder_probe(
            sink: extern "C" fn(*mut c_void, *const u8, usize) -> bool,
            context: *mut c_void,
        ) -> i32;
    }
    #[test]
    fn native_encoder_emits_bounded_avcc_with_parameter_sets() {
        let (mut sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
        let code = unsafe {
            rshare_mac_encoder_probe(
                frame_sink,
                (&mut sender as *mut std::sync::mpsc::SyncSender<Vec<u8>>).cast(),
            )
        };
        assert_eq!(code, 0, "native VideoToolbox encode");
        let frame = receiver
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        let sps = u16::from_be_bytes([frame[0], frame[1]]) as usize;
        let pps = u16::from_be_bytes([frame[2], frame[3]]) as usize;
        assert!(sps > 0 && pps > 0 && frame[4] & 31 == 7 && frame[4 + sps] & 31 == 8);
        assert!(frame.len() < rshare_core::usb_display::MAX_PACKET);
        let mut offset = 4 + sps + pps;
        while offset < frame.len() {
            let length = u32::from_be_bytes(frame[offset..offset + 4].try_into().unwrap()) as usize;
            assert!(length > 0 && length <= frame.len() - offset - 4);
            offset += 4 + length;
        }
        // Optional local artifact for a native encoder -> iOS decoder integration run.
        if let Some(path) = std::env::var_os("RSHARE_ENCODER_FIXTURE") {
            std::fs::write(path, frame).unwrap();
        }
    }
}
