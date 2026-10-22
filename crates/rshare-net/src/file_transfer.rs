//! File drops use authenticated, generation-bound bulk lanes and bounded workers.
use crate::qos::{BulkFrame, ConnectionRegistry, RegisteredPeer};
use anyhow::{bail, Context, Result};
use rshare_core::{file_transfer::*, ControlConnectionId, DeviceId};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, watch},
    time::timeout,
};

const MAX_ACTIVE: usize = 4;
const HISTORY: usize = 64;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_GLOBAL_RESERVED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_PEER_RESERVED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_PUBLISHED_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_PEER_PUBLISHED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MIN_FREE_BYTES_AFTER_RESERVATION: u64 = 256 * 1024 * 1024;
const MAX_PENDING_OFFERS: usize = 32;
const MAX_PENDING_OFFERS_PER_PEER: usize = 4;
const PENDING_OFFER_TTL: Duration = Duration::from_secs(30);

pub trait FolderDropResolver: Send + Sync {
    fn resolve(
        &self,
        owner: rshare_core::AuthenticatedInputOwner,
        point: FolderDropPoint,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf>> + Send + '_>>;
}

struct Job {
    snapshot: FileTransferSnapshot,
    generation: ControlConnectionId,
    reserved_bytes: u64,
    inbox: mpsc::Sender<FileTransferPacket>,
    cancel: watch::Sender<Option<String>>,
}

#[derive(Default)]
struct Jobs {
    active: HashMap<DeviceId, Job>,
    pending: HashMap<DeviceId, PendingOffer>,
    history: VecDeque<FileTransferSnapshot>,
    reserved_bytes: u64,
    reserved_by_peer: HashMap<DeviceId, u64>,
    published_bytes: u64,
    published_by_peer: HashMap<DeviceId, u64>,
}

struct PendingOffer {
    peer: RegisteredPeer,
    packet: FileTransferPacket,
    snapshot: FileTransferSnapshot,
    expires_at: Instant,
}

pub struct FileTransferService {
    registry: Arc<ConnectionRegistry>,
    root: PathBuf,
    jobs: Mutex<Jobs>,
    rejections: mpsc::Sender<(RegisteredPeer, FileTransferPacket)>,
    folder_resolver: Option<Arc<dyn FolderDropResolver>>,
}

struct Session {
    service: Arc<FileTransferService>,
    id: DeviceId,
    peer: RegisteredPeer,
    inbox: mpsc::Receiver<FileTransferPacket>,
    cancel: watch::Receiver<Option<String>>,
    // Committing a receive removes the registry entry before the final ACK.
    // Keep this channel alive so that cleanup is not mistaken for cancellation.
    _cancel_guard: watch::Sender<Option<String>>,
}

impl FileTransferService {
    pub fn new(registry: Arc<ConnectionRegistry>, root: PathBuf) -> Arc<Self> {
        Self::with_folder_resolver(registry, root, None)
    }

    pub fn with_folder_resolver(
        registry: Arc<ConnectionRegistry>,
        root: PathBuf,
        folder_resolver: Option<Arc<dyn FolderDropResolver>>,
    ) -> Arc<Self> {
        let published_bytes = directory_bytes(&root);
        let (rejections, mut rx) = mpsc::channel::<(RegisteredPeer, FileTransferPacket)>(16);
        tokio::spawn(async move {
            while let Some((peer, packet)) = rx.recv().await {
                let _ = timeout(
                    Duration::from_secs(1),
                    peer.transport.send_bulk(BulkFrame::file_transfer(packet)),
                )
                .await;
            }
        });
        Arc::new(Self {
            registry,
            root,
            jobs: Mutex::new(Jobs {
                published_bytes,
                ..Jobs::default()
            }),
            rejections,
            folder_resolver,
        })
    }

    pub fn snapshots(&self) -> Vec<FileTransferSnapshot> {
        let mut jobs = self.jobs.lock().unwrap();
        prune_pending(&mut jobs, Instant::now());
        let mut active: Vec<_> = jobs.active.values().map(|j| j.snapshot.clone()).collect();
        active.sort_by_key(|s| s.id);
        let mut pending: Vec<_> = jobs
            .pending
            .values()
            .map(|offer| offer.snapshot.clone())
            .collect();
        pending.sort_by_key(|s| s.id);
        active.extend(pending);
        active.extend(jobs.history.iter().rev().cloned());
        active
    }

