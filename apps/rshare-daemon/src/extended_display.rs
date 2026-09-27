//! Single daemon-owned extended desktop. WebRTC video never enters this channel.
use crate::{mobile_gateway::MobileHttpRequest, DaemonState};
use anyhow::{anyhow, bail, ensure, Context, Result};
use futures_util::{SinkExt, StreamExt};
use rshare_core::{
    LocalDisplayInfo, VirtualDisplayCreateRequest, VirtualDisplayOperationStatus,
    VirtualDisplayRemoveRequest,
};
use rshare_platform::{
    display::query_display_state,
    remote_touch::{Bounds, Contact, TouchInjector, TouchState},
    virtual_display,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{mpsc as blocking, Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    sync::{mpsc, RwLock},
    time::timeout,
};
use tokio_tungstenite::{
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Role as WsRole, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

pub(crate) const PAGE: &str = include_str!("extended-display/page.html");
pub(crate) const CLIENT: &str = include_str!("extended-display/client.mjs");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Host,
    Receiver,
}
impl Role {
    fn other(self) -> Self {
        if self == Self::Host {
            Self::Receiver
        } else {
            Self::Host
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientMessage {
    Ping,
    Create {
        width: u32,
        height: u32,
    },
    EnableTouch {
        enabled: bool,
    },
    Touch {
        sequence: u64,
        contacts: Vec<Contact>,
    },
    Offer {
        sdp: String,
    },
    Answer {
        sdp: String,
    },
    Ice {
        candidate: Value,
    },
}

fn allow_role(role: Role, peer: SocketAddr) -> bool {
    role != Role::Host || peer.ip().is_loopback()
}

fn validate_message(role: Role, text: &str) -> Result<ClientMessage> {
    ensure!(text.len() <= 65_536, "display message too large");
    let message: ClientMessage = serde_json::from_str(text)?;
    match &message {
        ClientMessage::Ping | ClientMessage::Ice { .. } => {}
        ClientMessage::Create { width, height } => {
            ensure!(
                role == Role::Host && (*width, *height) == (1920, 1080),
                "only host may create a 1080p display"
            );
        }
        ClientMessage::EnableTouch { .. } | ClientMessage::Offer { .. } => {
            ensure!(role == Role::Host, "host-only command")
        }
        ClientMessage::Answer { .. } => ensure!(role == Role::Receiver, "receiver-only command"),
        ClientMessage::Touch { sequence, contacts } => {
            ensure!(role == Role::Receiver, "receiver-only command");
            TouchState::default().prepare(
                *sequence,
                contacts,
                Bounds {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            )?;
        }
    }
    Ok(message)
}

#[derive(Debug)]
struct Slot {
    id: u64,
    events: mpsc::Sender<Value>,
}
#[derive(Debug, Default)]
struct HubState {
    next_id: u64,
    active_workers: usize,
    host: Option<Slot>,
    receiver: Option<Slot>,
    worker: Option<blocking::SyncSender<Work>>,
}
#[derive(Debug, Default, Clone)]
pub(crate) struct DisplayHub(Arc<Mutex<HubState>>);

impl DisplayHub {
    pub(crate) async fn shutdown(&self) -> Result<()> {
        {
            let mut state = self
                .0
                .lock()
                .map_err(|_| anyhow!("display hub unavailable"))?;
            state.host = None;
            state.receiver = None;
            state.worker = None;
        }
        timeout(Duration::from_secs(5), async {
            loop {
                if self
                    .0
                    .lock()
                    .map_err(|_| anyhow!("display hub unavailable"))?
                    .active_workers
                    == 0
                {
                    return Ok::<(), anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("extended display cleanup deadline exceeded")?
    }
    fn attach(&self, role: Role) -> Result<(Registration, mpsc::Receiver<Value>)> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| anyhow!("display hub unavailable"))?;
        ensure!(
            match role {
                Role::Host => state.host.is_none(),
                Role::Receiver => state.receiver.is_none(),
            },
            "display role already connected"
        );
        state.next_id += 1;
        let id = state.next_id;
        let (events, rx) = mpsc::channel(32);
        let slot = Slot { id, events };
        match role {
            Role::Host => state.host = Some(slot),
            Role::Receiver => state.receiver = Some(slot),
        }
        if state.host.is_some() && state.receiver.is_some() {
            let _ = state
                .host
                .as_ref()
                .unwrap()
                .events
                .try_send(json!({"type":"receiver_ready"}));
        }
        Ok((
            Registration {
                hub: self.clone(),
                role,
                id,
            },
            rx,
        ))
    }
    fn send(&self, role: Role, event: Value) -> Result<()> {
        let state = self
            .0
            .lock()
            .map_err(|_| anyhow!("display hub unavailable"))?;
        if let Some(slot) = match role {
            Role::Host => &state.host,
            Role::Receiver => &state.receiver,
        } {
            slot.events
                .try_send(event)
                .context("display event queue congested")?;
        }
        Ok(())
    }
    fn current(&self, role: Role) -> Option<u64> {
        let state = self.0.lock().ok()?;
        match role {
            Role::Host => &state.host,
            Role::Receiver => &state.receiver,
        }
        .as_ref()
        .map(|s| s.id)
    }
    fn work(&self, role: Role, id: u64, message: ClientMessage) -> Result<()> {
        let state = self
            .0
            .lock()
            .map_err(|_| anyhow!("display hub unavailable"))?;
        let worker = state.worker.as_ref().context("host is not connected")?;
        worker
            .try_send(Work { role, id, message })
            .context("display input queue congested")
    }
    fn report(&self, message: String) {
        let event = json!({"type":"error", "message":message});
        let _ = self.send(Role::Host, event.clone());
        let _ = self.send(Role::Receiver, event);
    }
}

struct Registration {
    hub: DisplayHub,
    role: Role,
    id: u64,
}

struct WorkerLifetime(DisplayHub);
impl WorkerLifetime {
    fn new(hub: DisplayHub) -> Self {
        hub.0.lock().expect("display hub").active_workers += 1;
        Self(hub)
    }
}
impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0 .0.lock() {
            state.active_workers = state.active_workers.saturating_sub(1);
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut state) = self.hub.0.lock() {
            let slot = match self.role {
                Role::Host => &mut state.host,
                Role::Receiver => &mut state.receiver,
            };
            if slot.as_ref().is_some_and(|s| s.id == self.id) {
                *slot = None;
            }
            if self.role == Role::Host {
                state.worker = None;
            }
            let other = match self.role {
                Role::Host => &state.receiver,
                Role::Receiver => &state.host,
            };
            if let Some(other) = other {
                let _ = other.events.try_send(json!({"type": if self.role == Role::Host { "host_left" } else { "receiver_left" }}));
            }
        }
    }
}

#[derive(Debug)]
struct Work {
    role: Role,
    id: u64,
    message: ClientMessage,
}

pub(crate) async fn serve(
    mut stream: TcpStream,
    peer: SocketAddr,
    request: &MobileHttpRequest,
    role: Role,
    hub: DisplayHub,
    daemon: Arc<RwLock<DaemonState>>,
) -> Result<()> {
    ensure!(
        allow_role(role, peer),
        "sender page requires a local connection"
    );
    ensure!(
        request
            .header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
            && request.header("connection").is_some_and(|v| v
                .split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("upgrade")))
            && request.header("sec-websocket-version") == Some("13"),
        "invalid WebSocket upgrade"
    );
    let host = request.header("host").context("missing Host")?;
    ensure!(
        request.header("origin") == Some(format!("http://{host}").as_str()),
        "display socket origin mismatch"
    );
    let key = request
        .header("sec-websocket-key")
        .context("missing WebSocket key")?;
    ensure!(key.len() == 24, "invalid WebSocket key");
    let (registration, mut events) = hub.attach(role)?;
    let header = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", derive_accept_key(key.as_bytes()));
    timeout(Duration::from_secs(3), stream.write_all(header.as_bytes())).await??;
    stream.set_nodelay(true)?;
    let config = WebSocketConfig {
        max_message_size: Some(65_536),
        max_frame_size: Some(65_536),
        ..Default::default()
    };
    let mut socket = WebSocketStream::from_raw_socket(stream, WsRole::Server, Some(config)).await;
    if role == Role::Host {
        let (tx, rx) = blocking::sync_channel(32);
        hub.0
            .lock()
            .map_err(|_| anyhow!("display hub unavailable"))?
            .worker = Some(tx);
        let worker_hub = hub.clone();
        let host_id = registration.id;
        let lifetime = WorkerLifetime::new(hub.clone());
        std::thread::Builder::new()
            .name("rshare-display-touch".into())
            .spawn(move || {
                let _lifetime = lifetime;
                run_worker(rx, worker_hub, host_id, daemon);
            })?;
    }
    timeout(
        Duration::from_secs(2),
        socket.send(Message::Text(json!({"type":"hello"}).to_string())),
    )
    .await??;
    let mut last_message = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                ensure!(last_message.elapsed() < Duration::from_secs(4), "display heartbeat expired");
            }
            event = events.recv() => {
                let Some(event) = event else { break };
                let ended = event["type"] == "host_left";
                timeout(Duration::from_secs(2), socket.send(Message::Text(event.to_string()))).await??;
                if ended { break; }
            }
            incoming = socket.next() => {
                let Some(incoming) = incoming else { break };
                let message = incoming?;
                match message {
                    Message::Text(text) => {
                        let result = (|| -> Result<()> {
                            let message = validate_message(role, &text)?;
                            last_message = Instant::now();
                            match &message {
                                ClientMessage::Ping => {},
                                ClientMessage::Offer { sdp } => hub.send(role.other(), json!({"type":"offer","sdp":sdp}))?,
                                ClientMessage::Answer { sdp } => hub.send(role.other(), json!({"type":"answer","sdp":sdp}))?,
                                ClientMessage::Ice { candidate } => hub.send(role.other(), json!({"type":"ice","candidate":candidate}))?,
                                _ => hub.work(role, registration.id, message)?,
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            timeout(Duration::from_secs(2), socket.send(Message::Text(json!({"type":"error","message":error.to_string()}).to_string()))).await??;
                            // Fail closed: no stale touch may survive an invalid/overflowed input.
                            break;
                        }
                    },
                    Message::Close(_) => break,
                    Message::Ping(data) => { timeout(Duration::from_secs(2), socket.send(Message::Pong(data))).await??; },
                    Message::Pong(_) => {},
                    _ => bail!("binary video is not allowed on display control socket"),
                }
            }
        }
    }
    Ok(()) // Registration drop revokes ownership; the dedicated worker performs cleanup.
}

fn bounds(display: &LocalDisplayInfo) -> Bounds {
    Bounds {
        x: display.x,
        y: display.y,
        width: display.width,
        height: display.height,
    }
}

fn refresh_inventory(daemon: &Arc<RwLock<DaemonState>>) {
    if let Ok(displays) = virtual_display::list_virtual_displays() {
        let mut state = daemon.blocking_write();
        state.virtual_displays.sync_platform_displays(displays);
        state.refresh_local_controls_platform();
    }
}

fn run_worker(
    rx: blocking::Receiver<Work>,
    hub: DisplayHub,
    host_id: u64,
    daemon: Arc<RwLock<DaemonState>>,
) {
    let mut lease = None;
    let mut owned = false;
    let mut display: Option<LocalDisplayInfo> = None;
    let mut injector: Option<TouchInjector> = None;
    let mut receiver_id = hub.current(Role::Receiver);
    let mut last_touch = Instant::now();
    let mut last_geometry = Instant::now();
    // The driver has one slot; its canonical ID is fixed by platform enumeration.
    let id = "rshare-vdisplay-1";
    while hub.current(Role::Host) == Some(host_id) {
        let current_receiver = hub.current(Role::Receiver);
        if receiver_id != current_receiver {
            injector = None;
            receiver_id = current_receiver;
            let _ = hub.send(
                Role::Receiver,
                json!({"type":"touch_enabled","enabled":false}),
            );
        }
        if injector.is_some() && last_touch.elapsed() > Duration::from_secs(1) {
            if let Some(touch) = injector.as_mut() {
                if let Err(error) = touch.cancel() {
                    hub.report(error.to_string());
                    injector = None;
                }
            }
        }
        if let Some(active) = &display {
            if last_geometry.elapsed() > Duration::from_millis(500) {
                last_geometry = Instant::now();
                let current = query_display_state().ok().and_then(|s| {
                    s.displays
                        .into_iter()
                        .find(|d| d.display_id == active.display_id && d.active)
                });
                if current.as_ref().map(bounds) != Some(bounds(active)) {
                    injector = None;
                    hub.report("扩展屏位置、分辨率或连接发生变化；请停止后重新创建会话".into());
                    break;
                }
            }
        }
        let work = match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(work) => work,
            Err(blocking::RecvTimeoutError::Timeout) => continue,
            Err(blocking::RecvTimeoutError::Disconnected) => break,
        };
        if hub.current(work.role) != Some(work.id) {
            continue;
        }
        let result = (|| -> Result<()> {
            match work.message {
                ClientMessage::Create { width, height } => {
                    ensure!(!owned, "本会话已经创建了扩展屏");
                    lease = Some(virtual_display::reserve_virtual_display()?);
                    ensure!(
                        virtual_display::list_virtual_displays()?.is_empty(),
                        "已有虚拟显示器，请先在显示设置中处理；不会覆盖现有屏幕"
                    );
                    let result =
                        virtual_display::create_virtual_display(&VirtualDisplayCreateRequest {
                            id: Some(id.into()),
                            width,
                            height,
                            refresh_rate_millihz: Some(60_000),
                            name: Some("R-ShareMouse Extended Display".into()),
                        })?;
                    ensure!(
                        result.status == VirtualDisplayOperationStatus::Created,
                        "{}",
                        result
                            .message
                            .unwrap_or_else(|| format!("虚拟显示器创建失败：{:?}", result.status))
                    );
                    owned = true;
                    // IDD hotplug may finish asynchronously. Never announce Ready from a request alone.
                    for _ in 0..40 {
                        if hub.current(Role::Host) != Some(host_id) {
                            bail!("主机已断开");
                        }
                        let virtuals = virtual_display::list_virtual_displays()?;
                        if let Some(os_id) = virtuals.iter().find_map(|v| v.display_id.as_ref()) {
                            display = query_display_state()?
                                .displays
                                .into_iter()
                                .find(|d| &d.display_id == os_id && d.active);
                            if display.is_some() {
                                break;
                            }
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    let active = display
                        .as_ref()
                        .context("驱动已接受创建，但系统尚未识别扩展屏；已结束会话以清理")?;
                    hub.send(Role::Host, json!({"type":"display", "display":active}))?;
                    refresh_inventory(&daemon);
                }
                ClientMessage::EnableTouch { enabled } => {
                    ensure!(display.is_some(), "扩展屏未就绪");
                    ensure!(!enabled || receiver_id.is_some(), "请先连接接收端");
                    injector = if enabled {
                        Some(TouchInjector::new()?)
                    } else {
                        None
                    };
                    last_touch = Instant::now();
                    let event = json!({"type":"touch_enabled","enabled":enabled});
                    hub.send(Role::Receiver, event.clone())?;
                    hub.send(Role::Host, event)?;
                }
                ClientMessage::Touch { sequence, contacts } => {
                    let touch = injector.as_mut().context("触摸未被主机启用")?;
                    let active = display.as_ref().context("扩展屏不可用")?;
                    touch.inject(sequence, &contacts, bounds(active))?;
                    last_touch = Instant::now();
                }
                _ => bail!("invalid display worker command"),
            }
            Ok(())
        })();
        if let Err(error) = result {
            injector = None;
            hub.report(error.to_string());
            if owned && display.is_none() {
                break;
            }
            if !owned {
                lease = None;
            }
            let _ = hub.send(
                Role::Receiver,
                json!({"type":"touch_enabled","enabled":false}),
            );
            let _ = hub.send(Role::Host, json!({"type":"touch_enabled","enabled":false}));
        }
    }
    drop(injector);
    if owned {
        match virtual_display::remove_virtual_display(&VirtualDisplayRemoveRequest {
            id: id.into(),
        }) {
            Ok(result) if result.status == VirtualDisplayOperationStatus::Removed => {}
            other => {
                hub.report(format!("扩展屏自动移除失败，请在显示设置中检查：{other:?}"));
                tracing::warn!("Extended display cleanup failed: {other:?}");
            }
        }
        refresh_inventory(&daemon);
    }
    drop(lease);
    if hub.current(Role::Host) == Some(host_id) {
        let _ = hub.send(Role::Host, json!({"type":"session_ended"}));
        let _ = hub.send(Role::Receiver, json!({"type":"host_left"}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_display_roles_are_separate_and_hosts_require_loopback() {
        assert!(validate_message(
            Role::Receiver,
            r#"{"type":"create","width":1920,"height":1080}"#
        )
        .is_err());
        assert!(
            validate_message(Role::Host, r#"{"type":"touch","sequence":1,"contacts":[]}"#).is_err()
        );
        assert!(validate_message(Role::Host, r#"{"type":"answer","sdp":"x"}"#).is_err());
        assert!(validate_message(Role::Receiver, r#"{"type":"answer","sdp":"x"}"#).is_ok());
        assert!(allow_role(Role::Host, "127.0.0.1:42".parse().unwrap()));
        assert!(!allow_role(Role::Host, "192.168.1.42:42".parse().unwrap()));
    }

    #[test]
    fn extended_display_limits_signal_and_touch_payloads() {
        assert!(validate_message(
            Role::Host,
            &format!(r#"{{"type":"offer","sdp":"{}"}}"#, "x".repeat(65_536))
        )
        .is_err());
        assert!(validate_message(
            Role::Receiver,
            r#"{"type":"touch","sequence":0,"contacts":[]}"#
        )
        .is_err());
        assert!(validate_message(
            Role::Receiver,
            r#"{"type":"touch","sequence":1,"contacts":[{"id":1,"x":2,"y":0,"pressure":1}]}"#
        )
        .is_err());
        assert!(validate_message(
            Role::Host,
            r#"{"type":"create","width":80000,"height":1080}"#
        )
        .is_err());
    }

    #[test]
    fn extended_display_does_not_replace_existing_clients_and_bounds_queue() {
        let hub = DisplayHub::default();
        let (_host, mut host_events) = hub.attach(Role::Host).unwrap();
        assert!(hub.attach(Role::Host).is_err());
        let (_receiver, _receiver_events) = hub.attach(Role::Receiver).unwrap();
        assert!(hub.attach(Role::Receiver).is_err());
        assert_eq!(host_events.try_recv().unwrap()["type"], "receiver_ready");
        for _ in 0..32 {
            hub.send(Role::Host, serde_json::json!({"type":"ping"}))
                .unwrap();
        }
        assert!(hub
            .send(Role::Host, serde_json::json!({"type":"ping"}))
            .is_err());
    }

    #[test]
    fn extended_display_disconnect_revokes_generation_and_notifies_peer() {
        let hub = DisplayHub::default();
        let (host, mut host_events) = hub.attach(Role::Host).unwrap();
        let (receiver, mut receiver_events) = hub.attach(Role::Receiver).unwrap();
        let old_id = receiver.id;
        host_events.try_recv().unwrap();
        drop(receiver);
        assert_eq!(hub.current(Role::Receiver), None);
        assert_eq!(host_events.try_recv().unwrap()["type"], "receiver_left");
        let (_replacement, _) = hub.attach(Role::Receiver).unwrap();
        assert_ne!(hub.current(Role::Receiver), Some(old_id));
        // Old connection cannot consume a new generation's events.
        assert!(receiver_events.try_recv().is_err());
        drop(host);
        assert!(hub
            .work(Role::Receiver, old_id, ClientMessage::Ping)
            .is_err());
    }

    #[tokio::test]
    async fn extended_display_shutdown_waits_for_platform_cleanup() {
        let hub = DisplayHub::default();
        let lifetime = WorkerLifetime::new(hub.clone());
        let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cleaned = cleaned.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            worker_cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(lifetime);
        });
        hub.shutdown().await.unwrap();
        assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
    }
}
