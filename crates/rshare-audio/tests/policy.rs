use rshare_audio::registry::Registry;
use rshare_core::network_audio::*;
use rshare_core::DeviceId;

fn endpoint(id: &str, direction: Direction) -> Endpoint {
    Endpoint {
        id: id.into(),
        name: id.into(),
        direction,
        channels: 8,
        sample_rates: vec![48000, 96000],
        virtual_device: false,
        available: true,
    }
}
fn format(channels: u8, sample_rate: u32) -> Format {
    Format {
        sample_rate,
        channels,
        encoding: Encoding::Float32,
    }
}
#[test]
fn pairing_does_not_grant_access_and_virtual_endpoints_are_never_exported() {
    let peer = DeviceId::new_v4();
    let mut registry = Registry::default();
    registry
        .set_local(vec![endpoint("mic", Direction::Input)])
        .unwrap();
    assert!(registry.catalog_for(peer, true).is_empty());
    registry.grant(peer, "mic", Direction::Input).unwrap();
    assert_eq!(registry.catalog_for(peer, true).len(), 1);
    assert!(registry.catalog_for(peer, false).is_empty());
    let mut virtual_endpoint = endpoint("virtual", Direction::Output);
    virtual_endpoint.virtual_device = true;
    registry.set_local(vec![virtual_endpoint]).unwrap();
    assert!(registry.grant(peer, "virtual", Direction::Output).is_err());
}
#[test]
fn stable_identity_disconnect_revoke_and_generation() {
    let peer = DeviceId::new_v4();
    let mut registry = Registry::default();
    registry
        .reconcile(peer, "Studio", 1, vec![endpoint("mic", Direction::Input)])
        .unwrap();
    let id = registry.devices()[0].id.clone();
    registry.disconnected(peer);
    assert_eq!(registry.devices()[0].status, DeviceStatus::Offline);
    registry
        .reconcile(peer, "Renamed", 2, vec![endpoint("mic", Direction::Input)])
        .unwrap();
    assert_eq!(registry.devices()[0].id, id);
    assert!(registry.reconcile(peer, "Old", 1, vec![]).is_err());
    registry.reconcile(peer, "Renamed", 2, vec![]).unwrap();
    assert!(registry.devices().is_empty());
}
#[test]
fn per_peer_direction_budget_and_sample_rate_lock_are_transactional() {
    let peer = DeviceId::new_v4();
    let mut registry = Registry::default();
    registry
        .reconcile(
            peer,
            "Studio",
            1,
            vec![
                endpoint("in", Direction::Input),
                endpoint("out", Direction::Output),
            ],
        )
        .unwrap();
    for id in registry
        .devices()
        .iter()
        .map(|d| d.id.clone())
        .collect::<Vec<_>>()
    {
        registry.set_registration(&id, Ok(()));
    }
    let input = registry
        .devices()
        .iter()
        .find(|x| x.endpoint.direction == Direction::Input)
        .unwrap()
        .id
        .clone();
    let output = registry
        .devices()
        .iter()
        .find(|x| x.endpoint.direction == Direction::Output)
        .unwrap()
        .id
        .clone();
    let first = registry
        .open(&input, format(8, 48000), vec![0, 1, 2, 3, 4, 5, 6, 7])
        .unwrap();
    assert_eq!(
        registry.open(&input, format(1, 48000), vec![0]),
        Err(AudioError::ResourceLimit)
    );
    assert_eq!(
        registry.open(&output, format(8, 96000), vec![0, 1, 2, 3, 4, 5, 6, 7]),
        Err(AudioError::FormatConflict)
    );
    let second = registry
        .open(&output, format(8, 48000), vec![0, 1, 2, 3, 4, 5, 6, 7])
        .unwrap();
    registry.close(first.id);
    registry.close(second.id);
    assert!(registry
        .open(&output, format(8, 96000), vec![0, 1, 2, 3, 4, 5, 6, 7])
        .is_ok());
}
#[test]
fn invalid_catalog_does_not_remove_existing_devices() {
    let mut registry = Registry::default();
    let peer = DeviceId::new_v4();
    registry
        .reconcile(peer, "pc", 1, vec![endpoint("in", Direction::Input)])
        .unwrap();
    let mut bad = endpoint("bad", Direction::Input);
    bad.channels = 0;
    assert!(registry.reconcile(peer, "pc", 1, vec![bad]).is_err());
    assert_eq!(registry.devices().len(), 1);
}

#[test]
fn advertised_endpoint_is_not_openable_until_os_registration_succeeds() {
    let peer = DeviceId::new_v4();
    let mut registry = Registry::default();
    registry
        .reconcile(peer, "pc", 1, vec![endpoint("mic", Direction::Input)])
        .unwrap();
    let id = registry.devices()[0].id.clone();
    assert_eq!(
        registry.open(&id, format(1, 48000), vec![0]),
        Err(AudioError::BackendUnavailable)
    );
    registry.set_registration(&id, Err("driver absent".into()));
    assert_eq!(
        registry.open(&id, format(1, 48000), vec![0]),
        Err(AudioError::BackendUnavailable)
    );
    assert!(registry.sessions().is_empty());
}