    /// Native gesture failures belong in the same user-visible transfer history.
    pub fn record_folder_drag_failure(&self, peer_id: DeviceId, error: String) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.history.push_back(FileTransferSnapshot {
            id: DeviceId::new_v4(),
            peer_id,
            incoming: false,
            status: FileTransferStatus::Failed,
            entries: vec![],
            entry_count: 0,
            total_bytes: 0,
            transferred_bytes: 0,
            destination: None,
            error: Some(error),
        });
        while jobs.history.len() > HISTORY {
            jobs.history.pop_front();
        }
    }

    pub fn cancel(&self, id: DeviceId) -> Result<()> {
        let mut jobs = self.jobs.lock().unwrap();
        prune_pending(&mut jobs, Instant::now());
        if let Some(job) = jobs.active.get(&id) {
            let _ = job.cancel.send(Some("用户取消传输".into()));
            return Ok(());
        }
        if let Some(mut offer) = jobs.pending.remove(&id) {
            let _ = self.rejections.try_send((
                offer.peer.clone(),
                FileTransferPacket {
                    transfer_id: id,
                    sequence: offer.packet.sequence,
                    body: FileTransferBody::Cancel {
                        reason: "本机用户拒绝文件接收".into(),
                    },
                },
            ));
            offer.snapshot.status = FileTransferStatus::Cancelled;
            offer.snapshot.error = Some("本机用户拒绝文件接收".into());
            jobs.history.push_back(offer.snapshot);
            while jobs.history.len() > HISTORY {
                jobs.history.pop_front();
            }
            return Ok(());
        }
        if jobs.history.iter().any(|s| s.id == id) {
            return Ok(());
        }
        bail!("传输不存在")
    }

    /// Approve one visible incoming offer. The exact transfer ID is surfaced
    /// through `snapshots`; no inbound offer is acknowledged before this call.
    pub fn approve_incoming(self: &Arc<Self>, id: DeviceId) -> Result<()> {
        let offer = {
            let mut jobs = self.jobs.lock().unwrap();
            prune_pending(&mut jobs, Instant::now());
            jobs.pending.remove(&id)
        }
        .with_context(|| "待审批的文件传输不存在、已过期或已处理")?;

        let total = offer.snapshot.total_bytes;
        match self.register(id, offer.peer.clone(), true, total) {
            Ok(session) => {
                tokio::spawn(session.run(None, Some(offer.packet), None));
                Ok(())
            }
            Err(error) => {
                let reason = error.to_string();
                let _ = self.rejections.try_send((
                    offer.peer,
                    FileTransferPacket {
                        transfer_id: id,
                        sequence: 0,
                        body: FileTransferBody::Cancel {
                            reason: reason.clone(),
                        },
                    },
                ));
                let mut snapshot = offer.snapshot;
                snapshot.status = FileTransferStatus::Failed;
                snapshot.error = Some(reason.clone());
                let mut jobs = self.jobs.lock().unwrap();
                jobs.history.push_back(snapshot);
                while jobs.history.len() > HISTORY {
                    jobs.history.pop_front();
                }
                bail!(reason)
            }
        }
    }

    pub fn disconnect(&self, peer: DeviceId, generation: ControlConnectionId) {
        let mut jobs = self.jobs.lock().unwrap();
        prune_pending(&mut jobs, Instant::now());
        let pending_ids: Vec<_> = jobs
            .pending
            .iter()
            .filter(|(_, offer)| {
                offer.peer.auth.peer_id == peer
                    && offer.peer.auth.control_connection_id == generation
            })
            .map(|(id, _)| *id)
            .collect();
        for id in pending_ids {
            if let Some(mut offer) = jobs.pending.remove(&id) {
                offer.snapshot.status = FileTransferStatus::Cancelled;
                offer.snapshot.error = Some("设备连接已断开，请重新拖入文件".into());
                jobs.history.push_back(offer.snapshot);
            }
        }
        while jobs.history.len() > HISTORY {
            jobs.history.pop_front();
        }
        for job in jobs.active.values() {
            if job.snapshot.peer_id == peer && job.generation == generation {
                let _ = job
                    .cancel
                    .send(Some("设备连接已断开，请重新拖入文件".into()));
            }
        }
    }

