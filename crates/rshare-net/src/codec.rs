//! Message encoding and decoding

use anyhow::Result;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use rshare_core::{
    ClockDomainId, Message, MonotonicStamp, RealtimeInputFrame, RealtimeInputPayload,
    ReliableInputEvent, ReliableInputFrame, SessionEpoch, INPUT_PROTOCOL_VERSION,
};

/// Message frame format
///
/// Frame structure:
/// - 4 bytes: Message length (u32 big-endian)
/// - 1 byte: Message type tag
/// - N bytes: Message payload
///
/// This allows for efficient streaming and future protocol extensions.
const FRAME_HEADER_SIZE: usize = 5; // length (4) + type (1)

const REALTIME_INPUT_HEADER_SIZE: usize = 37;
const RELATIVE_MOUSE_PAYLOAD_SIZE: u16 = 8;
const ABSOLUTE_ANCHOR_PAYLOAD_SIZE: u16 = 8;
const GAMEPAD_AXES_PAYLOAD_SIZE: u16 = 13;
const CURSOR_VISUAL_PAYLOAD_SIZE: u16 = 9;
pub const MAX_RELIABLE_INPUT_FRAME: usize = 4 * 1024;
// Reserve space for fixed frame metadata, the event tag, and the string length prefix.
const MAX_TEXT_COMMIT_BYTES: usize = MAX_RELIABLE_INPUT_FRAME - 64;

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum RealtimeInputKind {
    RelativeMouse = 1,
    AbsoluteAnchor = 2,
    GamepadAxes = 3,
    CursorVisual = 4,
}

impl RealtimeInputKind {
    fn decode(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::RelativeMouse),
            2 => Ok(Self::AbsoluteAnchor),
            3 => Ok(Self::GamepadAxes),
            4 => Ok(Self::CursorVisual),
            other => anyhow::bail!("unknown realtime input kind: {other}"),
        }
    }

    const fn payload_size(self) -> u16 {
        match self {
            Self::RelativeMouse => RELATIVE_MOUSE_PAYLOAD_SIZE,
            Self::AbsoluteAnchor => ABSOLUTE_ANCHOR_PAYLOAD_SIZE,
            Self::GamepadAxes => GAMEPAD_AXES_PAYLOAD_SIZE,
            Self::CursorVisual => CURSOR_VISUAL_PAYLOAD_SIZE,
        }
    }
}

pub struct RealtimeInputCodec;

impl RealtimeInputCodec {
    pub fn encode(frame: &RealtimeInputFrame) -> Result<Bytes> {
        if frame.protocol_version != INPUT_PROTOCOL_VERSION {
            anyhow::bail!(
                "unsupported realtime input protocol version: {}",
                frame.protocol_version
            );
        }

        let (kind, payload_size) = match frame.payload {
            RealtimeInputPayload::RelativeMouse { .. } => (
                RealtimeInputKind::RelativeMouse,
                RELATIVE_MOUSE_PAYLOAD_SIZE,
            ),
            RealtimeInputPayload::AbsoluteAnchor { .. } => (
                RealtimeInputKind::AbsoluteAnchor,
                ABSOLUTE_ANCHOR_PAYLOAD_SIZE,
            ),
            RealtimeInputPayload::GamepadAxes { .. } => {
                (RealtimeInputKind::GamepadAxes, GAMEPAD_AXES_PAYLOAD_SIZE)
            }
            RealtimeInputPayload::CursorVisual { .. } => {
                (RealtimeInputKind::CursorVisual, CURSOR_VISUAL_PAYLOAD_SIZE)
            }
        };
        let total_size = REALTIME_INPUT_HEADER_SIZE
            .checked_add(usize::from(payload_size))
            .ok_or_else(|| anyhow::anyhow!("realtime input frame length overflow"))?;
        let mut encoded = BytesMut::with_capacity(total_size);
        encoded.put_u16(frame.protocol_version);
        encoded.put_u8(kind as u8);
        encoded.put_u16(payload_size);
        encoded.put_u64(frame.session_epoch.0);
        encoded.put_u64(frame.sequence);
        encoded.put_u64(frame.captured_at.domain.0);
        encoded.put_u64(frame.captured_at.value_us);

        match frame.payload {
            RealtimeInputPayload::RelativeMouse { dx, dy } => {
                encoded.put_i32(dx);
                encoded.put_i32(dy);
            }
            RealtimeInputPayload::AbsoluteAnchor { x, y } => {
                encoded.put_i32(x);
                encoded.put_i32(y);
            }
            RealtimeInputPayload::GamepadAxes {
                gamepad_id,
                left_stick_x,
                left_stick_y,
                right_stick_x,
                right_stick_y,
                left_trigger,
                right_trigger,
            } => {
                encoded.put_u8(gamepad_id);
                encoded.put_i16(left_stick_x);
                encoded.put_i16(left_stick_y);
                encoded.put_i16(right_stick_x);
                encoded.put_i16(right_stick_y);
                encoded.put_u16(left_trigger);
                encoded.put_u16(right_trigger);
            }
            RealtimeInputPayload::CursorVisual { x, y, visible } => {
                encoded.put_i32(x);
                encoded.put_i32(y);
                encoded.put_u8(u8::from(visible));
            }
        }

        Ok(encoded.freeze())
    }

