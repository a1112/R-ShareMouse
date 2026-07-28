//! Message encoding and decoding

use anyhow::Result;
use bytes::{Buf, BufMut, BytesMut};
use rshare_core::{GamepadButton, GamepadButtonState, GamepadState, Message};
use std::time::{SystemTime, UNIX_EPOCH};

/// Message frame format
///
/// Frame structure:
/// - 4 bytes: Message length (u32 big-endian)
/// - 1 byte: Message type tag
/// - N bytes: Message payload
///
/// This allows for efficient streaming and future protocol extensions.

const FRAME_HEADER_SIZE: usize = 5; // length (4) + type (1)

/// Compact realtime datagram header size.
///
/// Header layout:
/// - version: u8
/// - message_type: u8
/// - flags: u8
/// - payload_len: u16
/// - seq: u32
/// - timestamp_us: u64
pub const REALTIME_FRAME_HEADER_SIZE: usize = 17;
pub const REALTIME_PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RealtimeMessageType {
    MouseMove = 1,
    GamepadState = 2,
}

impl RealtimeMessageType {
    fn decode(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::MouseMove),
            2 => Ok(Self::GamepadState),
            other => anyhow::bail!("Unknown realtime message type: {other}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeFrame {
    pub version: u8,
    pub message_type: RealtimeMessageType,
    pub flags: u8,
    pub payload_len: u16,
    pub seq: u32,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
}

impl RealtimeFrame {
    pub fn new(
        message_type: RealtimeMessageType,
        flags: u8,
        seq: u32,
        timestamp_us: u64,
        payload: Vec<u8>,
    ) -> Result<Self> {
        let payload_len = u16::try_from(payload.len())
            .map_err(|_| anyhow::anyhow!("Realtime payload too large: {} bytes", payload.len()))?;
        Ok(Self {
            version: REALTIME_PROTOCOL_VERSION,
            message_type,
            flags,
            payload_len,
            seq,
            timestamp_us,
            payload,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut frame = BytesMut::with_capacity(REALTIME_FRAME_HEADER_SIZE + self.payload.len());
        frame.put_u8(self.version);
        frame.put_u8(self.message_type as u8);
        frame.put_u8(self.flags);
        frame.put_u16(self.payload_len);
        frame.put_u32(self.seq);
        frame.put_u64(self.timestamp_us);
        frame.put_slice(&self.payload);
        frame.to_vec()
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < REALTIME_FRAME_HEADER_SIZE {
            anyhow::bail!("Realtime frame too short: {} bytes", data.len());
        }

        let version = data[0];
        if version != REALTIME_PROTOCOL_VERSION {
            anyhow::bail!("Unsupported realtime frame version: {version}");
        }

        let message_type = RealtimeMessageType::decode(data[1])?;
        let flags = data[2];
        let payload_len = u16::from_be_bytes([data[3], data[4]]);
        let seq = u32::from_be_bytes([data[5], data[6], data[7], data[8]]);
        let timestamp_us = u64::from_be_bytes([
            data[9], data[10], data[11], data[12], data[13], data[14], data[15], data[16],
        ]);

        let expected_len = REALTIME_FRAME_HEADER_SIZE + payload_len as usize;
        if data.len() != expected_len {
            anyhow::bail!(
                "Realtime payload length mismatch: expected {}, got {}",
                expected_len,
                data.len()
            );
        }

        Ok(Self {
            version,
            message_type,
            flags,
            payload_len,
            seq,
            timestamp_us,
            payload: data[REALTIME_FRAME_HEADER_SIZE..].to_vec(),
        })
    }
}

pub struct RealtimeInputCodec;

impl RealtimeInputCodec {
    pub fn encode_message(seq: u32, message: &Message) -> Result<Option<Vec<u8>>> {
        let timestamp_us = timestamp_us();
        match message {
            Message::MouseMove { x, y } => {
                let mut payload = BytesMut::with_capacity(8);
                payload.put_i32(*x);
                payload.put_i32(*y);
                Ok(Some(
                    RealtimeFrame::new(
                        RealtimeMessageType::MouseMove,
                        0,
                        seq,
                        timestamp_us,
                        payload.to_vec(),
                    )?
                    .encode(),
                ))
            }
            Message::GamepadState { state } => Ok(Some(
                RealtimeFrame::new(
                    RealtimeMessageType::GamepadState,
                    0,
                    seq,
                    timestamp_us,
                    encode_gamepad_state(state)?,
                )?
                .encode(),
            )),
            _ => Ok(None),
        }
    }

    pub fn decode_message(data: &[u8]) -> Result<Message> {
        let frame = RealtimeFrame::decode(data)?;
        match frame.message_type {
            RealtimeMessageType::MouseMove => {
                if frame.payload.len() != 8 {
                    anyhow::bail!(
                        "MouseMove realtime payload must be 8 bytes, got {}",
                        frame.payload.len()
                    );
                }
                let x = i32::from_be_bytes(frame.payload[0..4].try_into()?);
                let y = i32::from_be_bytes(frame.payload[4..8].try_into()?);
                Ok(Message::MouseMove { x, y })
            }
            RealtimeMessageType::GamepadState => Ok(Message::GamepadState {
                state: decode_gamepad_state(&frame.payload)?,
            }),
        }
    }
}

pub struct ControlMessageCodec;

impl ControlMessageCodec {
    pub fn encode(message: &Message) -> Result<Vec<u8>> {
        MessageCodec::encode(message)
    }

    pub fn decode(data: &[u8]) -> Result<Message> {
        MessageCodec::decode(data)
    }
}

fn timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

fn encode_gamepad_state(state: &GamepadState) -> Result<Vec<u8>> {
    let button_count = u16::try_from(state.buttons.len())
        .map_err(|_| anyhow::anyhow!("Too many gamepad buttons: {}", state.buttons.len()))?;
    let mut payload = BytesMut::with_capacity(1 + 8 + 8 + 8 + 4 + 2 + state.buttons.len() * 3);
    payload.put_u8(state.gamepad_id);
    payload.put_u64(state.sequence);
    payload.put_u64(state.timestamp_ms);
    payload.put_i16(state.left_stick_x);
    payload.put_i16(state.left_stick_y);
    payload.put_i16(state.right_stick_x);
    payload.put_i16(state.right_stick_y);
    payload.put_u16(state.left_trigger);
    payload.put_u16(state.right_trigger);
    payload.put_u16(button_count);
    for button in &state.buttons {
        payload.put_u16(encode_gamepad_button(button.button));
        payload.put_u8(u8::from(button.pressed));
    }
    Ok(payload.to_vec())
}

fn decode_gamepad_state(data: &[u8]) -> Result<GamepadState> {
    const FIXED_LEN: usize = 1 + 8 + 8 + 8 + 4 + 2;
    if data.len() < FIXED_LEN {
        anyhow::bail!(
            "GamepadState realtime payload too short: {} bytes",
            data.len()
        );
    }
    let mut bytes = BytesMut::from(data);
    let gamepad_id = bytes.get_u8();
    let sequence = bytes.get_u64();
    let timestamp_ms = bytes.get_u64();
    let left_stick_x = bytes.get_i16();
    let left_stick_y = bytes.get_i16();
    let right_stick_x = bytes.get_i16();
    let right_stick_y = bytes.get_i16();
    let left_trigger = bytes.get_u16();
    let right_trigger = bytes.get_u16();
    let button_count = bytes.get_u16() as usize;
    if bytes.len() != button_count * 3 {
        anyhow::bail!(
            "GamepadState button payload length mismatch: {} bytes for {} buttons",
            bytes.len(),
            button_count
        );
    }
    let mut buttons = Vec::with_capacity(button_count);
    for _ in 0..button_count {
        let button = decode_gamepad_button(bytes.get_u16());
        let pressed = bytes.get_u8() != 0;
        buttons.push(GamepadButtonState { button, pressed });
    }
    Ok(GamepadState {
        gamepad_id,
        sequence,
        buttons,
        left_stick_x,
        left_stick_y,
        right_stick_x,
        right_stick_y,
        left_trigger,
        right_trigger,
        timestamp_ms,
    })
}

fn encode_gamepad_button(button: GamepadButton) -> u16 {
    match button {
        GamepadButton::South => 0,
        GamepadButton::East => 1,
        GamepadButton::West => 2,
        GamepadButton::North => 3,
        GamepadButton::LeftBumper => 4,
        GamepadButton::RightBumper => 5,
        GamepadButton::LeftTrigger => 6,
        GamepadButton::RightTrigger => 7,
        GamepadButton::Select => 8,
        GamepadButton::Start => 9,
        GamepadButton::Guide => 10,
        GamepadButton::LeftStick => 11,
        GamepadButton::RightStick => 12,
        GamepadButton::DPadUp => 13,
        GamepadButton::DPadDown => 14,
        GamepadButton::DPadLeft => 15,
        GamepadButton::DPadRight => 16,
        GamepadButton::Other(code) => 0x8000 | code,
    }
}

fn decode_gamepad_button(code: u16) -> GamepadButton {
    match code {
        0 => GamepadButton::South,
        1 => GamepadButton::East,
        2 => GamepadButton::West,
        3 => GamepadButton::North,
        4 => GamepadButton::LeftBumper,
        5 => GamepadButton::RightBumper,
        6 => GamepadButton::LeftTrigger,
        7 => GamepadButton::RightTrigger,
        8 => GamepadButton::Select,
        9 => GamepadButton::Start,
        10 => GamepadButton::Guide,
        11 => GamepadButton::LeftStick,
        12 => GamepadButton::RightStick,
        13 => GamepadButton::DPadUp,
        14 => GamepadButton::DPadDown,
        15 => GamepadButton::DPadLeft,
        16 => GamepadButton::DPadRight,
        other if other & 0x8000 != 0 => GamepadButton::Other(other & 0x7fff),
        other => GamepadButton::Other(other),
    }
}

/// Message codec for encoding/decoding messages
pub struct MessageCodec;

impl MessageCodec {
    /// Maximum message size (10 MB)
    const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

    /// Encode a message to bytes with frame header
    pub fn encode(message: &Message) -> Result<Vec<u8>> {
        let payload = serde_json::to_vec(message)
            .map_err(|e| anyhow::anyhow!("Serialization error: {}", e))?;

        if payload.len() > Self::MAX_MESSAGE_SIZE {
            anyhow::bail!("Message too large: {} bytes", payload.len());
        }

        let mut frame = BytesMut::with_capacity(FRAME_HEADER_SIZE + payload.len());

        // Write length (excluding the length field itself)
        frame.put_u32((1 + payload.len()) as u32);

        // Write message type tag (for future extensibility)
        frame.put_u8(Self::message_type_tag(message));

        // Write payload
        frame.put_slice(&payload);

        Ok(frame.to_vec())
    }

    /// Decode a message from bytes (with frame header)
    pub fn decode(data: &[u8]) -> Result<Message> {
        if data.len() < FRAME_HEADER_SIZE {
            anyhow::bail!("Frame too short: {} bytes", data.len());
        }

        let mut frame = BytesMut::from(data);

        // Read length
        let frame_len = frame.get_u32() as usize;
        if frame_len != data.len() - 4 {
            anyhow::bail!(
                "Frame length mismatch: expected {}, got {}",
                frame_len,
                data.len() - 4
            );
        }

        // Read and verify type tag
        let type_tag = frame.get_u8();
        let _ = type_tag; // For future use

        // Read and decode payload
        let payload = &data[FRAME_HEADER_SIZE..];
        serde_json::from_slice(payload).map_err(|e| anyhow::anyhow!("Deserialization error: {}", e))
    }

    /// Encode a message without frame header
    pub fn encode_raw(message: &Message) -> Result<Vec<u8>> {
        serde_json::to_vec(message).map_err(|e| anyhow::anyhow!("Serialization error: {}", e))
    }

    /// Decode a message without frame header
    pub fn decode_raw(data: &[u8]) -> Result<Message> {
        serde_json::from_slice(data).map_err(|e| anyhow::anyhow!("Deserialization error: {}", e))
    }

    /// Get the type tag for a message
    fn message_type_tag(message: &Message) -> u8 {
        match message {
            // Discovery (0-9)
            Message::Hello { .. } => 0,
            Message::HelloBack { .. } => 1,
            Message::Goodbye { .. } => 2,
            Message::HelloRejected { .. } => 3,

            // Input events (10-19)
            Message::MouseMove { .. } => 10,
            Message::MouseButton { .. } => 11,
            Message::MouseWheel { .. } => 12,
            Message::Key { .. } => 13,
            Message::KeyExtended { .. } => 14,
            Message::GamepadConnected { .. } => 15,
            Message::GamepadDisconnected { .. } => 16,
            Message::GamepadState { .. } => 17,
            Message::InputDiagnostic { .. } => 25,
            Message::LatencyProbe { .. } => 26,
            Message::LatencyProbeAck { .. } => 27,
            Message::EndpointEventSubscribe { .. } => 28,
            Message::EndpointEventSnapshot { .. } => 29,
            Message::EndpointEventDelta { .. } => 33,
            Message::EndpointInjectRequest { .. } => 34,
            Message::EndpointInjectResult { .. } => 35,
            Message::AudioStreamStart { .. } => 18,
            Message::AudioFrame { .. } => 19,
            Message::AudioStreamStop { .. } => 23,
            Message::AudioStreamError { .. } => 24,

            // Clipboard (20-29)
            Message::ClipboardData { .. } => 20,
            Message::ClipboardRequest => 21,
            Message::ClipboardResponse { .. } => 22,

            // Screen control (30-39)
            Message::ScreenEnter { .. } => 30,
            Message::ScreenLeave { .. } => 31,
            Message::ScreenUpdate { .. } => 32,

            // Synchronization (40-49)
            Message::Heartbeat { .. } => 40,
            Message::Ack { .. } => 41,
            Message::Error { .. } => 42,

            // Experimental USB forwarding (50-69)
            Message::UsbDeviceAttached { .. } => 50,
            Message::UsbDeviceDetached { .. } => 51,
            Message::UsbTransfer { .. } => 52,
            Message::UsbTransferComplete { .. } => 53,
            Message::UsbForwardingError { .. } => 54,
            Message::UsbDeviceClaimRequest { .. } => 55,
            Message::UsbDeviceClaimResponse { .. } => 56,
            Message::UsbDeviceRelease { .. } => 57,
            Message::UsbDeviceReset { .. } => 58,
            Message::UsbTransferCancel { .. } => 59,
            Message::UsbFlowControl { .. } => 60,
        }
    }

    /// Create a framed message for sending
    pub fn frame_message(message: &Message) -> Result<BytesMut> {
        let payload = Self::encode_raw(message)?;
        let mut frame = BytesMut::with_capacity(4 + payload.len());

        frame.put_u32(payload.len() as u32);
        frame.put_slice(&payload);

        Ok(frame)
    }
}

/// Streaming message decoder for use with tokio
pub struct MessageDecoder {
    buffer: BytesMut,
    max_message_size: usize,
}

impl MessageDecoder {
    /// Create a new decoder
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::with_capacity(8192),
            max_message_size: MessageCodec::MAX_MESSAGE_SIZE,
        }
    }

