//! Pure policy state. Pending entries are not claims of OS device registration.
use rshare_core::{network_audio::*, DeviceId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub fn device_id(peer: DeviceId, endpoint: &str, direction: Direction) -> String {
    let mut digest = Sha256::new();
    digest.update(b"rshare-audio-v1\0");
    digest.update(peer.as_bytes());
    digest.update([match direction {
        Direction::Input => 0,
        Direction::Output => 1,
    }]);
    digest.update(endpoint.as_bytes());
    format!("rshare-audio-{:x}", digest.finalize())
}
#[derive(Debug, Clone, Default)]
pub struct Registry {
    local: Vec<Endpoint>,
    grants: Vec<Grant>,
    devices: Vec<VirtualDevice>,
    sessions: BTreeMap<DeviceId, Session>,
    generations: BTreeMap<DeviceId, u64>,
}
impl Registry {
    pub fn local(&self) -> &[Endpoint] {
        &self.local
    }
    pub fn devices(&self) -> &[VirtualDevice] {
        &self.devices
    }
    pub fn sessions(&self) -> Vec<Session> {
        self.sessions.values().cloned().collect()
    }
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }
    pub fn restore_grants(&mut self, grants: Vec<Grant>) {
        self.grants = grants;
    }
    pub fn set_local(&mut self, endpoints: Vec<Endpoint>) -> Result<(), AudioError> {
        validate_catalog(&endpoints)?;
        self.local = endpoints;
        Ok(())
    }
    pub fn grant(
        &mut self,
        peer: DeviceId,
        id: &str,
        direction: Direction,
    ) -> Result<(), AudioError> {
        let endpoint = self
            .local
            .iter()
            .find(|e| e.id == id && e.direction == direction)
            .ok_or(AudioError::InvalidEndpoint)?;
        if endpoint.virtual_device {
            return Err(AudioError::VirtualLoop);
        }
        let grant = Grant {
            peer,
            endpoint: id.into(),
            direction,
        };
        if !self.grants.contains(&grant) {
            self.grants.push(grant);
        }
        Ok(())
    }
    pub fn revoke(&mut self, grant: &Grant) {
        self.grants.retain(|g| g != grant);
    }
    pub fn catalog_for(&self, peer: DeviceId, trusted: bool) -> Vec<Endpoint> {
        if !trusted {
            return vec![];
        }
        self.local
            .iter()
            .filter(|e| {
                !e.virtual_device
                    && self
                        .grants
                        .iter()
                        .any(|g| g.peer == peer && g.endpoint == e.id && g.direction == e.direction)
            })
            .cloned()
            .collect()
    }
    pub fn reconcile(
        &mut self,
        peer: DeviceId,
        name: &str,
        generation: u64,
        endpoints: Vec<Endpoint>,
    ) -> Result<(), AudioError> {
        validate_catalog(&endpoints)?;
        if generation == 0
            || self
                .generations
                .get(&peer)
                .is_some_and(|previous| generation < *previous)
        {
            return Err(AudioError::StaleGeneration);
        }
        if name.len() > 1024 {
            return Err(AudioError::InvalidEndpoint);
        }
        if endpoints.iter().any(|e| e.virtual_device) {
            return Err(AudioError::VirtualLoop);
        }
        let mut next = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            let id = device_id(peer, &endpoint.id, endpoint.direction);
            let existing = self.devices.iter().find(|d| d.id == id);
            let status = if !endpoint.available {
                DeviceStatus::Unavailable
            } else if existing.is_some_and(|d| d.status == DeviceStatus::Registered) {
                DeviceStatus::Registered
            } else {
                DeviceStatus::Pending
            };
            next.push(VirtualDevice {
                id,
                peer,
                name: format!("RShare · {name} · {}", endpoint.name),
                endpoint,
                generation,
                status,
                error: None,
            });
        }
        self.sessions.retain(|_, s| {
            s.peer != peer
                || next.iter().any(|d| {
                    d.id == s.device
                        && d.generation == s.generation
                        && d.endpoint.available
                        && d.endpoint.channels >= s.format.channels
                        && d.endpoint.sample_rates.contains(&s.format.sample_rate)
                        && s.channel_map.iter().all(|c| *c < d.endpoint.channels)
                })
        });
        self.devices.retain(|d| d.peer != peer);
        self.devices.extend(next);
        self.generations.insert(peer, generation);
        Ok(())
    }
    pub fn set_registration(&mut self, id: &str, result: Result<(), String>) {
        if let Some(device) = self.devices.iter_mut().find(|d| d.id == id) {
            match result {
                Ok(()) => {
                    device.status = DeviceStatus::Registered;
                    device.error = None;
                }
                Err(error) => {
                    device.status = DeviceStatus::Unavailable;
                    device.error = Some(error);
                }
            }
        }
    }
    pub fn disconnected(&mut self, peer: DeviceId) {
        for d in self.devices.iter_mut().filter(|d| d.peer == peer) {
            d.status = DeviceStatus::Offline;
        }
        self.sessions.retain(|_, s| s.peer != peer);
    }
    pub fn open(
        &mut self,
        id: &str,
        format: Format,
        channel_map: Vec<u8>,
    ) -> Result<Session, AudioError> {
        format.validate()?;
        let device = self
            .devices
            .iter()
            .find(|d| d.id == id)
            .ok_or(AudioError::InvalidEndpoint)?;
        if device.status != DeviceStatus::Registered {
            return Err(AudioError::BackendUnavailable);
        }
        if !device.endpoint.sample_rates.contains(&format.sample_rate)
            || format.channels > device.endpoint.channels
        {
            return Err(AudioError::UnsupportedFormat);
        }
        if channel_map.len() != format.channels as usize
            || channel_map.iter().any(|c| *c >= device.endpoint.channels)
            || channel_map.iter().collect::<BTreeSet<_>>().len() != channel_map.len()
        {
            return Err(AudioError::InvalidChannelMap);
        }
        let mut channels = 0u16;
        for session in self.sessions.values().filter(|s| s.peer == device.peer) {
            if session.format.sample_rate != format.sample_rate {
                return Err(AudioError::FormatConflict);
            }
            if session.direction == device.endpoint.direction {
                channels += session.format.channels as u16;
            }
        }
        if channels + format.channels as u16 > MAX_CHANNELS as u16 {
            return Err(AudioError::ResourceLimit);
        }
        let session = Session {
            id: DeviceId::new_v4(),
            device: id.into(),
            peer: device.peer,
            direction: device.endpoint.direction,
            generation: device.generation,
            format,
            channel_map,
        };
        self.sessions.insert(session.id, session.clone());
        Ok(session)
    }
    pub fn close(&mut self, id: DeviceId) {
        self.sessions.remove(&id);
    }
}
fn validate_catalog(endpoints: &[Endpoint]) -> Result<(), AudioError> {
    if endpoints.len() > MAX_ENDPOINTS {
        return Err(AudioError::ResourceLimit);
    }
    let mut ids = BTreeSet::new();
    for endpoint in endpoints {
        endpoint.validate()?;
        if !ids.insert((&endpoint.id, endpoint.direction)) {
            return Err(AudioError::InvalidEndpoint);
        }
    }
    Ok(())
}