    fn register(
        self: &Arc<Self>,
        id: DeviceId,
        peer: RegisteredPeer,
        incoming: bool,
        reserved_bytes: u64,
    ) -> Result<Session> {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.active.len() >= MAX_ACTIVE {
            bail!("最多同时进行 4 个文件传输");
        }
        if jobs.active.contains_key(&id) || jobs.history.iter().any(|s| s.id == id) {
            bail!("重复的文件传输标识");
        }
        if incoming && reserved_bytes > 0 {
            if reserved_bytes > MAX_PEER_RESERVED_BYTES
                || jobs
                    .reserved_by_peer
                    .get(&peer.auth.peer_id)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(reserved_bytes)
                    > MAX_PEER_RESERVED_BYTES
            {
                bail!("该设备的文件暂存配额已用尽");
            }
            if jobs.reserved_bytes.saturating_add(reserved_bytes) > MAX_GLOBAL_RESERVED_BYTES {
                bail!("文件暂存总配额已用尽");
            }
            if jobs
                .published_bytes
                .saturating_add(jobs.reserved_bytes)
                .saturating_add(reserved_bytes)
                > MAX_PUBLISHED_BYTES
            {
                bail!("文件接收目录配额已用尽");
            }
            if jobs
                .published_by_peer
                .get(&peer.auth.peer_id)
                .copied()
                .unwrap_or_default()
                .saturating_add(
                    jobs.reserved_by_peer
                        .get(&peer.auth.peer_id)
                        .copied()
                        .unwrap_or_default(),
                )
                .saturating_add(reserved_bytes)
                > MAX_PEER_PUBLISHED_BYTES
            {
                bail!("该设备的文件接收配额已用尽");
            }
            if !available_space(&self.root).is_some_and(|free| {
                free >= reserved_bytes.saturating_add(MIN_FREE_BYTES_AFTER_RESERVATION)
            }) {
                bail!("磁盘剩余空间不足，拒绝文件接收");
            }
            jobs.reserved_bytes = jobs.reserved_bytes.saturating_add(reserved_bytes);
            let reserved = jobs.reserved_by_peer.entry(peer.auth.peer_id).or_default();
            *reserved = reserved.saturating_add(reserved_bytes);
        }
        let (tx, inbox) = mpsc::channel(4);
        let (cancel_tx, cancel) = watch::channel(None);
        jobs.active.insert(
            id,
            Job {
                snapshot: FileTransferSnapshot {
                    id,
                    peer_id: peer.auth.peer_id,
                    incoming,
                    status: FileTransferStatus::Preparing,
                    entries: vec![],
                    entry_count: 0,
                    total_bytes: 0,
                    transferred_bytes: 0,
                    destination: None,
                    error: None,
                },
                generation: peer.auth.control_connection_id,
                reserved_bytes,
                inbox: tx,
                cancel: cancel_tx.clone(),
            },
        );
        Ok(Session {
            service: self.clone(),
            id,
            peer,
            inbox,
            cancel,
            _cancel_guard: cancel_tx,
        })
    }

    pub fn send_files(
        self: &Arc<Self>,
        peer_id: DeviceId,
        paths: Vec<String>,
    ) -> Result<FileTransferSnapshot> {
        self.start_send(peer_id, paths, None)
    }

    pub fn send_files_to_folder(
        self: &Arc<Self>,
        peer_id: DeviceId,
        paths: Vec<String>,
        point: FolderDropPoint,
    ) -> Result<FileTransferSnapshot> {
        self.start_send(peer_id, paths, Some(point))
    }

    fn start_send(
        self: &Arc<Self>,
        peer_id: DeviceId,
        paths: Vec<String>,
        drop: Option<FolderDropPoint>,
    ) -> Result<FileTransferSnapshot> {
        if paths.is_empty() || paths.len() > MAX_FILE_ENTRIES {
            bail!("请拖入 1–1024 个文件或目录");
        }
        let peer = self.registry.peer(&peer_id).context("目标设备尚未连接")?;
        if peer.file_transfer_version != 1 {
            bail!("目标设备不支持文件拖拽，请先更新远端 R-ShareMouse");
        }
        if drop.is_some() && peer.folder_drop_version != 1 {
            bail!("远端不支持文件夹跨屏拖拽，请更新远端或检查平台支持");
        }
        let id = DeviceId::new_v4();
        let session = self.register(id, peer, false, 0)?;
        let snapshot = self.jobs.lock().unwrap().active[&id].snapshot.clone();
        tokio::spawn(session.run(Some(paths), None, drop));
        Ok(snapshot)
    }

