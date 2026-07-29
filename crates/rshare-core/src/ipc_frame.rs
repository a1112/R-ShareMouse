//! Bounded framing for local daemon IPC.

use std::io;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const IPC_FRAME_HEADER_LEN: usize = 5;
pub const DEFAULT_MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_BINARY_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IpcEnvelopeKind {
    Json = 1,
    Binary = 2,
    UiState = 3,
    Heartbeat = 4,
}

impl TryFrom<u8> for IpcEnvelopeKind {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Json),
            2 => Ok(Self::Binary),
            3 => Ok(Self::UiState),
            4 => Ok(Self::Heartbeat),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported IPC envelope kind {other}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpcFrameLimits {
    pub json: usize,
    pub binary: usize,
    pub ui_state: usize,
    pub heartbeat: usize,
}

impl Default for IpcFrameLimits {
    fn default() -> Self {
        Self {
            json: DEFAULT_MAX_JSON_FRAME_BYTES,
            binary: DEFAULT_MAX_BINARY_FRAME_BYTES,
            ui_state: DEFAULT_MAX_JSON_FRAME_BYTES,
            heartbeat: DEFAULT_MAX_JSON_FRAME_BYTES,
        }
    }
}

impl IpcFrameLimits {
    pub fn for_kind(self, kind: IpcEnvelopeKind) -> usize {
        match kind {
            IpcEnvelopeKind::Json => self.json,
            IpcEnvelopeKind::Binary => self.binary,
            IpcEnvelopeKind::UiState => self.ui_state,
            IpcEnvelopeKind::Heartbeat => self.heartbeat,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcFrame {
    pub kind: IpcEnvelopeKind,
    pub payload: Bytes,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IpcFrameCodec {
    pub limits: IpcFrameLimits,
}

impl IpcFrameCodec {
    pub async fn read_frame<R>(&self, reader: &mut R) -> io::Result<Option<IpcFrame>>
    where
        R: AsyncRead + Unpin,
    {
        self.read_frame_inner(reader, None, None).await
    }

    pub async fn read_frame_for_kind<R>(
        &self,
        reader: &mut R,
        expected_kind: IpcEnvelopeKind,
    ) -> io::Result<Option<IpcFrame>>
    where
        R: AsyncRead + Unpin,
    {
        self.read_frame_inner(reader, Some(expected_kind), None)
            .await
    }

    /// Read one frame whose kind must be in `allowed_kinds`.
    ///
    /// The kind is rejected from the fixed-size header before the declared
    /// body is allocated or read.
    pub async fn read_frame_for_kinds<R>(
        &self,
        reader: &mut R,
        allowed_kinds: &[IpcEnvelopeKind],
    ) -> io::Result<Option<IpcFrame>>
    where
        R: AsyncRead + Unpin,
    {
        self.read_frame_inner(reader, None, Some(allowed_kinds))
            .await
    }

    async fn read_frame_inner<R>(
        &self,
        reader: &mut R,
        expected_kind: Option<IpcEnvelopeKind>,
        allowed_kinds: Option<&[IpcEnvelopeKind]>,
    ) -> io::Result<Option<IpcFrame>>
    where
        R: AsyncRead + Unpin,
    {
        let mut header = [0_u8; IPC_FRAME_HEADER_LEN];
        match reader.read_exact(&mut header[..1]).await {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
        }
        reader.read_exact(&mut header[1..]).await?;

        let payload_length =
            u32::from_be_bytes(header[..4].try_into().expect("fixed four-byte length")) as usize;
        let kind = IpcEnvelopeKind::try_from(header[4])?;
        if expected_kind.is_some_and(|expected| expected != kind) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected IPC {:?} envelope, received {kind:?}",
                    expected_kind.expect("checked expected kind")
                ),
            ));
        }
        if allowed_kinds.is_some_and(|allowed| !allowed.contains(&kind)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("IPC {kind:?} envelope is not allowed on this stream"),
            ));
        }
        let limit = self.limits.for_kind(kind);
        if payload_length > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("IPC {kind:?} payload length {payload_length} exceeds {limit}-byte limit"),
            ));
        }

        let mut payload = vec![0_u8; payload_length];
        reader.read_exact(&mut payload).await?;
        Ok(Some(IpcFrame {
            kind,
            payload: Bytes::from(payload),
        }))
    }

    pub async fn write_frame<W>(
        &self,
        writer: &mut W,
        kind: IpcEnvelopeKind,
        payload: &[u8],
    ) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let limit = self.limits.for_kind(kind);
        if payload.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "IPC {kind:?} payload length {} exceeds {limit}-byte limit",
                    payload.len()
                ),
            ));
        }
        let payload_length = u32::try_from(payload.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPC payload length exceeds u32 framing capacity",
            )
        })?;
        let mut header = [0_u8; IPC_FRAME_HEADER_LEN];
        header[..4].copy_from_slice(&payload_length.to_be_bytes());
        header[4] = kind as u8;

        writer.write_all(&header).await?;
        writer.write_all(payload).await?;
        writer.flush().await
    }
}
