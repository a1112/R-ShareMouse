use rshare_core::wake::{WakeTargetInput, WakeTargetKind};
use rshare_core::{DaemonRequest, DaemonResponse};

#[test]
fn standalone_target_needs_ipv4_and_normalizes_mac() {
    let input = WakeTargetInput {
        id: None,
        name: "Office PC".into(),
        mac: "aa-bb-cc-dd-ee-01".into(),
        peer_id: None,
        ipv4: Some("192.168.1.50".parse().unwrap()),
    };
    let target = input.clone().validate().unwrap();
    assert_eq!(target.mac, "AA:BB:CC:DD:EE:01");
    assert_eq!(target.kind(), WakeTargetKind::Standalone);

    let missing_ip = WakeTargetInput {
        ipv4: None,
        ..input
    };
    assert!(missing_ip.validate().is_err());
}

#[test]
fn mac_parser_rejects_invalid_or_multicast_addresses() {
    for mac in ["AA:BB:CC:DD:EE", "GG:11:22:33:44:55", "01:11:22:33:44:55"] {
        assert!(rshare_core::wake::normalize_mac(mac).is_err(), "{mac}");
    }
}

#[test]
fn wake_ipc_round_trips() {
    let id = uuid::Uuid::new_v4();
    let input = WakeTargetInput {
        id: Some(id),
        name: "Desk".into(),
        mac: "02:11:22:33:44:55".into(),
        peer_id: None,
        ipv4: Some("192.168.1.50".parse().unwrap()),
    };
    for request in [
        DaemonRequest::ListWakeTargets,
        DaemonRequest::SaveWakeTarget {
            target: input.clone(),
        },
        DaemonRequest::DeleteWakeTarget { target_id: id },
        DaemonRequest::WakeTarget { target_id: id },
        DaemonRequest::GetWakeAttempt { attempt_id: id },
        DaemonRequest::ListWakeAttempts,
    ] {
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<DaemonRequest>(&json).unwrap(),
            request
        );
    }

    let response = DaemonResponse::WakeTargets(Vec::new());
    let json = serde_json::to_string(&response).unwrap();
    assert_eq!(
        serde_json::from_str::<DaemonResponse>(&json).unwrap(),
        response
    );

    let attempts_response = DaemonResponse::WakeAttempts(Vec::new());
    assert_eq!(
        serde_json::from_str::<DaemonResponse>(&serde_json::to_string(&attempts_response).unwrap())
            .unwrap(),
        attempts_response
    );
    let attempt_response = DaemonResponse::WakeAttempt(rshare_core::WakeAttemptSnapshot {
        id,
        target_id: id,
        status: rshare_core::WakeAttemptStatus::Waiting,
        message: "waiting".into(),
        started_at_ms: 1,
        deadline_at_ms: 90_001,
    });
    assert_eq!(
        serde_json::from_str::<DaemonResponse>(&serde_json::to_string(&attempt_response).unwrap())
            .unwrap(),
        attempt_response
    );
}