    /// Called only for authenticated bulk messages. Never performs filesystem I/O
    /// or waits for a worker while the daemon's network event loop is dispatching.
    pub fn receive(
        self: &Arc<Self>,
        peer_id: DeviceId,
        generation: ControlConnectionId,
        packet: FileTransferPacket,
    ) {
        let Some(peer) = self
            .registry
            .peer(&peer_id)
            .filter(|p| p.auth.control_connection_id == generation && p.file_transfer_version == 1)
        else {
            return;
        };
        if let FileTransferBody::Offer { entries }
        | FileTransferBody::OfferToFolder { entries, .. } = &packet.body
        {
            let result = validate_manifest(entries).and_then(|total| {
                if matches!(&packet.body, FileTransferBody::OfferToFolder { .. })
                    && (peer.folder_drop_version != 1 || self.folder_resolver.is_none())
                {
                    bail!("此设备没有可用的文件夹落点识别服务");
                }
                if packet.sequence != 0 {
                    bail!("无效的文件传输起始序号");
                }
                self.queue_pending_offer(packet.transfer_id, peer.clone(), packet.clone(), total)
            });
            match result {
                Ok(()) => {}
                Err(error) => {
                    // Reject without retaining another unbounded transfer worker.
                    // The peer will also time out if a saturated lane cannot send.
                    let response = FileTransferPacket {
                        transfer_id: packet.transfer_id,
                        sequence: packet.sequence,
                        body: FileTransferBody::Cancel {
                            reason: error.to_string(),
                        },
                    };
                    let _ = self.rejections.try_send((peer, response));
                }
            }
            return;
        }
        if let FileTransferBody::Cancel { reason } = &packet.body {
            let mut jobs = self.jobs.lock().unwrap();
            let pending_matches = jobs.pending.get(&packet.transfer_id).is_some_and(|offer| {
                offer.peer.auth.peer_id == peer_id
                    && offer.peer.auth.control_connection_id == generation
            });
            if pending_matches {
                let mut offer = jobs.pending.remove(&packet.transfer_id).unwrap();
                offer.snapshot.status = FileTransferStatus::Cancelled;
                offer.snapshot.error = Some(format!(
                    "远端取消：{}",
                    reason.chars().take(240).collect::<String>()
                ));
                jobs.history.push_back(offer.snapshot);
                while jobs.history.len() > HISTORY {
                    jobs.history.pop_front();
                }
                return;
            }
        }
        let jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs
            .active
            .get(&packet.transfer_id)
            .filter(|j| j.snapshot.peer_id == peer_id && j.generation == generation)
        {
            if let FileTransferBody::Cancel { reason } = &packet.body {
                let _ = job.cancel.send(Some(format!(
                    "远端取消：{}",
                    reason.chars().take(240).collect::<String>()
                )));
            } else if job.inbox.try_send(packet).is_err() {
                let _ = job.cancel.send(Some("文件传输队列溢出".into()));
            }
        }
    }

    fn update(&self, id: DeviceId, f: impl FnOnce(&mut FileTransferSnapshot)) {
        if let Some(job) = self.jobs.lock().unwrap().active.get_mut(&id) {
            f(&mut job.snapshot);
        }
    }

