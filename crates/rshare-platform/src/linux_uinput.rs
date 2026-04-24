//! Linux uinput virtual device driver support
//!
//! This module provides support for creating virtual input devices using
//! the Linux uinput subsystem. This allows injecting input events that
//! appear to come from real hardware devices.

use anyhow::{Context, Result};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use input_event_codes::*;

/// Virtual input device type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Keyboard,
    Mouse,
    Gamepad,
}

/// UInput virtual device
pub struct UInputDevice {
    file: Option<File>,
    device_type: DeviceType,
    active: Arc<AtomicBool>,
}

unsafe impl Send for UInputDevice {}

impl UInputDevice {
    /// Create a new virtual keyboard device
    pub fn new_keyboard() -> Result<Self> {
        Self::create(DeviceType::Keyboard)
    }

    /// Create a new virtual mouse device
    pub fn new_mouse() -> Result<Self> {
        Self::create(DeviceType::Mouse)
    }

    /// Create a new virtual gamepad device
    pub fn new_gamepad() -> Result<Self> {
        Self::create(DeviceType::Gamepad)
    }

    /// Create a new virtual input device
    fn create(device_type: DeviceType) -> Result<Self> {
        #[cfg(feature = "x11")]
        {
            use nix::fcntl::OFlag;
            use nix::sys::stat::Mode;
            use std::fs::OpenOptions;

            // Try multiple uinput device paths
            let paths = ["/dev/uinput", "/dev/input/uinput", "/dev/misc/uinput"];

            let mut file = None;
            for path in &paths {
                if std::path::Path::new(path).exists() {
                    match OpenOptions::new().read(true).write(true).open(path) {
                        Ok(f) => {
                            file = Some(f);
                            break;
                        }
                        Err(e) => {
                            tracing::debug!("Failed to open {}: {}", path, e);
                        }
                    }
                }
            }

            let file = match file {
                Some(f) => f,
                None => {
                    anyhow::bail!(
                        "No uinput device found. Tried: {:?}. \
                         Make sure the uinput module is loaded: modprobe uinput",
                        paths
                    );
                }
            };

            let fd = file.as_raw_fd();

            // Setup the device based on type
            match device_type {
                DeviceType::Keyboard => {
                    Self::setup_keyboard_device(fd)?;
                }
                DeviceType::Mouse => {
                    Self::setup_mouse_device(fd)?;
                }
                DeviceType::Gamepad => {
                    Self::setup_gamepad_device(fd)?;
                }
            }

            // Create the device
            unsafe {
                if libc::ioctl(fd, UI_DEV_CREATE, 0) < 0 {
                    anyhow::bail!("Failed to create uinput device");
                }
            }

            tracing::info!("Created virtual uinput device: {:?}", device_type);

            Ok(Self {
                file: Some(file),
                device_type,
                active: Arc::new(AtomicBool::new(true)),
            })
        }

        #[cfg(not(feature = "x11"))]
        {
            anyhow::bail!("uinput support not enabled");
        }
    }

    #[cfg(feature = "x11")]
    fn setup_keyboard_device(fd: i32) -> Result<()> {
        use std::ffi::CString;

        // Enable keyboard events
        unsafe {
            // Enable EV_KEY event type
            if libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable EV_KEY");
            }

            // Enable all key codes (0-255)
            for key in KEY_ESC..KEY_PAUSE {
                if libc::ioctl(fd, UI_SET_KEYBIT, key as libc::c_ulong) < 0 {
                    // Some keys may not be supported, continue
                }
            }

            // Set device name
            let name = CString::new("R-ShareMouse Virtual Keyboard").unwrap();
            let mut setup: libc::input_id = std::mem::zeroed();
            setup.bustype = BUS_VIRTUAL as u16;
            setup.vendor = 0x1234;
            setup.product = 0x5678;
            setup.version = 1;

            let mut uinput_setup = uinput_setup {
                id: setup,
                name: [0; 80],
                ff_effects_max: 0,
            };

            let name_bytes = name.as_bytes_with_nul();
            uinput_setup.name[..name_bytes.len()]
                .copy_from_slice(unsafe { std::mem::transmute::<&[u8], &[i8]>(name_bytes) });

            if libc::ioctl(fd, UI_DEV_SETUP, &uinput_setup) < 0 {
                anyhow::bail!("Failed to setup uinput device");
            }
        }

