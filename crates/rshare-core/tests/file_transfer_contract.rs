use rshare_core::file_transfer::{
    validate_manifest, FileEntry, FileTransferBody, FileTransferPacket,
};
use rshare_core::Message;

fn file(path: &str, size: u64) -> FileEntry {
    FileEntry {
        path: path.into(),
        size,
        directory: false,
    }
}

#[test]
fn file_drop_manifest_rejects_escape_and_cross_platform_collisions() {
    for path in [
        "../secret",
        "/tmp/secret",
        "C:\\secret",
        "a/../../b",
        "a\\b",
        "CON",
        "NUL.txt",
        "file.",
        "a//b",
        ".",
        "a:b",
        "a/ /b",
    ] {
        assert!(validate_manifest(&[file(path, 1)]).is_err(), "{path}");
    }
    assert!(validate_manifest(&[file("Readme", 1), file("README", 1)]).is_err());
    assert!(validate_manifest(&[file("folder", 1), file("folder/file", 1)]).is_err());
    assert!(validate_manifest(&[file("报告/测试.txt", 0), file("photo.png", 42)]).is_ok());
}

#[test]
fn file_drop_protocol_round_trips_without_exposing_source_paths() {
    let message = Message::FileTransfer(FileTransferPacket {
        transfer_id: uuid::Uuid::new_v4(),
        sequence: 0,
        body: FileTransferBody::Offer {
            entries: vec![file("报告.txt", 42)],
        },
    });
    let encoded = serde_json::to_string(&message).unwrap();
    assert!(encoded.contains("报告.txt"));
    let decoded: Message = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(decoded, Message::FileTransfer(_)));
}

#[test]
fn file_drop_limits_and_optional_negotiation_are_backwards_compatible() {
    use rshare_core::file_transfer::{MAX_FILE_ENTRIES, MAX_TRANSFER_BYTES};
    assert!(validate_manifest(&[]).is_err());
    assert!(validate_manifest(&[file("big", MAX_TRANSFER_BYTES + 1)]).is_err());
    assert!(validate_manifest(&[file("one", MAX_TRANSFER_BYTES), file("two", 1)]).is_err());
    assert!(validate_manifest(&vec![file("a", 0); MAX_FILE_ENTRIES + 1]).is_err());
    let old = r#"{"realtime_input_version":1,"reliable_input_version":1,"qos_lanes":true,"separate_media_quic_version":0}"#;
    let old: rshare_core::PeerTransportCapabilities = serde_json::from_str(old).unwrap();
    assert_eq!(old.file_transfer_version, 0);
    assert_eq!(old.folder_drop_version, 0);
    assert!(old.advertises_required_v3_transport_capabilities());
    assert_eq!(
        rshare_core::PeerTransportCapabilities::required_v3().file_transfer_version,
        1
    );
}