    fn queue_pending_offer(
        &self,
        id: DeviceId,
        peer: RegisteredPeer,
        packet: FileTransferPacket,
        total: u64,
    ) -> Result<()> {
        let mut jobs = self.jobs.lock().unwrap();
        prune_pending(&mut jobs, Instant::now());
        if jobs.active.contains_key(&id)
            || jobs.pending.contains_key(&id)
            || jobs.history.iter().any(|s| s.id == id)
        {
            bail!("重复的文件传输标识");
        }
        if jobs.pending.len() >= MAX_PENDING_OFFERS {
            bail!("待审批的文件传输过多，请稍后重试");
        }
        let peer_pending = jobs
            .pending
            .values()
            .filter(|offer| offer.peer.auth.peer_id == peer.auth.peer_id)
            .count();
        if peer_pending >= MAX_PENDING_OFFERS_PER_PEER {
            bail!("该设备的待审批文件传输过多");
        }
        let (entries, entry_count) = match &packet.body {
            FileTransferBody::Offer { entries }
            | FileTransferBody::OfferToFolder { entries, .. } => {
                (entries.iter().take(8).cloned().collect(), entries.len())
            }
            _ => unreachable!("queue_pending_offer only accepts file offers"),
        };
        jobs.pending.insert(
            id,
            PendingOffer {
                peer: peer.clone(),
                packet,
                snapshot: FileTransferSnapshot {
                    id,
                    peer_id: peer.auth.peer_id,
                    incoming: true,
                    status: FileTransferStatus::Waiting,
                    entries,
                    entry_count,
                    total_bytes: total,
                    transferred_bytes: 0,
                    destination: None,
                    error: None,
                },
                expires_at: Instant::now() + PENDING_OFFER_TTL,
            },
        );
        Ok(())
    }

    fn finish(&self, id: DeviceId, status: FileTransferStatus, error: Option<String>) {
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(mut job) = jobs.active.remove(&id) {
            if job.reserved_bytes > 0 {
                jobs.reserved_bytes = jobs.reserved_bytes.saturating_sub(job.reserved_bytes);
                if let Some(reserved) = jobs.reserved_by_peer.get_mut(&job.snapshot.peer_id) {
                    *reserved = reserved.saturating_sub(job.reserved_bytes);
                    if *reserved == 0 {
                        jobs.reserved_by_peer.remove(&job.snapshot.peer_id);
                    }
                }
            }
            job.snapshot.status = status;
            job.snapshot.error = error;
            if status == FileTransferStatus::Completed && job.snapshot.incoming {
                jobs.published_bytes = jobs
                    .published_bytes
                    .saturating_add(job.snapshot.total_bytes);
                let published = jobs
                    .published_by_peer
                    .entry(job.snapshot.peer_id)
                    .or_default();
                *published = published.saturating_add(job.snapshot.total_bytes);
            }
            jobs.history.push_back(job.snapshot);
            while jobs.history.len() > HISTORY {
                jobs.history.pop_front();
            }
        }
    }
}

fn prune_pending(jobs: &mut Jobs, now: Instant) {
    jobs.pending.retain(|_, offer| offer.expires_at > now);
}

impl Session {
    async fn run(
        mut self,
        paths: Option<Vec<String>>,
        offer: Option<FileTransferPacket>,
        drop: Option<FolderDropPoint>,
    ) {
        // Observe cancellation between filesystem operations. Dropping an
        // in-flight tokio file write can keep its Windows handle open while
        // TempDir attempts cleanup, leaving partial files behind.
        let result = async {
            self.check_cancelled()?;
            if let Some(paths) = paths {
                self.send(paths, drop).await
            } else {
                self.receive(offer.context("缺少文件清单")?).await
            }
        }
        .await;
        match result {
            Ok(()) => self
                .service
                .finish(self.id, FileTransferStatus::Completed, None),
            Err(error) => {
                let reason = error.to_string();
                let cancelled = self.cancel.borrow().is_some();
                let _ = timeout(
                    Duration::from_secs(1),
                    self.write(
                        0,
                        FileTransferBody::Cancel {
                            reason: reason.clone(),
                        },
                    ),
                )
                .await;
                self.service.finish(
                    self.id,
                    if cancelled {
                        FileTransferStatus::Cancelled
                    } else {
                        FileTransferStatus::Failed
                    },
                    Some(reason),
                );
            }
        }
    }

    async fn write(&self, sequence: u64, body: FileTransferBody) -> Result<()> {
        let terminal = matches!(
            body,
            FileTransferBody::Cancel { .. } | FileTransferBody::Completed
        );
        let current = self
            .service
            .registry
            .peer(&self.peer.auth.peer_id)
            .context("设备连接已断开")?;
        if current.auth.control_connection_id != self.peer.auth.control_connection_id {
            bail!("设备连接已更换，请重新拖入文件");
        }
        let send = timeout(
            IDLE_TIMEOUT,
            self.peer
                .transport
                .send_bulk(BulkFrame::file_transfer(FileTransferPacket {
                    transfer_id: self.id,
                    sequence,
                    body,
                })),
        );
        tokio::select! {
            biased;
            reason = wait_for_cancel(self.cancel.clone()), if !terminal => bail!(reason),
            result = send => { result.context("文件发送超时")??; }
        }
        Ok(())
    }

