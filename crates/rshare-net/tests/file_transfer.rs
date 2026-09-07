use rshare_core::{file_transfer::*, DeviceId, Message};
use rshare_net::{
    connection::{ConnectionManager, ManagerEvent},
    encryption::{Encryption, QuicIdentity},
    file_transfer::FileTransferService,
    qos::BulkFrame,
    QuicTransport,
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::timeout};

struct TestFolderResolver(std::path::PathBuf);
impl rshare_net::file_transfer::FolderDropResolver for TestFolderResolver {
    fn resolve(
        &self,
        _owner: rshare_core::AuthenticatedInputOwner,
        point: FolderDropPoint,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<std::path::PathBuf>> + Send + '_>,
    > {
        Box::pin(async move {
            if point != drop_point() {
                anyhow::bail!("没有匹配的跨屏松手事件");
            }
            Ok(self.0.clone())
        })
    }
}

fn drop_point() -> FolderDropPoint {
    FolderDropPoint {
        x: 450,
        y: 340,
        session_epoch: 3,
        release_sequence: 19,
    }
}

#[tokio::test]
async fn folder_drop_publishes_into_the_resolved_folder_and_preserves_collisions() {
    let destination = tempfile::tempdir().unwrap();
    let pair = Pair::with_resolver(Some(Arc::new(TestFolderResolver(
        destination.path().into(),
    ))))
    .await;
    let source = pair._root.path().join("源文件夹");
    std::fs::create_dir_all(source.join("资料/空目录")).unwrap();
    std::fs::write(source.join("报告.txt"), "新报告").unwrap();
    std::fs::write(source.join("资料/data.bin"), [1, 2, 3]).unwrap();
    std::fs::write(destination.path().join("报告.txt"), "原报告").unwrap();
    std::fs::create_dir(destination.path().join("资料")).unwrap();
    std::fs::write(destination.path().join("资料/原文件"), "保留").unwrap();
    for copy in 2..=3 {
        let sent = pair
            .sender
            .send_files_to_folder(
                pair.b_id,
                vec![
                    source.join("报告.txt").to_string_lossy().into(),
                    source.join("资料").to_string_lossy().into(),
                ],
                drop_point(),
            )
            .unwrap();
        let sent = pair.wait(&pair.sender, sent.id).await;
        assert_eq!(
            sent.status,
            FileTransferStatus::Completed,
            "{:?}",
            sent.error
        );
        let received = pair.wait(&pair.receiver, sent.id).await;
        assert_eq!(
            std::path::PathBuf::from(received.destination.unwrap()),
            destination.path().canonicalize().unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(destination.path().join(format!("报告 ({copy}).txt"))).unwrap(),
            "新报告"
        );
        assert_eq!(
            std::fs::read(destination.path().join(format!("资料 ({copy})/data.bin"))).unwrap(),
            [1, 2, 3]
        );
        assert!(destination
            .path()
            .join(format!("资料 ({copy})/空目录"))
            .is_dir());
    }
    assert_eq!(
        std::fs::read_to_string(destination.path().join("报告.txt")).unwrap(),
        "原报告"
    );
    assert_eq!(
        std::fs::read_to_string(destination.path().join("资料/原文件")).unwrap(),
        "保留"
    );
    assert!(!pair._root.path().join("b-received").exists());
    assert!(std::fs::read_dir(destination.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".partial")));
    pair.close().await;
}

#[tokio::test]
async fn folder_drop_rejects_invalid_gestures_and_never_falls_back_to_downloads() {
    let destination = tempfile::tempdir().unwrap();
    let pair = Pair::with_resolver(Some(Arc::new(TestFolderResolver(
        destination.path().into(),
    ))))
    .await;
    let source = pair._root.path().join("test.txt");
    std::fs::write(&source, "owned fixture").unwrap();
    let sent = pair
        .sender
        .send_files_to_folder(
            pair.b_id,
            vec![source.to_string_lossy().into()],
            FolderDropPoint {
                release_sequence: 20,
                ..drop_point()
            },
        )
        .unwrap();
    let sent = pair.wait(&pair.sender, sent.id).await;
    assert_eq!(sent.status, FileTransferStatus::Cancelled);
    assert!(sent.error.unwrap().contains("松手事件"));
    assert_eq!(
        pair.wait(&pair.receiver, sent.id).await.status,
        FileTransferStatus::Failed
    );
    assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
    assert!(!pair._root.path().join("b-received").exists());
    pair.close().await;
}

