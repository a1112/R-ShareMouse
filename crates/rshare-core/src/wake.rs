//! Local Wake-on-LAN target and attempt contracts.

use crate::DeviceId;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeTargetKind {
    Peer,
    Standalone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeTarget {
    pub id: Uuid,
    pub name: String,
    pub mac: String,
    pub peer_id: Option<DeviceId>,
    pub ipv4: Option<Ipv4Addr>,
}

impl WakeTarget {
    pub fn kind(&self) -> WakeTargetKind {
        if self.peer_id.is_some() {
            WakeTargetKind::Peer
        } else {
            WakeTargetKind::Standalone
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeTargetInput {
    /// `None` creates a target; an existing ID updates it.
    pub id: Option<Uuid>,
    pub name: String,
    pub mac: String,
    pub peer_id: Option<DeviceId>,
    pub ipv4: Option<Ipv4Addr>,
}

impl WakeTargetInput {
    pub fn validate(self) -> Result<WakeTarget, String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err("Target name must contain 1 to 80 characters".into());
        }
        if self.peer_id.is_none() && self.ipv4.is_none() {
            return Err("Standalone target requires an IPv4 address".into());
        }
        if let Some(ip) = self.ipv4 {
            if ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_unspecified()
            {
                return Err("Target IPv4 address must be a usable LAN unicast address".into());
            }
        }
        let mac = normalize_mac(&self.mac)?;
        Ok(WakeTarget {
            id: self.id.unwrap_or_else(Uuid::new_v4),
            name: name.to_owned(),
            mac,
            peer_id: self.peer_id,
            ipv4: self.ipv4,
        })
    }
}

pub fn normalize_mac(value: &str) -> Result<String, String> {
    let compact: String = value.chars().filter(|c| *c != ':' && *c != '-').collect();
    if compact.len() != 12 || !compact.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("MAC address must contain six hexadecimal octets".into());
    }
    let mut octets = [0u8; 6];
    for (index, octet) in octets.iter_mut().enumerate() {
        *octet = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)
            .map_err(|_| "Invalid MAC address")?;
    }
    if octets[0] & 1 != 0 || octets.iter().all(|octet| *octet == 0) {
        return Err("MAC address must identify a unicast network adapter".into());
    }
    Ok(octets
        .iter()
        .map(|octet| format!("{octet:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeAttemptStatus {
    Waiting,
    Confirmed,
    Unconfirmed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeAttemptSnapshot {
    pub id: Uuid,
    pub target_id: Uuid,
    pub status: WakeAttemptStatus,
    pub message: String,
    pub started_at_ms: u64,
    pub deadline_at_ms: u64,
}