    fn check_cancelled(&self) -> Result<()> {
        if let Some(reason) = self.cancel.borrow().clone() {
            bail!(reason);
        }
        Ok(())
    }

    async fn next(&mut self) -> Result<FileTransferPacket> {
        tokio::select! {
            biased;
            reason = wait_for_cancel(self.cancel.clone()) => bail!(reason),
            result = timeout(IDLE_TIMEOUT, self.inbox.recv()) => result
                .context("文件传输超时；请确认两端均为支持文件拖拽的版本")?.context("文件传输已关闭"),
        }
    }

    async fn exchange(
        &mut self,
        sequence: u64,
        body: FileTransferBody,
        expected: FileTransferBody,
    ) -> Result<()> {
        self.write(sequence, body).await?;
        let reply = self.next().await?;
        if let FileTransferBody::Cancel { reason } = &reply.body {
            bail!("远端拒绝文件投放：{reason}");
        }
        if reply.sequence != sequence || reply.body != expected {
            bail!("无效的文件传输确认");
        }
        Ok(())
    }

    async fn send(&mut self, paths: Vec<String>, drop: Option<FolderDropPoint>) -> Result<()> {
        let sources = collect_sources(paths).await?;
        self.check_cancelled()?;
        let entries: Vec<_> = sources.iter().map(|(e, _)| e.clone()).collect();
        let total = validate_manifest(&entries)?;
        self.service.update(self.id, |s| {
            s.entries = entries.iter().take(8).cloned().collect();
            s.entry_count = entries.len();
            s.total_bytes = total;
            s.status = FileTransferStatus::Waiting;
        });
        let offer = match drop {
            Some(drop) => FileTransferBody::OfferToFolder { entries, drop },
            None => FileTransferBody::Offer { entries },
        };
        self.exchange(0, offer, FileTransferBody::Accepted).await?;
        self.service
            .update(self.id, |s| s.status = FileTransferStatus::Transferring);
        let mut sequence = 1;
        for (index, (entry, path)) in sources.into_iter().enumerate() {
            if entry.directory {
                continue;
            }
            if fs::symlink_metadata(&path).await?.file_type().is_symlink() {
                bail!("不发送符号链接");
            }
            let mut file = fs::File::open(&path).await.context("无法读取拖入文件")?;
            if !file.metadata().await?.is_file() {
                bail!("只支持普通文件和目录");
            }
            let mut hash = Sha256::new();
            let mut offset = 0;
            let mut buffer = vec![0; FILE_CHUNK_BYTES];
            loop {
                let count = file.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                if offset + count as u64 > entry.size {
                    bail!("源文件在传输期间发生变化");
                }
                hash.update(&buffer[..count]);
                self.exchange(
                    sequence,
                    FileTransferBody::Chunk {
                        index: index as u32,
                        offset,
                        data: buffer[..count].to_vec(),
                    },
                    FileTransferBody::Received,
                )
                .await?;
                offset += count as u64;
                self.service
                    .update(self.id, |s| s.transferred_bytes += count as u64);
                sequence += 1;
            }
            if offset != entry.size {
                bail!("源文件在传输期间发生变化");
            }
            self.exchange(
                sequence,
                FileTransferBody::FileEnd {
                    index: index as u32,
                    sha256: hash.finalize().into(),
                },
                FileTransferBody::Received,
            )
            .await?;
            sequence += 1;
        }
        self.exchange(
            sequence,
            FileTransferBody::Complete,
            FileTransferBody::Completed,
        )
        .await
    }

