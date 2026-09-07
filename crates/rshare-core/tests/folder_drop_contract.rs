use rshare_core::{file_transfer::*, AuthenticatedInputOwner, ControlConnectionId, DeviceId};

#[test]
fn folder_drop_receipt_is_single_use_and_bound_to_connection_epoch_and_position() {
    let owner = AuthenticatedInputOwner {
        peer_id: DeviceId::new_v4(),
        control_connection_id: ControlConnectionId::new(),
    };
    let point = FolderDropPoint {
        x: 40,
        y: 90,
        session_epoch: 5,
        release_sequence: 12,
    };
    let mut receipts = FolderDropReceipts::default();
    receipts.record(owner, point.clone(), 100);
    let mut wrong = point.clone();
    wrong.x += 1;
    assert!(!receipts.take(owner, &wrong, 101));
    let wrong_owner = AuthenticatedInputOwner {
        control_connection_id: ControlConnectionId::new(),
        ..owner
    };
    assert!(!receipts.take(wrong_owner, &point, 101));
    assert!(receipts.take(owner, &point, 101));
    assert!(!receipts.take(owner, &point, 101));
    receipts.record(owner, point.clone(), 100);
    assert!(!receipts.take(owner, &point, 10_101));
}

#[test]
fn folder_drop_does_not_send_target_local_paths_over_the_wire() {
    let body = FileTransferBody::OfferToFolder {
        entries: vec![FileEntry {
            path: "报告.txt".into(),
            size: 0,
            directory: false,
        }],
        drop: FolderDropPoint {
            x: 12,
            y: 20,
            session_epoch: 2,
            release_sequence: 9,
        },
    };
    let json = serde_json::to_string(&body).unwrap();
    assert!(json.contains("release_sequence"));
    assert!(!json.contains("destination"));
}