    /// Set the maximum message size
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_message_size = max_size;
        self
    }

    /// Feed data into the decoder
    pub fn feed(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    /// Try to decode a complete message
    pub fn try_decode(&mut self) -> Result<Option<Message>> {
        // Need at least 4 bytes for length
        if self.buffer.len() < 4 {
            return Ok(None);
        }

        // Peek at the length
        let len = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;

        // Validate length
        if len > self.max_message_size {
            anyhow::bail!("Message too large: {} bytes", len);
        }

        // Check if we have the complete message
        if self.buffer.len() < 4 + len {
            return Ok(None);
        }

        // Extract the message data
        let data = self.buffer[4..4 + len].to_vec();
        self.buffer.advance(4 + len);

        // Decode
        let message = MessageCodec::decode_raw(&data)?;
        Ok(Some(message))
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Get the number of bytes buffered
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }
}

impl Default for MessageDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{
        hello_message, DeviceId, GamepadState, UsbDeviceClaimRequest, UsbTransferDirection,
        UsbTransferKind, UsbTransferPayload,
    };

    #[test]
    fn test_encode_decode() {
        let msg = hello_message(
            DeviceId::new_v4(),
            "Test".to_string(),
            "test-host".to_string(),
        );

        let encoded = MessageCodec::encode(&msg).unwrap();
        let decoded = MessageCodec::decode(&encoded).unwrap();

        match decoded {
            Message::Hello { device_name, .. } => {
                assert_eq!(device_name, "Test");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_raw_encode_decode() {
        let msg = Message::MouseMove { x: 100, y: 200 };

        let encoded = MessageCodec::encode_raw(&msg).unwrap();
        let decoded = MessageCodec::decode_raw(&encoded).unwrap();

        match decoded {
            Message::MouseMove { x, y } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decoder_streaming() {
        let msg1 = Message::MouseMove { x: 100, y: 200 };
        let msg2 = Message::MouseMove { x: 300, y: 400 };

        let enc1 = MessageCodec::frame_message(&msg1).unwrap();
        let enc2 = MessageCodec::frame_message(&msg2).unwrap();

        let mut decoder = MessageDecoder::new();

        // Feed partial data
        decoder.feed(&enc1[..5]);
        assert!(decoder.try_decode().unwrap().is_none());

        // Feed rest of first message
        decoder.feed(&enc1[5..]);
        let dec1 = decoder.try_decode().unwrap().unwrap();
        match dec1 {
            Message::MouseMove { x, y } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
            }
            _ => panic!("Wrong message type"),
        }

        // Feed second message
        decoder.feed(&enc2[..]);
        let dec2 = decoder.try_decode().unwrap().unwrap();
        match dec2 {
            Message::MouseMove { x, y } => {
                assert_eq!(x, 300);
                assert_eq!(y, 400);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_message_type_tags() {
        assert_eq!(
            MessageCodec::message_type_tag(&Message::Hello {
                app_id: rshare_core::DISCOVERY_APP_ID.to_string(),
                device_id: DeviceId::new_v4(),
                device_name: String::new(),
                hostname: String::new(),
                protocol_version: 1,
                capabilities: Default::default(),
                transport_capabilities: Default::default(),
            }),
            0
        );
        assert_eq!(
            MessageCodec::message_type_tag(&Message::HelloRejected {
                app_id: rshare_core::DISCOVERY_APP_ID.to_string(),
                device_id: DeviceId::new_v4(),
                reason: rshare_core::HandshakeRejectReason::ApplicationMismatch,
            }),
            3
        );

        assert_eq!(
            MessageCodec::message_type_tag(&Message::MouseMove { x: 0, y: 0 }),
            10
        );

        assert_eq!(
            MessageCodec::message_type_tag(&Message::GamepadState {
                state: GamepadState::neutral(0, 1, 123),
            }),
            17
        );

        assert_eq!(
            MessageCodec::message_type_tag(&Message::UsbTransfer {
                transfer: UsbTransferPayload {
                    transfer_id: 1,
                    bus_id: "usb:1-2".to_string(),
                    session_id: None,
                    endpoint_address: 0x81,
                    transfer_kind: UsbTransferKind::Interrupt,
                    direction: UsbTransferDirection::In,
                    setup_packet: None,
                    control_setup: None,
                    stream_id: None,
                    expected_length: Some(64),
                    flags: Vec::new(),
                    iso_packets: Vec::new(),
                    data: Vec::new(),
                    timeout_ms: 100,
                },
            }),
            52
        );

        assert_eq!(
            MessageCodec::message_type_tag(&Message::UsbDeviceClaimRequest {
                request: UsbDeviceClaimRequest {
                    request_id: 1,
                    bus_id: "usb:1-2".to_string(),
                    exclusive: true,
                    configuration_value: Some(1),
                    interface_numbers: vec![0],
                },
            }),
            55
        );
    }

    #[test]
    fn hello_rejected_uses_reserved_tag_and_round_trips() {
        let message = Message::HelloRejected {
            app_id: rshare_core::DISCOVERY_APP_ID.to_string(),
            device_id: DeviceId::new_v4(),
            reason: rshare_core::HandshakeRejectReason::ProtocolMismatch {
                required: rshare_core::PROTOCOL_VERSION,
                received: 2,
            },
        };

        assert_eq!(MessageCodec::message_type_tag(&message), 3);
        let encoded = MessageCodec::encode(&message).unwrap();
        assert!(matches!(
            MessageCodec::decode(&encoded).unwrap(),
            Message::HelloRejected {
                reason: rshare_core::HandshakeRejectReason::ProtocolMismatch {
                    required: 3,
                    received: 2
                },
                ..
            }
        ));
    }

    #[test]
    fn test_gamepad_state_raw_encode_decode() {
        let msg = Message::GamepadState {
            state: GamepadState::neutral(0, 7, 456),
        };

        let encoded = MessageCodec::encode_raw(&msg).unwrap();
        let decoded = MessageCodec::decode_raw(&encoded).unwrap();

        assert!(matches!(
            decoded,
            Message::GamepadState {
                state: GamepadState {
                    gamepad_id: 0,
                    sequence: 7,
                    timestamp_ms: 456,
                    ..
                }
            }
        ));
    }

    #[test]
    fn test_max_message_size() {
        let msg = Message::ClipboardData {
            mime_type: "text/plain".to_string(),
            data: vec![0u8; 100],
        };

        let result = MessageCodec::encode(&msg);
        assert!(result.is_ok());
    }

    #[test]
    fn realtime_mouse_move_encode_decode() {
        let encoded = RealtimeInputCodec::encode_message(7, &Message::MouseMove { x: 123, y: -45 })
            .unwrap()
            .unwrap();
        let frame = RealtimeFrame::decode(&encoded).unwrap();
        assert_eq!(frame.version, REALTIME_PROTOCOL_VERSION);
        assert_eq!(frame.message_type, RealtimeMessageType::MouseMove);
        assert_eq!(frame.payload_len, 8);
        assert_eq!(frame.seq, 7);

        let decoded = RealtimeInputCodec::decode_message(&encoded).unwrap();
        assert!(matches!(decoded, Message::MouseMove { x: 123, y: -45 }));
    }

    #[test]
    fn realtime_gamepad_state_encode_decode() {
        let mut state = GamepadState::neutral(2, 99, 1234);
        state.left_stick_x = -12;
        state.right_trigger = 500;
        state.buttons.push(GamepadButtonState {
            button: GamepadButton::South,
            pressed: true,
        });
        state.buttons.push(GamepadButtonState {
            button: GamepadButton::Other(42),
            pressed: false,
        });

        let encoded = RealtimeInputCodec::encode_message(9, &Message::GamepadState { state })
            .unwrap()
            .unwrap();
        let decoded = RealtimeInputCodec::decode_message(&encoded).unwrap();

        match decoded {
            Message::GamepadState { state } => {
                assert_eq!(state.gamepad_id, 2);
                assert_eq!(state.sequence, 99);
                assert_eq!(state.left_stick_x, -12);
                assert_eq!(state.right_trigger, 500);
                assert_eq!(state.buttons.len(), 2);
                assert_eq!(state.buttons[0].button, GamepadButton::South);
                assert!(state.buttons[0].pressed);
                assert_eq!(state.buttons[1].button, GamepadButton::Other(42));
                assert!(!state.buttons[1].pressed);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn realtime_decode_rejects_unknown_version_and_type() {
        let mut encoded = RealtimeInputCodec::encode_message(1, &Message::MouseMove { x: 1, y: 2 })
            .unwrap()
            .unwrap();
        encoded[0] = REALTIME_PROTOCOL_VERSION + 1;
        assert!(RealtimeFrame::decode(&encoded).is_err());

        let mut encoded = RealtimeInputCodec::encode_message(1, &Message::MouseMove { x: 1, y: 2 })
            .unwrap()
            .unwrap();
        encoded[1] = 99;
        assert!(RealtimeFrame::decode(&encoded).is_err());
    }

    #[test]
    fn realtime_decode_rejects_payload_length_mismatch() {
        let mut encoded = RealtimeInputCodec::encode_message(1, &Message::MouseMove { x: 1, y: 2 })
            .unwrap()
            .unwrap();
        encoded.pop();
        assert!(RealtimeFrame::decode(&encoded).is_err());
    }
}