    async fn receive(&mut self, offer: FileTransferPacket) -> Result<()> {
        let (entries, folder_drop) = match offer.body {
            FileTransferBody::Offer { entries } => (entries, None),
            FileTransferBody::OfferToFolder { entries, drop } => (entries, Some(drop)),
            _ => bail!("缺少文件清单"),
        };
        let total = validate_manifest(&entries)?;
        let into_folder = folder_drop.is_some();
        let root = if let Some(point) = folder_drop {
            let resolver = self
                .service
                .folder_resolver
                .as_ref()
                .context("文件夹落点识别不可用")?;
            let owner = rshare_core::AuthenticatedInputOwner {
                peer_id: self.peer.auth.peer_id,
                control_connection_id: self.peer.auth.control_connection_id,
            };
            let path = resolver.resolve(owner, point).await?;
            let path = fs::canonicalize(path).await.context("目标文件夹已不存在")?;
            if !fs::metadata(&path).await?.is_dir() {
                bail!("落点不是文件夹");
            }
            path
        } else {
            self.service.root.clone()
        };
        if !into_folder {
            fs::create_dir_all(&root)
                .await
                .context("无法创建接收目录")?;
        }
        let staging = tempfile::Builder::new()
            .prefix(".partial-")
            .tempdir_in(&root)?;
        let destination = if into_folder {
            root.clone()
        } else {
            root.join(self.id.to_string())
        };
        self.service.update(self.id, |s| {
            s.entries = entries.iter().take(8).cloned().collect();
            s.entry_count = entries.len();
            s.total_bytes = total;
            s.status = FileTransferStatus::Transferring;
        });
        for entry in &entries {
            self.check_cancelled()?;
            let path = staging.path().join(&entry.path);
            if entry.directory {
                fs::create_dir_all(path).await?;
            } else if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
        }
        self.write(0, FileTransferBody::Accepted).await?;
        let mut sequence = 1;
        for (index, entry) in entries.iter().enumerate() {
            if entry.directory {
                continue;
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staging.path().join(&entry.path))
                .await?;
            let mut hash = Sha256::new();
            let mut offset = 0;
            loop {
                let packet = self.next().await?;
                if packet.sequence != sequence {
                    bail!("文件块顺序错误");
                }
                match packet.body {
                    FileTransferBody::Chunk {
                        index: file_index,
                        offset: at,
                        data,
                    } => {
                        if file_index as usize != index
                            || at != offset
                            || data.is_empty()
                            || data.len() > FILE_CHUNK_BYTES
                            || offset + data.len() as u64 > entry.size
                        {
                            bail!("文件块超出声明范围");
                        }
                        file.write_all(&data).await?;
                        // Drain Tokio's buffered blocking write before observing
                        // cancellation, so Windows can remove the staging tree.
                        file.flush().await?;
                        hash.update(&data);
                        offset += data.len() as u64;
                        self.service
                            .update(self.id, |s| s.transferred_bytes += data.len() as u64);
                    }
                    FileTransferBody::FileEnd {
                        index: file_index,
                        sha256,
                    } => {
                        if file_index as usize != index
                            || offset != entry.size
                            || <[u8; 32]>::from(hash.finalize()) != sha256
                        {
                            bail!("文件完整性校验失败");
                        }
                        file.flush().await?;
                        file.sync_all().await?;
                        self.write(sequence, FileTransferBody::Received).await?;
                        sequence += 1;
                        break;
                    }
                    _ => bail!("不合法的文件传输状态"),
                }
                self.write(sequence, FileTransferBody::Received).await?;
                sequence += 1;
            }
        }
        let packet = self.next().await?;
        if packet.sequence != sequence || packet.body != FileTransferBody::Complete {
            bail!("文件传输未完整结束");
        }
        if !into_folder && fs::try_exists(&destination).await? {
            bail!("接收目录已存在");
        }
        self.check_cancelled()?;
        if into_folder {
            self.service.update(self.id, |s| {
                s.destination = Some(destination.to_string_lossy().into_owned())
            });
            publish_folder_contents(staging.path(), &destination)
                .await
                .context("目标文件夹投放失败；已完成的项目保留，请检查目标文件夹")?;
        } else {
            rename_without_replace(staging.path().to_path_buf(), destination.clone()).await?;
        }
        self.service.update(self.id, |s| {
            s.destination = Some(destination.to_string_lossy().into_owned())
        });
        // A lost final acknowledgement cannot roll back a committed receive.
        self.service
            .finish(self.id, FileTransferStatus::Completed, None);
        let _ = self.write(sequence, FileTransferBody::Completed).await;
        Ok(())
    }
}