    pub fn decode(data: &[u8]) -> Result<RealtimeInputFrame> {
        if data.len() < REALTIME_INPUT_HEADER_SIZE {
            anyhow::bail!("realtime input frame too short: {} bytes", data.len());
        }

        let protocol_version = u16::from_be_bytes(data[0..2].try_into()?);
        if protocol_version != INPUT_PROTOCOL_VERSION {
            anyhow::bail!("unsupported realtime input protocol version: {protocol_version}");
        }
        let kind = RealtimeInputKind::decode(data[2])?;
        let payload_size = u16::from_be_bytes(data[3..5].try_into()?);
        if payload_size != kind.payload_size() {
            anyhow::bail!(
                "invalid payload length {payload_size} for realtime input kind {}",
                data[2]
            );
        }
        let expected_size = REALTIME_INPUT_HEADER_SIZE
            .checked_add(usize::from(payload_size))
            .ok_or_else(|| anyhow::anyhow!("realtime input frame length overflow"))?;
        if data.len() != expected_size {
            anyhow::bail!(
                "realtime input frame length mismatch: expected {expected_size}, got {}",
                data.len()
            );
        }

        let session_epoch = u64::from_be_bytes(data[5..13].try_into()?);
        let sequence = u64::from_be_bytes(data[13..21].try_into()?);
        let captured_clock_domain = u64::from_be_bytes(data[21..29].try_into()?);
        let captured_at_us = u64::from_be_bytes(data[29..37].try_into()?);
        let payload = &data[REALTIME_INPUT_HEADER_SIZE..];
        let payload = match kind {
            RealtimeInputKind::RelativeMouse => RealtimeInputPayload::RelativeMouse {
                dx: i32::from_be_bytes(payload[0..4].try_into()?),
                dy: i32::from_be_bytes(payload[4..8].try_into()?),
            },
            RealtimeInputKind::AbsoluteAnchor => RealtimeInputPayload::AbsoluteAnchor {
                x: i32::from_be_bytes(payload[0..4].try_into()?),
                y: i32::from_be_bytes(payload[4..8].try_into()?),
            },
            RealtimeInputKind::GamepadAxes => RealtimeInputPayload::GamepadAxes {
                gamepad_id: payload[0],
                left_stick_x: i16::from_be_bytes(payload[1..3].try_into()?),
                left_stick_y: i16::from_be_bytes(payload[3..5].try_into()?),
                right_stick_x: i16::from_be_bytes(payload[5..7].try_into()?),
                right_stick_y: i16::from_be_bytes(payload[7..9].try_into()?),
                left_trigger: u16::from_be_bytes(payload[9..11].try_into()?),
                right_trigger: u16::from_be_bytes(payload[11..13].try_into()?),
            },
            RealtimeInputKind::CursorVisual => {
                let visible = match payload[8] {
                    0 => false,
                    1 => true,
                    other => anyhow::bail!(
                        "invalid cursor visibility value in realtime input frame: {other}"
                    ),
                };
                RealtimeInputPayload::CursorVisual {
                    x: i32::from_be_bytes(payload[0..4].try_into()?),
                    y: i32::from_be_bytes(payload[4..8].try_into()?),
                    visible,
                }
            }
        };

        Ok(RealtimeInputFrame {
            protocol_version,
            session_epoch: SessionEpoch(session_epoch),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(captured_clock_domain), captured_at_us),
            payload,
        })
    }
}

