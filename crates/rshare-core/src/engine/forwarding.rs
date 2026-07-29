//! Local capture-domain event types.
//!
//! Peer forwarding is owned by the typed input router and QoS lanes. This
//! module deliberately contains no conversion to the general peer `Message`
//! protocol.

use crate::{GamepadDeviceInfo, GamepadState};

/// Raw input event produced by local capture adapters.
#[derive(Debug, Clone)]
pub enum RawInputEvent {
    MouseMove {
        x: i32,
        y: i32,
    },
    MouseButton {
        button: u8,
        pressed: bool,
    },
    MouseWheel {
        delta_x: i32,
        delta_y: i32,
    },
    Key {
        keycode: u32,
        pressed: bool,
    },
    KeyExtended {
        keycode: u32,
        pressed: bool,
        shift: bool,
        ctrl: bool,
        alt: bool,
        meta: bool,
    },
    GamepadConnected {
        info: GamepadDeviceInfo,
    },
    GamepadDisconnected {
        gamepad_id: u8,
    },
    GamepadState {
        state: GamepadState,
    },
}