        Ok(())
    }

    #[cfg(feature = "x11")]
    fn setup_mouse_device(fd: i32) -> Result<()> {
        use std::ffi::CString;

        unsafe {
            // Enable EV_KEY, EV_REL, and EV_ABS event types
            if libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable EV_KEY");
            }
            if libc::ioctl(fd, UI_SET_EVBIT, EV_REL as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable EV_REL");
            }

            // Enable mouse buttons
            let buttons = [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA];
            for btn in &buttons {
                if libc::ioctl(fd, UI_SET_KEYBIT, *btn as libc::c_ulong) < 0 {
                    // Continue even if some buttons fail
                }
            }

            // Enable relative axes
            if libc::ioctl(fd, UI_SET_RELBIT, REL_X as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable REL_X");
            }
            if libc::ioctl(fd, UI_SET_RELBIT, REL_Y as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable REL_Y");
            }
            if libc::ioctl(fd, UI_SET_RELBIT, REL_WHEEL as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable REL_WHEEL");
            }
            if libc::ioctl(fd, UI_SET_RELBIT, REL_HWHEEL as libc::c_ulong) < 0 {
                // Some systems don't have horizontal wheel
            }

            // Set device name
            let name = CString::new("R-ShareMouse Virtual Mouse").unwrap();
            let mut setup: libc::input_id = std::mem::zeroed();
            setup.bustype = BUS_VIRTUAL as u16;
            setup.vendor = 0x1234;
            setup.product = 0x5679;
            setup.version = 1;

            let mut uinput_setup = uinput_setup {
                id: setup,
                name: [0; 80],
                ff_effects_max: 0,
            };

            let name_bytes = name.as_bytes_with_nul();
            uinput_setup.name[..name_bytes.len()]
                .copy_from_slice(unsafe { std::mem::transmute::<&[u8], &[i8]>(name_bytes) });

            if libc::ioctl(fd, UI_DEV_SETUP, &uinput_setup) < 0 {
                anyhow::bail!("Failed to setup uinput device");
            }
        }

        Ok(())
    }

    #[cfg(feature = "x11")]
    fn setup_gamepad_device(fd: i32) -> Result<()> {
        use std::ffi::CString;

        unsafe {
            // Enable EV_KEY, EV_ABS, and EV_FF event types
            if libc::ioctl(fd, UI_SET_EVBIT, EV_KEY as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable EV_KEY");
            }
            if libc::ioctl(fd, UI_SET_EVBIT, EV_ABS as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable EV_ABS");
            }

            // Enable gamepad buttons
            for btn in BTN_GAMEPAD..BTN_DEAD {
                if libc::ioctl(fd, UI_SET_KEYBIT, btn as libc::c_ulong) < 0 {
                    // Continue even if some buttons fail
                }
            }

            // Enable absolute axes
            if libc::ioctl(fd, UI_SET_ABSBIT, ABS_X as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable ABS_X");
            }
            if libc::ioctl(fd, UI_SET_ABSBIT, ABS_Y as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable ABS_Y");
            }
            if libc::ioctl(fd, UI_SET_ABSBIT, ABS_Z as libc::c_ulong) < 0 {
                // Some systems don't have Z axis
            }
            if libc::ioctl(fd, UI_SET_ABSBIT, ABS_RX as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable ABS_RX");
            }
            if libc::ioctl(fd, UI_SET_ABSBIT, ABS_RY as libc::c_ulong) < 0 {
                anyhow::bail!("Failed to enable ABS_RY");
            }

            // Set device name
            let name = CString::new("R-ShareMouse Virtual Gamepad").unwrap();
            let mut setup: libc::input_id = std::mem::zeroed();
            setup.bustype = BUS_VIRTUAL as u16;
            setup.vendor = 0x1234;
            setup.product = 0x567A;
            setup.version = 1;

            let mut uinput_setup = uinput_setup {
                id: setup,
                name: [0; 80],
                ff_effects_max: 0,
            };

            let name_bytes = name.as_bytes_with_nul();
            uinput_setup.name[..name_bytes.len()]
                .copy_from_slice(unsafe { std::mem::transmute::<&[u8], &[i8]>(name_bytes) });

            if libc::ioctl(fd, UI_DEV_SETUP, &uinput_setup) < 0 {
                anyhow::bail!("Failed to setup uinput device");
            }
        }

        Ok(())
    }

    /// Send a key event
    pub fn send_key(&self, keycode: u16, press: bool) -> Result<()> {
        if !self.active.load(Ordering::Relaxed) {
            return Ok(());
        }

        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            let mut event = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: EV_KEY as u16,
                code: keycode,
                value: if press { 1 } else { 0 },
            };

            unsafe {
                let ret = libc::write(
                    file.as_raw_fd(),
                    &event as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::input_event>(),
                );
                if ret < 0 {
                    anyhow::bail!("Failed to write key event");
                }
            }

            self.sync()?;
        }

        Ok(())
    }

    /// Send a mouse move event (relative)
    pub fn send_mouse_move(&self, dx: i32, dy: i32) -> Result<()> {
        if !self.active.load(Ordering::Relaxed) {
            return Ok(());
        }

        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            // X axis
            if dx != 0 {
                let mut event = libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: EV_REL as u16,
                    code: REL_X as u16,
                    value: dx,
                };

                unsafe {
                    libc::write(
                        file.as_raw_fd(),
                        &event as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::input_event>(),
                    );
                }
            }

            // Y axis
            if dy != 0 {
                let mut event = libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: EV_REL as u16,
                    code: REL_Y as u16,
                    value: dy,
                };

                unsafe {
                    libc::write(
                        file.as_raw_fd(),
                        &event as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::input_event>(),
                    );
                }
            }

            self.sync()?;
        }

        Ok(())
    }

    /// Send a mouse move event (absolute)
    pub fn send_mouse_move_absolute(&self, x: u32, y: u32) -> Result<()> {
        if !self.active.load(Ordering::Relaxed) {
            return Ok(());
        }

        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            // X axis
            let mut event_x = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: EV_ABS as u16,
                code: ABS_X as u16,
                value: x as i32,
            };

            // Y axis
            let mut event_y = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: EV_ABS as u16,
                code: ABS_Y as u16,
                value: y as i32,
            };

            unsafe {
                libc::write(
                    file.as_raw_fd(),
                    &event_x as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::input_event>(),
                );
                libc::write(
                    file.as_raw_fd(),
                    &event_y as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::input_event>(),
                );
            }

            self.sync()?;
        }

        Ok(())
    }

    /// Send a mouse button event
    pub fn send_mouse_button(&self, button: u16, press: bool) -> Result<()> {
        if !self.active.load(Ordering::Relaxed) {
            return Ok(());
        }

        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            let mut event = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: EV_KEY as u16,
                code: button,
                value: if press { 1 } else { 0 },
            };

            unsafe {
                libc::write(
                    file.as_raw_fd(),
                    &event as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::input_event>(),
                );
            }

            self.sync()?;
        }

        Ok(())
    }

    /// Send a mouse wheel event
    pub fn send_mouse_wheel(&self, delta_x: i32, delta_y: i32) -> Result<()> {
        if !self.active.load(Ordering::Relaxed) {
            return Ok(());
        }

        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            // Vertical wheel
            if delta_y != 0 {
                let mut event = libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: EV_REL as u16,
                    code: REL_WHEEL as u16,
                    value: delta_y,
                };

                unsafe {
                    libc::write(
                        file.as_raw_fd(),
                        &event as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::input_event>(),
                    );
                }
            }

            // Horizontal wheel
            if delta_x != 0 {
                let mut event = libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: EV_REL as u16,
                    code: REL_HWHEEL as u16,
                    value: delta_x,
                };

                unsafe {
                    libc::write(
                        file.as_raw_fd(),
                        &event as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::input_event>(),
                    );
                }
            }

            self.sync()?;
        }

        Ok(())
    }

    /// Sync the device to send events
    fn sync(&self) -> Result<()> {
        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            let mut event = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: EV_SYN as u16,
                code: SYN_REPORT as u16,
                value: 0,
            };

            unsafe {
                let ret = libc::write(
                    file.as_raw_fd(),
                    &event as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::input_event>(),
                );
                if ret < 0 {
                    anyhow::bail!("Failed to sync uinput device");
                }
            }
        }

        Ok(())
    }

    /// Check if device is active
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Get device type
    pub fn device_type(&self) -> DeviceType {
        self.device_type
    }
}

