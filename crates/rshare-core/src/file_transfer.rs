//! Bounded file-drop protocol. Paths are relative to a new receiver-owned folder.
use crate::DeviceId;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const FILE_CHUNK_BYTES: usize = 64 * 1024;
pub const MAX_FILE_ENTRIES: usize = 1024;
pub const MAX_TRANSFER_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub directory: bool,
}

pub fn validate_manifest(entries: &[FileEntry]) -> Result<u64> {
    if entries.is_empty() || entries.len() > MAX_FILE_ENTRIES {
        bail!("一次传输需要 1–1024 个文件或目录");
    }
    let mut paths = HashMap::new();
    let mut total = 0_u64;
    for entry in entries {
        if entry.path.len() > 1024 || entry.path.contains(['\\', ':']) {
            bail!("不安全的文件路径");
        }
        for part in entry.path.split('/') {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            if part.is_empty()
                || part == "."
                || part == ".."
                || part.trim() != part
                || part.ends_with('.')
                || part
                    .chars()
                    .any(|c| c.is_control() || "<>\"|?*".contains(c))
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
            {
                bail!("不安全或不兼容的文件名：{}", entry.path);
            }
        }
        if entry.directory && entry.size != 0 {
            bail!("目录大小必须为零");
        }
        total = total
            .checked_add(entry.size)
            .filter(|n| *n <= MAX_TRANSFER_BYTES)
            .ok_or_else(|| anyhow::anyhow!("单次传输不能超过 10 GiB"))?;
        if paths
            .insert(entry.path.to_lowercase(), entry.directory)
            .is_some()
        {
            bail!("文件路径重复（不区分大小写）");
        }
    }
    for path in paths.keys() {
        for (index, _) in path.match_indices('/') {
            if paths.get(&path[..index]) == Some(&false) {
                bail!("文件与目录路径冲突");
            }
        }
    }
    Ok(total)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileTransferPacket {
    pub transfer_id: DeviceId,
    pub sequence: u64,
    pub body: FileTransferBody,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileTransferBody {
    Offer {
        entries: Vec<FileEntry>,
    },
    Accepted,
    Chunk {
        index: u32,
        offset: u64,
        data: Vec<u8>,
    },
    FileEnd {
        index: u32,
        sha256: [u8; 32],
    },
    Received,
    Complete,
    Completed,
    Cancel {
        reason: String,
    },
    OfferToFolder {
        entries: Vec<FileEntry>,
        drop: FolderDropPoint,
    },
}

/// A receiver-local position tied to one successfully injected release.
/// Never accepts a sender-supplied destination filesystem path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FolderDropPoint {
    pub x: i32,
    pub y: i32,
    pub session_epoch: u64,
    pub release_sequence: u64,
}

#[derive(Default)]
pub struct FolderDropReceipts {
    entries: std::collections::VecDeque<(crate::AuthenticatedInputOwner, FolderDropPoint, u64)>,
}

impl FolderDropReceipts {
    pub fn record(
        &mut self,
        owner: crate::AuthenticatedInputOwner,
        point: FolderDropPoint,
        now_ms: u64,
    ) {
        while self.entries.len() >= 64 {
            self.entries.pop_front();
        }
        self.entries.push_back((owner, point, now_ms));
    }

    pub fn take(
        &mut self,
        owner: crate::AuthenticatedInputOwner,
        point: &FolderDropPoint,
        now_ms: u64,
    ) -> bool {
        self.entries
            .retain(|(_, _, at)| now_ms.saturating_sub(*at) <= 10_000);
        let index = self
            .entries
            .iter()
            .position(|(source, position, _)| *source == owner && position == point);
        index.and_then(|i| self.entries.remove(i)).is_some()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FileTransferStatus {
    Preparing,
    Waiting,
    Transferring,
    Completed,
    Cancelled,
    Failed,
}

impl FileTransferStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileTransferSnapshot {
    pub id: DeviceId,
    pub peer_id: DeviceId,
    pub incoming: bool,
    pub status: FileTransferStatus,
    pub entries: Vec<FileEntry>,
    pub entry_count: usize,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub destination: Option<String>,
    pub error: Option<String>,
}
