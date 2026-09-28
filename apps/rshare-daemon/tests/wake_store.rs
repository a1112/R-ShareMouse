use rshare_core::{WakeAttemptStatus, WakeTargetInput};
use rshare_daemon::wake::{wait_for_confirmation, ProbeResult, WakeManager};
use std::time::Duration;

#[test]
fn targets_survive_restart_and_duplicate_peer_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wake-targets.json");
    let peer_id = rshare_core::DeviceId::new_v4();
    let input = WakeTargetInput {
        id: None,
        name: "Desk".into(),
        mac: "02:11:22:33:44:55".into(),
        peer_id: Some(peer_id),
        ipv4: None,
    };
    let mut manager = WakeManager::load(path.clone()).unwrap();
    let saved = manager.save_target(input.clone()).unwrap();
    assert_eq!(
        WakeManager::load(path).unwrap().targets().unwrap(),
        vec![saved]
    );
    assert!(manager.save_target(input).is_err());
}

#[test]
fn target_update_and_delete_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wake-targets.json");
    let mut manager = WakeManager::load(path.clone()).unwrap();
    let created = manager
        .save_target(WakeTargetInput {
            id: None,
            name: "Old".into(),
            mac: "02:11:22:33:44:55".into(),
            peer_id: None,
            ipv4: Some("192.168.1.50".parse().unwrap()),
        })
        .unwrap();
    let updated = manager
        .save_target(WakeTargetInput {
            id: Some(created.id),
            name: "New".into(),
            mac: created.mac.clone(),
            peer_id: None,
            ipv4: created.ipv4,
        })
        .unwrap();
    assert_eq!(
        WakeManager::load(path.clone()).unwrap().targets().unwrap(),
        vec![updated]
    );
    manager.delete_target(created.id).unwrap();
    assert!(WakeManager::load(path)
        .unwrap()
        .targets()
        .unwrap()
        .is_empty());
}

#[test]
fn repeated_wake_uses_the_same_active_attempt() {
    let mut manager = WakeManager::empty();
    let target = manager
        .save_target(WakeTargetInput {
            id: None,
            name: "Desk".into(),
            mac: "02:11:22:33:44:55".into(),
            peer_id: None,
            ipv4: Some("192.168.1.50".parse().unwrap()),
        })
        .unwrap();
    let (_, first, should_send) = manager.begin(target.id).unwrap();
    assert!(should_send);
    let (_, second, should_send) = manager.begin(target.id).unwrap();
    assert!(!should_send);
    assert_eq!(first.id, second.id);
}

#[tokio::test(start_paused = true)]
async fn polling_confirms_after_target_appears() {
    let mut checks = 0;
    let result = wait_for_confirmation(Duration::from_secs(10), Duration::from_secs(2), || {
        checks += 1;
        let current = checks;
        async move {
            if current == 3 {
                ProbeResult::Responded
            } else {
                ProbeResult::NoReply
            }
        }
    })
    .await;
    assert_eq!(result.status, WakeAttemptStatus::Confirmed);
    assert_eq!(checks, 3);
}

#[tokio::test(start_paused = true)]
async fn polling_reports_unconfirmed_after_timeout() {
    let mut checks = 0;
    let result = wait_for_confirmation(Duration::from_secs(6), Duration::from_secs(2), || {
        checks += 1;
        async { ProbeResult::NoReply }
    })
    .await;
    assert_eq!(result.status, WakeAttemptStatus::Unconfirmed);
    assert!(result.message.contains("No response"));
    assert_eq!(checks, 4);
}

#[tokio::test(start_paused = true)]
async fn polling_reports_probe_unavailable_without_claiming_success() {
    let result = wait_for_confirmation(Duration::from_secs(90), Duration::from_secs(2), || async {
        ProbeResult::Unavailable("ping command not found".into())
    })
    .await;
    assert_eq!(result.status, WakeAttemptStatus::Unconfirmed);
    assert!(result.message.contains("ping command not found"));
}

#[test]
fn malformed_store_is_not_overwritten_by_a_new_target() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wake-targets.json");
    std::fs::write(&path, b"broken JSON").unwrap();
    assert!(WakeManager::load(path.clone()).is_err());
    let mut manager = WakeManager::unavailable(path.clone(), "invalid JSON".into());
    assert!(manager.targets().is_err());
    assert!(manager
        .save_target(WakeTargetInput {
            id: None,
            name: "Desk".into(),
            mac: "02:11:22:33:44:55".into(),
            peer_id: None,
            ipv4: Some("192.168.1.50".parse().unwrap()),
        })
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"broken JSON");
}