pub struct ReliableInputCodec;

impl ReliableInputCodec {
    pub fn encode(frame: &ReliableInputFrame) -> Result<Bytes> {
        anyhow::ensure!(
            frame.protocol_version == INPUT_PROTOCOL_VERSION,
            "unsupported reliable input body version: {}",
            frame.protocol_version
        );
        validate_text_commit_size(frame)?;

        let config = bincode::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding()
            .with_limit::<MAX_RELIABLE_INPUT_FRAME>();
        let body = bincode::serde::encode_to_vec(frame, config)?;
        anyhow::ensure!(
            body.len() <= MAX_RELIABLE_INPUT_FRAME,
            "reliable input body too large: {} bytes",
            body.len()
        );

        let mut encoded = BytesMut::with_capacity(2 + body.len());
        encoded.put_u16(INPUT_PROTOCOL_VERSION);
        encoded.extend_from_slice(&body);
        Ok(encoded.freeze())
    }

    pub fn decode(data: &[u8]) -> Result<ReliableInputFrame> {
        anyhow::ensure!(
            (2..=MAX_RELIABLE_INPUT_FRAME + 2).contains(&data.len()),
            "invalid reliable input frame length: {} bytes",
            data.len()
        );
        let outer_version = u16::from_be_bytes(data[0..2].try_into()?);
        anyhow::ensure!(
            outer_version == INPUT_PROTOCOL_VERSION,
            "unsupported reliable input outer version: {outer_version}"
        );

        let config = bincode::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding()
            .with_limit::<MAX_RELIABLE_INPUT_FRAME>();
        let (frame, consumed): (ReliableInputFrame, usize) =
            bincode::serde::decode_from_slice(&data[2..], config)?;
        anyhow::ensure!(
            consumed == data.len() - 2,
            "trailing reliable input bytes: consumed {consumed}, body length {}",
            data.len() - 2
        );
        anyhow::ensure!(
            frame.protocol_version == INPUT_PROTOCOL_VERSION,
            "reliable input prefix/body version mismatch: outer {outer_version}, body {}",
            frame.protocol_version
        );
        validate_text_commit_size(&frame)?;
        Ok(frame)
    }
}

