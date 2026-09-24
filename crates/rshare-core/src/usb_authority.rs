//! Local authorization and connection-scoped USB leases; never trust a wire peer ID.
use crate::{ControlConnectionId, DeviceId};
use anyhow::{bail, Context, Result};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UsbOwner {
    pub peer: DeviceId,
    pub connection: ControlConnectionId,
}

#[derive(Debug)]
struct Lease {
    owner: UsbOwner,
    key: String,
    expires: Instant,
    last_transfer: Option<u64>,
}

#[derive(Default)]
pub struct UsbAuthority {
    grants: HashMap<(DeviceId, String), Instant>,
    leases: HashMap<Uuid, Lease>,
}
impl UsbAuthority {
    pub fn visible(&self, peer: DeviceId, key: &str) -> bool {
        self.grants
            .get(&(peer, key.to_owned()))
            .is_some_and(|expiry| *expiry > Instant::now())
    }
    pub fn grant(&mut self, peer: DeviceId, key: String, seconds: u32) -> Result<()> {
        if key.len() > 128 || Uuid::parse_str(&key).is_err() || !(1..=3600).contains(&seconds) {
            bail!("Invalid USB grant key or lifetime");
        }
        self.grants.retain(|_, expiry| *expiry > Instant::now());
        if self.grants.len() >= 256 {
            bail!("USB grant limit reached");
        }
        self.grants.insert(
            (peer, key),
            Instant::now() + Duration::from_secs(seconds.into()),
        );
        Ok(())
    }
    pub fn check_grant(&self, owner: UsbOwner, key: &str) -> Result<Instant> {
        let expiry = *self
            .grants
            .get(&(owner.peer, key.to_owned()))
            .context("Explicit local USB device authorization required")?;
        if expiry <= Instant::now() {
            bail!("USB authorization expired");
        }
        if self.leases.values().any(|l| l.key == key) {
            bail!("USB device is already leased");
        }
        if self.leases.len() >= 32 {
            bail!("USB lease limit reached");
        }
        Ok(expiry)
    }
    pub fn attach(&mut self, owner: UsbOwner, key: String, id: Uuid, expires: Instant) {
        self.leases.insert(
            id,
            Lease {
                owner,
                key,
                expires,
                last_transfer: None,
            },
        );
    }
    pub fn check(&self, owner: UsbOwner, id: Option<Uuid>, key: &str) -> Result<Uuid> {
        let id = id.context("USB lease ID is required")?;
        let lease = self.leases.get(&id).context("Unknown USB lease")?;
        if lease.owner != owner || lease.key != key || lease.expires <= Instant::now() {
            bail!("USB lease owner, generation, device or lifetime mismatch");
        }
        Ok(id)
    }
    pub fn begin(
        &mut self,
        owner: UsbOwner,
        id: Option<Uuid>,
        key: &str,
        transfer: u64,
    ) -> Result<Uuid> {
        let id = self.check(owner, id, key)?;
        let lease = self.leases.get_mut(&id).unwrap();
        if lease.last_transfer.is_some_and(|last| transfer <= last) {
            bail!("Duplicate or stale USB transfer");
        }
        lease.last_transfer = Some(transfer);
        Ok(id)
    }
    pub fn remove(&mut self, id: Uuid) {
        self.leases.remove(&id);
    }
    pub fn revoke(&mut self, peer: DeviceId, key: &str) -> Vec<Uuid> {
        self.grants.remove(&(peer, key.to_owned()));
        self.take_where(|l| l.owner.peer == peer && l.key == key)
    }
    pub fn disconnect(&mut self, owner: UsbOwner) -> Vec<Uuid> {
        // A new connection must receive a new local authorization, never replay OUT.
        let ids = self.take_where(|l| l.owner == owner);
        if !self.leases.values().any(|l| l.owner.peer == owner.peer) {
            self.grants.retain(|(peer, _), _| *peer != owner.peer);
        }
        ids
    }
    pub fn expire(&mut self) -> Vec<Uuid> {
        self.take_where(|l| l.expires <= Instant::now())
    }
    pub fn device_gone(&mut self, key: &str) -> Vec<Uuid> {
        self.grants.retain(|(_, k), _| k != key);
        self.take_where(|l| l.key == key)
    }
    fn take_where(&mut self, predicate: impl Fn(&Lease) -> bool) -> Vec<Uuid> {
        let ids: Vec<_> = self
            .leases
            .iter()
            .filter(|(_, l)| predicate(l))
            .map(|(id, _)| *id)
            .collect();
        for id in &ids {
            self.leases.remove(id);
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_rejects_wrong_peer_generation_device_missing_id_and_replay() {
        let mut table = UsbAuthority::default();
        let owner = UsbOwner {
            peer: Uuid::new_v4(),
            connection: ControlConnectionId::new(),
        };
        let key = Uuid::new_v4().to_string();
        assert!(table.check_grant(owner, &key).is_err());
        table.grant(owner.peer, key.clone(), 30).unwrap();
        let expiry = table.check_grant(owner, &key).unwrap();
        let id = Uuid::new_v4();
        table.attach(owner, key.clone(), id, expiry);
        assert!(table.check_grant(owner, &key).is_err());
        assert!(table.check(owner, None, &key).is_err());
        assert!(table.check(owner, Some(id), "another-device").is_err());
        assert!(table
            .check(
                UsbOwner {
                    peer: Uuid::new_v4(),
                    ..owner
                },
                Some(id),
                &key
            )
            .is_err());
        let newer = UsbOwner {
            connection: ControlConnectionId::new(),
            ..owner
        };
        assert!(table.check(newer, Some(id), &key).is_err());
        table.begin(owner, Some(id), &key, 10).unwrap();
        assert!(table.begin(owner, Some(id), &key, 10).is_err());
        assert!(table.begin(owner, Some(id), &key, 9).is_err());
        assert!(table.disconnect(newer).is_empty());
        assert!(table.check(owner, Some(id), &key).is_ok());
        assert_eq!(table.disconnect(owner), vec![id]);
        assert!(table.check(owner, Some(id), &key).is_err());
        assert!(table.check_grant(newer, &key).is_err());
    }
    #[test]
    fn revoke_and_expiry_return_each_lease_once() {
        let mut table = UsbAuthority::default();
        let owner = UsbOwner {
            peer: Uuid::new_v4(),
            connection: ControlConnectionId::new(),
        };
        let key = Uuid::new_v4().to_string();
        let id = Uuid::new_v4();
        table.attach(
            owner,
            key.clone(),
            id,
            Instant::now() - Duration::from_secs(1),
        );
        assert!(table.check(owner, Some(id), &key).is_err());
        assert_eq!(table.expire(), vec![id]);
        assert!(table.expire().is_empty());
        table.attach(
            owner,
            key.clone(),
            id,
            Instant::now() + Duration::from_secs(20),
        );
        assert_eq!(table.revoke(owner.peer, &key), vec![id]);
        assert!(table.revoke(owner.peer, &key).is_empty());
    }
}