impl Drop for UInputDevice {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Relaxed);

        #[cfg(feature = "x11")]
        if let Some(file) = &self.file {
            unsafe {
                // Destroy the device
                libc::ioctl(file.as_raw_fd(), UI_DEV_DESTROY, 0);
            }
        }
    }
}

// ============================================================================
// Input Event Codes (from linux/input-event-codes.h)
// ============================================================================

mod input_event_codes {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
    pub const EV_FF: u16 = 0x15;

    pub const SYN_REPORT: u16 = 0x00;

    pub const KEY_ESC: u16 = 1;
    pub const KEY_1: u16 = 2;
    pub const KEY_2: u16 = 3;
    pub const KEY_3: u16 = 4;
    pub const KEY_4: u16 = 5;
    pub const KEY_5: u16 = 6;
    pub const KEY_6: u16 = 7;
    pub const KEY_7: u16 = 8;
    pub const KEY_8: u16 = 9;
    pub const KEY_9: u16 = 10;
    pub const KEY_0: u16 = 11;
    pub const KEY_PAUSE: u16 = 119;

    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
    pub const BTN_SIDE: u16 = 0x113;
    pub const BTN_EXTRA: u16 = 0x114;
    pub const BTN_FORWARD: u16 = 0x115;
    pub const BTN_BACK: u16 = 0x116;

    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_WHEEL: u16 = 0x08;
    pub const REL_HWHEEL: u16 = 0x06;

    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
    pub const ABS_Z: u16 = 0x02;
    pub const ABS_RX: u16 = 0x03;
    pub const ABS_RY: u16 = 0x04;