#[tokio::test]
async fn folder_drop_requires_its_own_capability_in_addition_to_file_transfer() {
    let pair = Pair::new().await;
    let mut peer = pair.a.qos_registry().peer(&pair.b_id).unwrap();
    peer.folder_drop_version = 0;
    pair.a.qos_registry().insert(pair.b_id, peer);
    assert!(pair
        .sender
        .send_files_to_folder(pair.b_id, vec!["/fixture".into()], drop_point())
        .unwrap_err()
        .to_string()
        .contains("不支持"));
    pair.close().await;
}

struct Pair {
    _root: tempfile::TempDir,
    a_id: DeviceId,
    b_id: DeviceId,
    a: ConnectionManager,
    b: ConnectionManager,
    sender: Arc<FileTransferService>,
    receiver: Arc<FileTransferService>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn manager(id: DeviceId, root: &std::path::Path) -> ConnectionManager {
    let (cert_der, key_der) = Encryption::generate_cert().unwrap();
    ConnectionManager::with_transport(
        id,
        QuicTransport::with_identity(id, QuicIdentity { cert_der, key_der })
            .with_trust_store_path(root.join(format!("{id}.json"))),
    )
}

fn pump(
    mut events: mpsc::Receiver<ManagerEvent>,
    service: Arc<FileTransferService>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                ManagerEvent::BulkReceived { auth, frame } => {
                    if let Message::FileTransfer(packet) = frame.into_message() {
                        service.receive(auth.peer_id, auth.control_connection_id, packet);
                    }
                }
                ManagerEvent::Disconnected {
                    peer_id,
                    control_connection_id,
                } => service.disconnect(peer_id, control_connection_id),
                _ => {}
            }
        }
    })
}

impl Pair {
    async fn new() -> Self {
        Self::with_resolver(None).await
    }

