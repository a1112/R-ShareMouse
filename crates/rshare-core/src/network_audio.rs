//! Versioned network audio contracts. Separate from the legacy PCM16 audio path.
use crate::DeviceId;
use serde::{Deserialize, Serialize};

pub const MEDIA_VERSION: u16 = 1;
pub const MEDIA_PORT: u16 = 27438;
pub const MAX_CHANNELS: u8 = 8;
pub const MAX_ENDPOINTS: usize = 256;
pub const MAX_CONTROL_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    Input,
    Output,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Encoding {
    Pcm24,
    Float32,
}
impl Encoding {
    pub fn bytes(self) -> usize {
        match self {
            Self::Pcm24 => 3,
            Self::Float32 => 4,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Format {
    pub sample_rate: u32,
    pub channels: u8,
    pub encoding: Encoding,
}
impl Format {
    pub fn validate(self) -> Result<(), AudioError> {
        if ![48000, 96000].contains(&self.sample_rate)
            || !(1..=MAX_CHANNELS).contains(&self.channels)
        {
            return Err(AudioError::UnsupportedFormat);
        }
        Ok(())
    }
    pub fn frames_per_packet(self) -> usize {
        self.sample_rate as usize / 1000
    }
    pub fn frame_bytes(self) -> usize {
        self.channels as usize * self.encoding.bytes()
    }
    pub fn packet_bytes(self) -> usize {
        self.frames_per_packet() * self.frame_bytes()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    /// Platform persistent identity, not a display name or enumeration index.
    pub id: String,
    pub name: String,
    pub direction: Direction,
    pub channels: u8,
    pub sample_rates: Vec<u32>,
    pub virtual_device: bool,
    pub available: bool,
}
impl Endpoint {
    pub fn validate(&self) -> Result<(), AudioError> {
        if self.id.is_empty()
            || self.id.len() > 1024
            || self.name.len() > 1024
            || self.id.contains('\0')
            || self.name.contains('\0')
            || self.channels == 0
            || self.sample_rates.is_empty()
            || self.sample_rates.len() > 32
            || self.sample_rates.iter().any(|x| *x == 0)
        {
            return Err(AudioError::InvalidEndpoint);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grant {
    pub peer: DeviceId,
    pub endpoint: String,
    pub direction: Direction,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AudioConfig {
    pub enabled: bool,
    pub auto_register: bool,
    pub media_port: u16,
    pub buffer_ms: u16,
    pub grants: Vec<Grant>,
}
impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_register: true,
            media_port: MEDIA_PORT,
            buffer_ms: 3,
            grants: vec![],
        }
    }
}
impl AudioConfig {
    pub fn validate(&self) -> Result<(), AudioError> {
        if self.media_port == 0 || !(2..=20).contains(&self.buffer_ms) || self.grants.len() > 4096 {
            return Err(AudioError::InvalidConfiguration);
        }
        if self
            .grants
            .iter()
            .any(|g| g.endpoint.is_empty() || g.endpoint.len() > 1024 || g.endpoint.contains('\0'))
        {
            return Err(AudioError::InvalidConfiguration);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeviceStatus {
    Pending,
    Registered,
    Offline,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VirtualDevice {
    pub id: String,
    pub peer: DeviceId,
    pub name: String,
    pub endpoint: Endpoint,
    pub generation: u64,
    pub status: DeviceStatus,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: DeviceId,
    pub device: String,
    pub peer: DeviceId,
    pub direction: Direction,
    pub generation: u64,
    pub format: Format,
    pub channel_map: Vec<u8>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Diagnostics {
    pub buffer_depth_ms: f64,
    pub network_rtt_ms: Option<f64>,
    /// Only externally measured values; never inferred from RTT or queue depth.
    pub measured_one_way_p95_ms: Option<f64>,
    pub drift_ppm: f64,
    pub received: u64,
    pub lost: u64,
    pub late: u64,
    pub duplicates: u64,
    pub underruns: u64,
    pub overruns: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioSnapshot {
    pub config: AudioConfig,
    pub local_endpoints: Vec<Endpoint>,
    pub devices: Vec<VirtualDevice>,
    pub sessions: Vec<Session>,
    pub diagnostics: Diagnostics,
    pub backend: String,
    pub backend_ready: bool,
    pub last_error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AudioCommand {
    Status,
    Configure(AudioConfig),
    Grant(Grant),
    Revoke(Grant),
    Open {
        device: String,
        format: Format,
        channel_map: Vec<u8>,
    },
    Close {
        session: DeviceId,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaControl {
    Bind {
        version: u16,
        peer: DeviceId,
        control_token: DeviceId,
        generation: u64,
    },
    Catalog {
        generation: u64,
        endpoints: Vec<Endpoint>,
    },
    Open {
        request: DeviceId,
        endpoint: String,
        direction: Direction,
        format: Format,
        channel_map: Vec<u8>,
    },
    Opened {
        request: DeviceId,
        stream: DeviceId,
        generation: u64,
    },
    Close {
        stream: DeviceId,
    },
    Error {
        request: DeviceId,
        error: AudioError,
    },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    #[error("audio endpoint is invalid")]
    InvalidEndpoint,
    #[error("invalid audio configuration")]
    InvalidConfiguration,
    #[error("audio format is not supported")]
    UnsupportedFormat,
    #[error("active sessions use a different sample rate")]
    FormatConflict,
    #[error("audio channel budget exceeded")]
    ResourceLimit,
    #[error("peer has not been authorized for this endpoint")]
    Unauthorized,
    #[error("virtual audio endpoints cannot be exported")]
    VirtualLoop,
    #[error("endpoint is unavailable")]
    Unavailable,
    #[error("stale connection generation")]
    StaleGeneration,
    #[error("invalid channel mapping")]
    InvalidChannelMap,
    #[error("invalid media packet")]
    InvalidPacket,
    #[error("virtual audio backend is not ready")]
    BackendUnavailable,
}