fn validate_text_commit_size(frame: &ReliableInputFrame) -> Result<()> {
    if let ReliableInputEvent::TextCommit { text } = &frame.event {
        anyhow::ensure!(
            text.len() <= MAX_TEXT_COMMIT_BYTES,
            "reliable TextCommit exceeds {MAX_TEXT_COMMIT_BYTES} UTF-8 bytes"
        );
    }
    Ok(())
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

            // Input-adjacent control and diagnostics (10-19)
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
        hello_message, AcceptRealtime, AuthenticatedInputOwner, ButtonState, ClockDomainId,
        ControlConnectionId, DeviceId, GamepadButton, GamepadDeviceInfo, InputOwnershipGate,
        KeyState, MonotonicStamp, MouseButton, RealtimeInputFrame, RealtimeInputPayload,
        ReleaseAllReason, ReliableInputEvent, ReliableInputFrame, SessionEpoch,
        UsbDeviceClaimRequest, UsbTransferDirection, UsbTransferKind, UsbTransferPayload,
        INPUT_PROTOCOL_VERSION,
    };

    fn realtime_frame(sequence: u64, payload: RealtimeInputPayload) -> RealtimeInputFrame {
        RealtimeInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(42),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(7), 123_456),
            payload,
        }
    }

    fn reliable_frame(sequence: u64, event: ReliableInputEvent) -> ReliableInputFrame {
        ReliableInputFrame {
            protocol_version: INPUT_PROTOCOL_VERSION,
            session_epoch: SessionEpoch(4),
            sequence,
            captured_at: MonotonicStamp::new(ClockDomainId(7), 123_456),
            event,
        }
    }

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
        let msg = Message::Heartbeat {
            sequence: 100,
            timestamp: 200,
        };

        let encoded = MessageCodec::encode_raw(&msg).unwrap();
        let decoded = MessageCodec::decode_raw(&encoded).unwrap();

        match decoded {
            Message::Heartbeat {
                sequence,
                timestamp,
            } => {
                assert_eq!(sequence, 100);
                assert_eq!(timestamp, 200);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_decoder_streaming() {
        let msg1 = Message::Heartbeat {
            sequence: 100,
            timestamp: 200,
        };
        let msg2 = Message::Heartbeat {
            sequence: 300,
            timestamp: 400,
        };

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
            Message::Heartbeat {
                sequence,
                timestamp,
            } => {
                assert_eq!(sequence, 100);
                assert_eq!(timestamp, 200);
            }
            _ => panic!("Wrong message type"),
        }

        // Feed second message
        decoder.feed(&enc2[..]);
        let dec2 = decoder.try_decode().unwrap().unwrap();
        match dec2 {
            Message::Heartbeat {
                sequence,
                timestamp,
            } => {
                assert_eq!(sequence, 300);
                assert_eq!(timestamp, 400);
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
    fn test_max_message_size() {
        let msg = Message::ClipboardData {
            mime_type: "text/plain".to_string(),
            data: vec![0u8; 100],
        };

        let result = MessageCodec::encode(&msg);
        assert!(result.is_ok());
    }

    #[test]
    fn realtime_round_trip_preserves_ordering_metadata() {
        let frame = realtime_frame(9, RealtimeInputPayload::RelativeMouse { dx: 7, dy: -4 });

        let encoded = RealtimeInputCodec::encode(&frame).unwrap();

        assert_eq!(RealtimeInputCodec::decode(&encoded).unwrap(), frame);
    }

    #[test]
    fn realtime_round_trip_preserves_every_payload_variant() {
        let payloads = [
            RealtimeInputPayload::RelativeMouse {
                dx: i32::MIN,
                dy: i32::MAX,
            },
            RealtimeInputPayload::AbsoluteAnchor { x: -800, y: 600 },
            RealtimeInputPayload::GamepadAxes {
                gamepad_id: 3,
                left_stick_x: i16::MIN,
                left_stick_y: i16::MAX,
                right_stick_x: -123,
                right_stick_y: 456,
                left_trigger: 1,
                right_trigger: u16::MAX,
            },
            RealtimeInputPayload::CursorVisual {
                x: -1920,
                y: 1080,
                visible: true,
            },
        ];

        for (sequence, payload) in payloads.into_iter().enumerate() {
            let frame = realtime_frame(sequence as u64, payload);
            let encoded = RealtimeInputCodec::encode(&frame).unwrap();
            assert_eq!(RealtimeInputCodec::decode(&encoded).unwrap(), frame);
        }
    }

    #[test]
    fn realtime_encoding_uses_the_exact_fixed_header_layout() {
        let frame = realtime_frame(9, RealtimeInputPayload::RelativeMouse { dx: 7, dy: -4 });

        let encoded = RealtimeInputCodec::encode(&frame).unwrap();

        assert_eq!(encoded.len(), 37 + 8);
        assert_eq!(&encoded[0..2], &INPUT_PROTOCOL_VERSION.to_be_bytes());
        assert_eq!(encoded[2], 1);
        assert_eq!(&encoded[3..5], &8_u16.to_be_bytes());
        assert_eq!(&encoded[5..13], &42_u64.to_be_bytes());
        assert_eq!(&encoded[13..21], &9_u64.to_be_bytes());
        assert_eq!(&encoded[21..29], &7_u64.to_be_bytes());
        assert_eq!(&encoded[29..37], &123_456_u64.to_be_bytes());
        assert_eq!(&encoded[37..41], &7_i32.to_be_bytes());
        assert_eq!(&encoded[41..45], &(-4_i32).to_be_bytes());
    }

    #[test]
    fn realtime_decode_rejects_wrong_version_kind_length_and_trailing_bytes() {
        let frame = realtime_frame(1, RealtimeInputPayload::RelativeMouse { dx: 1, dy: 2 });
        let encoded = RealtimeInputCodec::encode(&frame).unwrap();

        let mut wrong_version = encoded.to_vec();
        wrong_version[0..2].copy_from_slice(&(INPUT_PROTOCOL_VERSION + 1).to_be_bytes());
        assert!(RealtimeInputCodec::decode(&wrong_version).is_err());

        let mut unknown_kind = encoded.to_vec();
        unknown_kind[2] = u8::MAX;
        assert!(RealtimeInputCodec::decode(&unknown_kind).is_err());

        let mut wrong_length = encoded.to_vec();
        wrong_length[3..5].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(RealtimeInputCodec::decode(&wrong_length).is_err());

        let mut truncated = encoded.to_vec();
        truncated.pop();
        assert!(RealtimeInputCodec::decode(&truncated).is_err());

        let mut trailing = encoded.to_vec();
        trailing.push(0);
        assert!(RealtimeInputCodec::decode(&trailing).is_err());
    }

    #[test]
    fn realtime_decode_rejects_noncanonical_cursor_visibility() {
        let frame = realtime_frame(
            1,
            RealtimeInputPayload::CursorVisual {
                x: 10,
                y: 20,
                visible: false,
            },
        );
        let mut encoded = RealtimeInputCodec::encode(&frame).unwrap().to_vec();
        *encoded.last_mut().unwrap() = 2;

        assert!(RealtimeInputCodec::decode(&encoded).is_err());
    }

    #[test]
    fn realtime_encode_rejects_wrong_inner_protocol_version() {
        let mut frame = realtime_frame(1, RealtimeInputPayload::RelativeMouse { dx: 1, dy: 2 });
        frame.protocol_version += 1;

        assert!(RealtimeInputCodec::encode(&frame).is_err());
    }

    #[test]
    fn realtime_receiver_filters_out_of_order_frames_after_decode() {
        let owner = AuthenticatedInputOwner {
            peer_id: DeviceId::new_v4(),
            control_connection_id: ControlConnectionId::new(),
        };
        let mut gate = InputOwnershipGate::new(owner, SessionEpoch(42));
        let frames = [10, 12, 11].map(|sequence| {
            let frame = realtime_frame(
                sequence,
                RealtimeInputPayload::RelativeMouse {
                    dx: sequence as i32,
                    dy: 0,
                },
            );
            let encoded = RealtimeInputCodec::encode(&frame).unwrap();
            RealtimeInputCodec::decode(&encoded).unwrap()
        });

        assert_eq!(
            gate.accept_realtime(owner, frames[0].session_epoch, frames[0].sequence),
            AcceptRealtime::Accepted
        );
        assert_eq!(
            gate.accept_realtime(owner, frames[1].session_epoch, frames[1].sequence),
            AcceptRealtime::AcceptedWithGap(1)
        );
        assert_eq!(
            gate.accept_realtime(owner, frames[2].session_epoch, frames[2].sequence),
            AcceptRealtime::OutOfOrder
        );
    }

    #[test]
    fn reliable_round_trip_preserves_every_event_variant() {
        let events = vec![
            ReliableInputEvent::Enter {
                target_display_id: "display-main".to_string(),
                x: -800,
                y: 600,
            },
            ReliableInputEvent::Leave,
            ReliableInputEvent::ReleaseAll {
                reason: ReleaseAllReason::OwnershipTransfer,
            },
            ReliableInputEvent::Key {
                keycode: 0x41,
                state: KeyState::Released,
            },
            ReliableInputEvent::TextCommit {
                text: "你好🙂".to_string(),
            },
            ReliableInputEvent::MouseButton {
                button: MouseButton::Back,
                state: ButtonState::Pressed,
                x: -1920,
                y: 1080,
                realtime_anchor_sequence: 19,
            },
            ReliableInputEvent::Wheel {
                delta_x: i32::MIN,
                delta_y: i32::MAX,
            },
            ReliableInputEvent::GamepadConnected {
                info: GamepadDeviceInfo {
                    gamepad_id: 2,
                    name: "test-pad".to_string(),
                    vendor_id: Some(0x1234),
                    product_id: Some(0x5678),
                },
            },
            ReliableInputEvent::GamepadDisconnected { gamepad_id: 2 },
            ReliableInputEvent::GamepadButton {
                gamepad_id: 2,
                button: GamepadButton::Other(42),
                pressed: true,
            },
        ];

        for (sequence, event) in events.into_iter().enumerate() {
            let frame = reliable_frame(sequence as u64, event);
            let encoded = ReliableInputCodec::encode(&frame).unwrap();
            assert_eq!(ReliableInputCodec::decode(&encoded).unwrap(), frame);
        }
    }

    #[test]
    fn mouse_button_anchor_round_trips_compactly() {
        let frame = reliable_frame(
            22,
            ReliableInputEvent::MouseButton {
                button: MouseButton::Left,
                state: ButtonState::Released,
                x: 800,
                y: 450,
                realtime_anchor_sequence: 19,
            },
        );

        let encoded = ReliableInputCodec::encode(&frame).unwrap();

        assert_eq!(ReliableInputCodec::decode(&encoded).unwrap(), frame);
        assert!(encoded.len() < 96);
    }

    #[test]
    fn reliable_encoding_is_deterministic_and_bounded() {
        let frame = reliable_frame(
            22,
            ReliableInputEvent::Enter {
                target_display_id: "display-main".to_string(),
                x: 800,
                y: 450,
            },
        );

        let first = ReliableInputCodec::encode(&frame).unwrap();
        let second = ReliableInputCodec::encode(&frame).unwrap();

        assert_eq!(first, second);
        assert!(first.len() <= MAX_RELIABLE_INPUT_FRAME + 2);

        let oversized = reliable_frame(
            23,
            ReliableInputEvent::Enter {
                target_display_id: "x".repeat(MAX_RELIABLE_INPUT_FRAME),
                x: 0,
                y: 0,
            },
        );
        assert!(ReliableInputCodec::encode(&oversized).is_err());
        assert!(ReliableInputCodec::decode(&vec![0; MAX_RELIABLE_INPUT_FRAME + 3]).is_err());
    }

    #[test]
    fn reliable_decode_rejects_unknown_outer_protocol_and_event_tag() {
        let frame = reliable_frame(1, ReliableInputEvent::Leave);
        let encoded = ReliableInputCodec::encode(&frame).unwrap();

        let mut unknown_outer = encoded.to_vec();
        unknown_outer[0..2].copy_from_slice(&(INPUT_PROTOCOL_VERSION + 1).to_be_bytes());
        assert!(ReliableInputCodec::decode(&unknown_outer).is_err());

        const EVENT_TAG_OFFSET: usize = 2 + 2 + 8 + 8 + 8 + 8;
        let mut unknown_event = encoded.to_vec();
        assert_eq!(
            &unknown_event[EVENT_TAG_OFFSET..EVENT_TAG_OFFSET + 4],
            &1_u32.to_be_bytes(),
            "the test must mutate the serialized Leave event discriminant"
        );
        unknown_event[EVENT_TAG_OFFSET..EVENT_TAG_OFFSET + 4]
            .copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(ReliableInputCodec::decode(&unknown_event).is_err());
    }

    #[test]
    fn reliable_decode_rejects_truncated_invalid_and_trailing_data() {
        assert!(ReliableInputCodec::decode(&[]).is_err());
        assert!(ReliableInputCodec::decode(&[0]).is_err());

        let frame = reliable_frame(
            1,
            ReliableInputEvent::Key {
                keycode: 0x41,
                state: KeyState::Pressed,
            },
        );
        let encoded = ReliableInputCodec::encode(&frame).unwrap();

        let mut truncated = encoded.to_vec();
        truncated.pop();
        assert!(ReliableInputCodec::decode(&truncated).is_err());

        let mut invalid = encoded.to_vec();
        *invalid.last_mut().unwrap() = u8::MAX;
        assert!(ReliableInputCodec::decode(&invalid).is_err());

        let mut trailing = encoded.to_vec();
        trailing.push(0);
        assert!(ReliableInputCodec::decode(&trailing).is_err());
    }

    #[test]
    fn reliable_decode_rejects_outer_and_body_version_mismatch() {
        let frame = reliable_frame(1, ReliableInputEvent::Leave);
        let mut encoded = ReliableInputCodec::encode(&frame).unwrap().to_vec();
        encoded[2..4].copy_from_slice(&(INPUT_PROTOCOL_VERSION + 1).to_be_bytes());

        assert!(ReliableInputCodec::decode(&encoded).is_err());

        let mut wrong_body = frame;
        wrong_body.protocol_version += 1;
        assert!(ReliableInputCodec::encode(&wrong_body).is_err());
    }

    #[test]
    fn text_commit_enforces_utf8_byte_limit_on_encode_and_decode() {
        let boundary_text = "🙂".repeat(MAX_TEXT_COMMIT_BYTES / 4);
        assert_eq!(boundary_text.len(), MAX_TEXT_COMMIT_BYTES);
        let boundary = reliable_frame(
            1,
            ReliableInputEvent::TextCommit {
                text: boundary_text,
            },
        );
        let encoded = ReliableInputCodec::encode(&boundary).unwrap();
        assert!(encoded.len() <= MAX_RELIABLE_INPUT_FRAME + 2);
        assert_eq!(ReliableInputCodec::decode(&encoded).unwrap(), boundary);

        let over_limit_text = "x".repeat(MAX_TEXT_COMMIT_BYTES + 1);
        let over_limit = reliable_frame(
            2,
            ReliableInputEvent::TextCommit {
                text: over_limit_text,
            },
        );
        assert!(ReliableInputCodec::encode(&over_limit).is_err());

        const TEXT_LENGTH_OFFSET: usize = 2 + 2 + 8 + 8 + 8 + 8 + 4;
        let mut over_limit_wire = encoded.to_vec();
        over_limit_wire[TEXT_LENGTH_OFFSET..TEXT_LENGTH_OFFSET + 8]
            .copy_from_slice(&((MAX_TEXT_COMMIT_BYTES + 1) as u64).to_be_bytes());
        over_limit_wire.push(b'x');
        assert!(over_limit_wire.len() <= MAX_RELIABLE_INPUT_FRAME + 2);
        assert!(ReliableInputCodec::decode(&over_limit_wire).is_err());
    }
}
