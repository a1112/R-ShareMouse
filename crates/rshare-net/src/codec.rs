//! Message encoding and decoding

use anyhow::Result;
use bytes::{Buf, BufMut, BytesMut};
use rshare_core::Message;

/// Message frame format
///
/// Frame structure:
/// - 4 bytes: Message length (u32 big-endian)
/// - 1 byte: Message type tag
/// - N bytes: Message payload
///
/// This allows for efficient streaming and future protocol extensions.

const FRAME_HEADER_SIZE: usize = 5; // length (4) + type (1)

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

            // Input events (10-19)
            Message::MouseMove { .. } => 10,
            Message::MouseButton { .. } => 11,
            Message::MouseWheel { .. } => 12,
            Message::Key { .. } => 13,
            Message::KeyExtended { .. } => 14,
            Message::GamepadConnected { .. } => 15,
            Message::GamepadDisconnected { .. } => 16,
            Message::GamepadState { .. } => 17,

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
    use rshare_core::{hello_message, DeviceId, GamepadState};

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
                device_id: DeviceId::new_v4(),
                device_name: String::new(),
                hostname: String::new(),
                protocol_version: 1,
                capabilities: Default::default(),
            }),
            0
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
}