    pub const BTN_GAMEPAD: u16 = 0x130;
    pub const BTN_DEAD: u16 = 0x13f;

    pub const BUS_VIRTUAL: u16 = 0x06;
}

// ============================================================================
// UInput IOCTL Constants
// ============================================================================

#[cfg(feature = "x11")]
const UI_DEV_CREATE: u64 = 0x5501;
#[cfg(feature = "x11")]
const UI_DEV_DESTROY: u64 = 0x5502;
#[cfg(feature = "x11")]
const UI_DEV_SETUP: u64 = 0x5503;

#[cfg(feature = "x11")]
const UI_SET_EVBIT: u64 = 0x40045564;
#[cfg(feature = "x11")]
const UI_SET_KEYBIT: u64 = 0x40045565;
#[cfg(feature = "x11")]
const UI_SET_RELBIT: u64 = 0x40045566;
#[cfg(feature = "x11")]
const UI_SET_ABSBIT: u64 = 0x40045567;

#[repr(C)]
#[cfg(feature = "x11")]
struct uinput_setup {
    id: libc::input_id,
    name: [i8; 80],
    ff_effects_max: u32,
}

// ============================================================================
// Unified Virtual Input Device
// ============================================================================

/// Unified virtual input device manager
pub struct VirtualInputDevice {
    keyboard: Option<UInputDevice>,
    mouse: Option<UInputDevice>,
    gamepad: Option<UInputDevice>,
}

impl VirtualInputDevice {
    /// Create a new virtual input device manager
    pub fn new() -> Result<Self> {
        Ok(Self {
            keyboard: None,
            mouse: None,
            gamepad: None,
        })
    }

    /// Create and initialize virtual keyboard
    pub fn init_keyboard(&mut self) -> Result<()> {
        let keyboard = UInputDevice::new_keyboard()?;
        self.keyboard = Some(keyboard);
        Ok(())
    }

    /// Create and initialize virtual mouse
    pub fn init_mouse(&mut self) -> Result<()> {
        let mouse = UInputDevice::new_mouse()?;
        self.mouse = Some(mouse);
        Ok(())
    }

    /// Create and initialize virtual gamepad
    pub fn init_gamepad(&mut self) -> Result<()> {
        let gamepad = UInputDevice::new_gamepad()?;
        self.gamepad = Some(gamepad);
        Ok(())
    }

    /// Initialize all devices
    pub fn init_all(&mut self) -> Result<()> {
        self.init_keyboard()?;
        self.init_mouse()?;
        self.init_gamepad()?;
        Ok(())
    }

    /// Send key event
    pub fn send_key(&self, keycode: u16, press: bool) -> Result<()> {
        if let Some(keyboard) = &self.keyboard {
            keyboard.send_key(keycode, press)?;
        }
        Ok(())
    }

    /// Send mouse move event
    pub fn send_mouse_move(&self, dx: i32, dy: i32) -> Result<()> {
        if let Some(mouse) = &self.mouse {
            mouse.send_mouse_move(dx, dy)?;
        }
        Ok(())
    }

    /// Send mouse move absolute event
    pub fn send_mouse_move_absolute(&self, x: u32, y: u32) -> Result<()> {
        if let Some(mouse) = &self.mouse {
            mouse.send_mouse_move_absolute(x, y)?;
        }
        Ok(())
    }

    /// Send mouse button event
    pub fn send_mouse_button(&self, button: u16, press: bool) -> Result<()> {
        if let Some(mouse) = &self.mouse {
            mouse.send_mouse_button(button, press)?;
        }
        Ok(())
    }

    /// Send mouse wheel event
    pub fn send_mouse_wheel(&self, delta_x: i32, delta_y: i32) -> Result<()> {
        if let Some(mouse) = &self.mouse {
            mouse.send_mouse_wheel(delta_x, delta_y)?;
        }
        Ok(())
    }

    /// Check if any device is active
    pub fn is_active(&self) -> bool {
        self.keyboard
            .as_ref()
            .map(|d| d.is_active())
            .unwrap_or(false)
            || self.mouse.as_ref().map(|d| d.is_active()).unwrap_or(false)
            || self
                .gamepad
                .as_ref()
                .map(|d| d.is_active())
                .unwrap_or(false)
    }
}

impl Default for VirtualInputDevice {
    fn default() -> Self {
        Self::new().expect("Failed to create VirtualInputDevice")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_type_display() {
        assert_eq!(format!("{:?}", DeviceType::Keyboard), "Keyboard");
        assert_eq!(format!("{:?}", DeviceType::Mouse), "Mouse");
        assert_eq!(format!("{:?}", DeviceType::Gamepad), "Gamepad");
    }

    #[test]
    fn test_virtual_input_device_default() {
        let device = VirtualInputDevice::default();
        assert!(!device.is_active());
    }
}
