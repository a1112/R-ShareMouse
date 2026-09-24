//! Resident-memory accounting. Cancellation must retain the guard until native I/O ends.
use crate::usb_authority::UsbOwner;
use anyhow::{bail, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub requests: usize,
    pub bytes: usize,
}
#[derive(Default)]
struct Counts {
    total: Usage,
    peers: HashMap<Uuid, Usage>,
    leases: HashMap<(UsbOwner, Uuid), Usage>,
}
#[derive(Default)]
pub struct UsbBudget {
    counts: Mutex<Counts>,
}
pub struct Reservation {
    budget: Arc<UsbBudget>,
    owner: UsbOwner,
    lease: Uuid,
    bytes: usize,
}
impl UsbBudget {
    pub fn reserve(
        self: &Arc<Self>,
        owner: UsbOwner,
        lease: Uuid,
        payload_capacity: usize,
    ) -> Result<Reservation> {
        if payload_capacity > 1024 * 1024 {
            bail!("USB single request budget exhausted");
        }
        // Wire payload + native buffer + completion serialization may coexist.
        let bytes = payload_capacity * 3 + 1024;
        let mut c = self.counts.lock().unwrap();
        let peer = c.peers.get(&owner.peer).copied().unwrap_or_default();
        let leased = c.leases.get(&(owner, lease)).copied().unwrap_or_default();
        if c.total.bytes + bytes > 64 * 1024 * 1024
            || c.total.requests >= 512
            || peer.bytes + bytes > 16 * 1024 * 1024
            || peer.requests >= 128
            || leased.bytes + bytes > 4 * 1024 * 1024
            || leased.requests >= 32
        {
            bail!("USB resident byte or request budget exhausted");
        }
        add(&mut c.total, bytes);
        add(c.peers.entry(owner.peer).or_default(), bytes);
        add(c.leases.entry((owner, lease)).or_default(), bytes);
        Ok(Reservation {
            budget: self.clone(),
            owner,
            lease,
            bytes,
        })
    }
    pub fn usage(&self) -> Usage {
        self.counts.lock().unwrap().total
    }
}
fn add(usage: &mut Usage, bytes: usize) {
    usage.requests += 1;
    usage.bytes += bytes;
}
fn subtract(usage: &mut Usage, bytes: usize) {
    usage.requests -= 1;
    usage.bytes -= bytes;
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut c = self.budget.counts.lock().unwrap();
        subtract(&mut c.total, self.bytes);
        let peer = c.peers.get_mut(&self.owner.peer).unwrap();
        subtract(peer, self.bytes);
        if peer.requests == 0 {
            c.peers.remove(&self.owner.peer);
        }
        let leased = c.leases.get_mut(&(self.owner, self.lease)).unwrap();
        subtract(leased, self.bytes);
        if leased.requests == 0 {
            c.leases.remove(&(self.owner, self.lease));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ControlConnectionId;
    #[test]
    fn failed_admission_does_not_charge_and_drop_refunds() {
        let budget = Arc::new(UsbBudget::default());
        let owner = UsbOwner {
            peer: Uuid::new_v4(),
            connection: ControlConnectionId::new(),
        };
        let lease = Uuid::new_v4();
        let held = budget.reserve(owner, lease, 1024 * 1024).unwrap();
        let before = budget.usage();
        assert!(budget.reserve(owner, lease, 1024 * 1024).is_err());
        assert_eq!(budget.usage(), before);
        drop(held);
        assert_eq!(budget.usage(), Usage::default());
        let guards: Vec<_> = (0..32)
            .map(|_| budget.reserve(owner, lease, 0).unwrap())
            .collect();
        assert!(budget.reserve(owner, lease, 0).is_err());
        drop(guards);
        assert_eq!(budget.usage(), Usage::default());
    }
    #[test]
    fn concurrent_completion_and_peer_budgets_are_bounded() {
        let budget = Arc::new(UsbBudget::default());
        let owner = UsbOwner {
            peer: Uuid::new_v4(),
            connection: ControlConnectionId::new(),
        };
        let guards: Vec<_> = (0..5)
            .map(|_| budget.reserve(owner, Uuid::new_v4(), 1024 * 1024).unwrap())
            .collect();
        assert!(budget.reserve(owner, Uuid::new_v4(), 1024 * 1024).is_err());
        let threads: Vec<_> = guards
            .into_iter()
            .map(|guard| std::thread::spawn(move || drop(guard)))
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(budget.usage(), Usage::default());
    }
}