    async fn with_resolver(
        resolver: Option<Arc<dyn rshare_net::file_transfer::FolderDropResolver>>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let a_id = DeviceId::new_v4();
        let b_id = DeviceId::new_v4();
        let mut a = manager(a_id, root.path());
        let mut b = manager(b_id, root.path());
        let a_events = a.events().unwrap();
        let b_events = b.events().unwrap();
        let mut a_peers = a.authenticated_peers().unwrap();
        let mut b_peers = b.authenticated_peers().unwrap();
        let sender = FileTransferService::new(a.qos_registry(), root.path().join("a-received"));
        let folder_test = resolver.is_some();
        let receiver = FileTransferService::with_folder_resolver(
            b.qos_registry(),
            root.path().join("b-received"),
            resolver,
        );
        b.start_server("127.0.0.1:0").await.unwrap();
        a.connect(b_id, &b.transport_local_addr().unwrap().to_string())
            .await
            .unwrap();
        timeout(Duration::from_secs(3), async {
            while b.qos_registry().peer(&a_id).is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if folder_test {
            // The test resolver models the native platform seam even on Linux.
            let mut peer = a.qos_registry().peer(&b_id).unwrap();
            peer.folder_drop_version = 1;
            a.qos_registry().insert(b_id, peer);
            let mut peer = b.qos_registry().peer(&a_id).unwrap();
            peer.folder_drop_version = 1;
            b.qos_registry().insert(a_id, peer);
        }
        // Production input runtime drains this independent bulk subscription.
        // Mirror that consumer so its bounded queue cannot stall event delivery.
        let mut a_inbound = a_peers.recv().await.unwrap();
        let mut b_inbound = b_peers.recv().await.unwrap();
        let tasks = vec![
            pump(a_events, sender.clone()),
            pump(b_events, receiver.clone()),
            tokio::spawn(async move { while a_inbound.bulk_rx.recv().await.is_some() {} }),
            tokio::spawn(async move { while b_inbound.bulk_rx.recv().await.is_some() {} }),
        ];
        Self {
            _root: root,
            a_id,
            b_id,
            a,
            b,
            sender,
            receiver,
            tasks,
        }
    }

    async fn wait(&self, service: &FileTransferService, id: DeviceId) -> FileTransferSnapshot {
        timeout(Duration::from_secs(15), async {
            loop {
                if let Some(result) = service
                    .snapshots()
                    .into_iter()
                    .find(|s| s.id == id && s.status.is_terminal())
                {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "file transfer did not finish: {error}; sender={:?}; receiver={:?}",
                self.sender.snapshots(),
                self.receiver.snapshots()
            )
        })
    }

    async fn close(mut self) {
        let _ = self.a.disconnect(&self.b_id).await;
        let _ = self.b.disconnect(&self.a_id).await;
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
    }
}

#[tokio::test]
async fn file_drop_copies_bytes_empty_files_folders_and_same_names_over_real_quic() {
    let pair = Pair::new().await;
    let source = pair._root.path().join("报告");
    std::fs::create_dir_all(source.join("空目录")).unwrap();
    let bytes: Vec<u8> = (0..FILE_CHUNK_BYTES * 3 + 19)
        .map(|n| (n % 251) as u8)
        .collect();
    std::fs::write(source.join("数据.bin"), &bytes).unwrap();
    std::fs::write(source.join("空.txt"), []).unwrap();
    let mut destinations = Vec::new();
    for _ in 0..2 {
        let sent = pair
            .sender
            .send_files(pair.b_id, vec![source.to_string_lossy().into()])
            .unwrap();
        let sent = pair.wait(&pair.sender, sent.id).await;
        assert_eq!(
            sent.status,
            FileTransferStatus::Completed,
            "{:?}",
            sent.error
        );
        assert_eq!(sent.transferred_bytes, bytes.len() as u64);
        let received = pair.wait(&pair.receiver, sent.id).await;
        assert_eq!(received.status, FileTransferStatus::Completed);
        let dest = std::path::PathBuf::from(received.destination.unwrap());
        assert_eq!(std::fs::read(dest.join("报告/数据.bin")).unwrap(), bytes);
        assert_eq!(
            std::fs::read(dest.join("报告/空.txt")).unwrap(),
            Vec::<u8>::new()
        );
        assert!(dest.join("报告/空目录").is_dir());
        destinations.push(dest);
    }
    assert_ne!(destinations[0], destinations[1]);
    assert!(std::fs::read_dir(pair._root.path().join("b-received"))
        .unwrap()
        .all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".partial")));
    pair.close().await;
}

#[tokio::test]
async fn file_drop_rejects_old_peers_offline_targets_and_missing_files() {
    let pair = Pair::new().await;
    assert!(pair
        .sender
        .send_files(DeviceId::new_v4(), vec!["/missing".into()])
        .is_err());
    let sent = pair
        .sender
        .send_files(pair.b_id, vec!["/rshare-test-does-not-exist".into()])
        .unwrap();
    assert_eq!(
        pair.wait(&pair.sender, sent.id).await.status,
        FileTransferStatus::Failed
    );
    let mut old_peer = pair.a.qos_registry().peer(&pair.b_id).unwrap();
    old_peer.file_transfer_version = 0;
    pair.a.qos_registry().insert(pair.b_id, old_peer);
    assert!(pair
        .sender
        .send_files(pair.b_id, vec!["/missing".into()])
        .unwrap_err()
        .to_string()
        .contains("更新"));
    pair.close().await;
}

#[tokio::test]
async fn file_drop_rejects_corrupt_content_and_cleans_staging() {
    let pair = Pair::new().await;
    let peer = pair.a.qos_registry().peer(&pair.b_id).unwrap();
    let id = DeviceId::new_v4();
    let bodies = [
        FileTransferBody::Offer {
            entries: vec![FileEntry {
                path: "test.bin".into(),
                size: 1,
                directory: false,
            }],
        },
        FileTransferBody::Chunk {
            index: 0,
            offset: 0,
            data: vec![7],
        },
        FileTransferBody::FileEnd {
            index: 0,
            sha256: [0; 32],
        },
    ];
    for (sequence, body) in bodies.into_iter().enumerate() {
        peer.transport
            .send_bulk(BulkFrame::file_transfer(FileTransferPacket {
                transfer_id: id,
                sequence: sequence as u64,
                body,
            }))
            .await
            .unwrap();
    }
    let received = pair.wait(&pair.receiver, id).await;
    assert_eq!(received.status, FileTransferStatus::Failed);
    assert!(received.error.unwrap().contains("完整性"));
    assert_eq!(
        std::fs::read_dir(pair._root.path().join("b-received"))
            .unwrap()
            .count(),
        0
    );
    pair.close().await;
}

#[tokio::test]
async fn file_drop_cancel_disconnect_and_stale_generation_remove_partial_files() {
    let pair = Pair::new().await;
    for disconnect in [false, true] {
        let id = DeviceId::new_v4();
        let peer = pair.b.qos_registry().peer(&pair.a_id).unwrap();
        let offer = FileTransferPacket {
            transfer_id: id,
            sequence: 0,
            body: FileTransferBody::Offer {
                entries: vec![FileEntry {
                    path: "test.bin".into(),
                    size: 9,
                    directory: false,
                }],
            },
        };
        pair.receiver.receive(
            pair.a_id,
            rshare_core::ControlConnectionId::new(),
            offer.clone(),
        );
        assert!(!pair.receiver.snapshots().iter().any(|s| s.id == id));
        pair.receiver
            .receive(pair.a_id, peer.auth.control_connection_id, offer);
        timeout(Duration::from_secs(3), async {
            while !pair._root.path().join("b-received").exists()
                || pair
                    .receiver
                    .snapshots()
                    .iter()
                    .any(|s| s.id == id && s.status == FileTransferStatus::Preparing)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if disconnect {
            pair.receiver
                .disconnect(pair.a_id, peer.auth.control_connection_id);
        } else {
            pair.receiver.cancel(id).unwrap();
        }
        let received = pair.wait(&pair.receiver, id).await;
        assert_eq!(received.status, FileTransferStatus::Cancelled);
        assert_eq!(
            std::fs::read_dir(pair._root.path().join("b-received"))
                .unwrap()
                .count(),
            0
        );
    }
    pair.close().await;
}

#[tokio::test]
async fn file_drop_invalid_chunk_offsets_and_sizes_never_commit() {
    let pair = Pair::new().await;
    let peer = pair.a.qos_registry().peer(&pair.b_id).unwrap();
    for (offset, data) in [
        (1, vec![1]),
        (0, vec![1; FILE_CHUNK_BYTES + 1]),
        (0, vec![1; 10]),
    ] {
        let id = DeviceId::new_v4();
        for (sequence, body) in [
            FileTransferBody::Offer {
                entries: vec![FileEntry {
                    path: "test.bin".into(),
                    size: 9,
                    directory: false,
                }],
            },
            FileTransferBody::Chunk {
                index: 0,
                offset,
                data,
            },
        ]
        .into_iter()
        .enumerate()
        {
            peer.transport
                .send_bulk(BulkFrame::file_transfer(FileTransferPacket {
                    transfer_id: id,
                    sequence: sequence as u64,
                    body,
                }))
                .await
                .unwrap();
        }
        let received = pair.wait(&pair.receiver, id).await;
        assert_eq!(received.status, FileTransferStatus::Failed);
        assert_eq!(
            std::fs::read_dir(pair._root.path().join("b-received"))
                .unwrap()
                .count(),
            0
        );
    }
    pair.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn file_drop_does_not_follow_symlinks_or_send_duplicate_root_names() {
    let pair = Pair::new().await;
    let original = pair._root.path().join("original");
    std::fs::write(&original, b"private").unwrap();
    let link = pair._root.path().join("link");
    std::os::unix::fs::symlink(&original, &link).unwrap();
    for paths in [
        vec![link.to_string_lossy().into_owned()],
        vec![original.to_string_lossy().into_owned(); 2],
    ] {
        let transfer = pair.sender.send_files(pair.b_id, paths).unwrap();
        assert_eq!(
            pair.wait(&pair.sender, transfer.id).await.status,
            FileTransferStatus::Failed
        );
    }
    assert!(pair.receiver.snapshots().is_empty());
    pair.close().await;
}

#[tokio::test]
async fn file_drop_admission_is_bounded_and_source_cancel_is_observed() {
    let pair = Pair::new().await;
    let peer = pair.b.qos_registry().peer(&pair.a_id).unwrap();
    let mut ids = Vec::new();
    for _ in 0..5 {
        let id = DeviceId::new_v4();
        ids.push(id);
        pair.receiver.receive(
            pair.a_id,
            peer.auth.control_connection_id,
            FileTransferPacket {
                transfer_id: id,
                sequence: 0,
                body: FileTransferBody::Offer {
                    entries: vec![FileEntry {
                        path: "pending".into(),
                        size: 10,
                        directory: false,
                    }],
                },
            },
        );
    }
    assert_eq!(pair.receiver.snapshots().len(), 4);
    for id in ids.into_iter().take(4) {
        pair.receiver.cancel(id).unwrap();
        assert_eq!(
            pair.wait(&pair.receiver, id).await.status,
            FileTransferStatus::Cancelled
        );
    }
    let source = pair._root.path().join("cancel.bin");
    std::fs::write(&source, b"cancel before preparation").unwrap();
    let sent = pair
        .sender
        .send_files(pair.b_id, vec![source.to_string_lossy().into()])
        .unwrap();
    pair.sender.cancel(sent.id).unwrap();
    assert_eq!(
        pair.wait(&pair.sender, sent.id).await.status,
        FileTransferStatus::Cancelled
    );
    pair.close().await;
}