async fn wait_for_cancel(mut receiver: watch::Receiver<Option<String>>) -> String {
    loop {
        if let Some(reason) = receiver.borrow().clone() {
            return reason;
        }
        if receiver.changed().await.is_err() {
            return "文件传输服务关闭".into();
        }
    }
}

fn available_space(path: &Path) -> Option<u64> {
    let mut probe = path;
    while !probe.exists() {
        probe = probe.parent()?;
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::mem::MaybeUninit;
        use std::os::unix::ffi::OsStrExt;

        let path = CString::new(probe.as_os_str().as_bytes()).ok()?;
        let mut stats = MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: statvfs initializes the caller-provided struct on success.
        let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
        if result != 0 {
            return None;
        }
        let stats = unsafe { stats.assume_init() };
        Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
    }
    #[cfg(windows)]
    {
        use std::iter::once;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide_path: Vec<u16> = probe.as_os_str().encode_wide().chain(once(0)).collect();
        let mut available = 0_u64;
        let mut total = 0_u64;
        let mut free = 0_u64;
        // SAFETY: The path is NUL-terminated and all output pointers refer to
        // initialized stack storage owned by this call.
        let result = unsafe {
            GetDiskFreeSpaceExW(wide_path.as_ptr(), &mut available, &mut total, &mut free)
        };
        (result != 0).then_some(available)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = probe;
        None
    }
}

fn directory_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                return 0;
            };
            if metadata.file_type().is_symlink() {
                return 0;
            }
            if metadata.is_dir() {
                directory_bytes(&path)
            } else {
                metadata.len()
            }
        })
        .fold(0_u64, u64::saturating_add)
}

async fn publish_folder_contents(
    staging: &std::path::Path,
    destination: &std::path::Path,
) -> Result<()> {
    let mut entries = fs::read_dir(staging).await?;
    while let Some(entry) = entries.next_entry().await? {
        let original = entry.file_name();
        let original = original.to_str().context("文件名不是 Unicode")?;
        let directory = entry.file_type().await?.is_dir();
        let path = std::path::Path::new(original);
        let stem = if directory {
            original
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(original)
        };
        let extension = if directory {
            None
        } else {
            path.extension().and_then(|s| s.to_str())
        };
        let mut published = false;
        for copy in 1..=10_000 {
            let name = if copy == 1 {
                original.to_string()
            } else {
                format!(
                    "{stem} ({copy}){}",
                    extension.map(|e| format!(".{e}")).unwrap_or_default()
                )
            };
            match rename_without_replace(entry.path(), destination.join(name)).await {
                Ok(()) => {
                    published = true;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if !published {
            bail!("同名文件过多，无法生成安全的新文件名");
        }
    }
    Ok(())
}

async fn rename_without_replace(from: PathBuf, to: PathBuf) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
            let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
            #[cfg(target_os = "macos")]
            let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe {
                windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
            } != 0
            {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn collect_sources(paths: Vec<String>) -> Result<Vec<(FileEntry, PathBuf)>> {
    let mut pending = Vec::new();
    for path in paths {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            bail!("拖入文件必须使用绝对路径");
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("文件名不是有效的 Unicode")?
            .to_string();
        pending.push((path, name));
    }
    let mut sources = Vec::new();
    while let Some((path, relative)) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)
            .await
            .context("拖入文件不存在或无法读取")?;
        if metadata.file_type().is_symlink() || !(metadata.is_dir() || metadata.is_file()) {
            bail!("只支持普通文件和目录，不发送符号链接");
        }
        sources.push((
            FileEntry {
                path: relative.clone(),
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                directory: metadata.is_dir(),
            },
            path.clone(),
        ));
        if sources.len() + pending.len() > MAX_FILE_ENTRIES {
            bail!("文件和目录总数不能超过 1024");
        }
        if relative.len() > 1024 {
            bail!("目录嵌套过深");
        }
        if metadata.is_dir() {
            let mut children = fs::read_dir(&path).await?;
            while let Some(child) = children.next_entry().await? {
                let name = child
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("文件名不是有效的 Unicode"))?;
                pending.push((child.path(), format!("{relative}/{name}")));
                if sources.len() + pending.len() > MAX_FILE_ENTRIES {
                    bail!("文件和目录总数不能超过 1024");
                }
            }
        }
    }
    sources.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    Ok(sources)
}
