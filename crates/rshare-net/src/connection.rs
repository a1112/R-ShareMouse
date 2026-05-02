//! Connection management

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Instant;

use rshare_core::{
    hello_back_message, hello_message, protocol::PROTOCOL_VERSION, DeviceId, Message, ScreenInfo,
};

use super::transport::{ConnectionPool, QuicTransport};

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

/// Connection information
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub device_id: DeviceId,
    pub address: String,
    pub state: ConnectionState,
    pub last_activity: Instant,
    pub messages_sent: u64,
    pub messages_received: u64,
}

impl ConnectionInfo {
    pub fn new(device_id: DeviceId, address: String) -> Self {
        Self {
            device_id,
            address,
            state: ConnectionState::Connecting,
            last_activity: Instant::now(),
            messages_sent: 0,
            messages_received: 0,
        }
    }

    pub fn is_stale(&self, timeout: Duration) -> bool {
        self.state != ConnectionState::Connected && self.last_activity.elapsed() > timeout
    }
}

/// Connection manager event
#[derive(Debug, Clone)]
pub enum ManagerEvent {
    Connected(DeviceId),
    Disconnected(DeviceId),
    MessageReceived { from: DeviceId, message: Message },
    Error { device_id: DeviceId, error: String },
}

/// Connection manager for handling multiple device connections
pub struct ConnectionManager {
    local_device_id: DeviceId,
    connections: HashMap<DeviceId, ConnectionInfo>,
    transport: QuicTransport,
    pool: Arc<ConnectionPool>,
    event_tx: mpsc::Sender<ManagerEvent>,
    event_rx: Option<mpsc::Receiver<ManagerEvent>>,
}

fn spawn_message_reader(
    device_id: DeviceId,
    mut messages: mpsc::Receiver<Message>,
    first_message: Option<Message>,
    event_tx: mpsc::Sender<ManagerEvent>,
) {
    tokio::spawn(async move {
        if let Some(message) = first_message {
            if event_tx
                .send(ManagerEvent::MessageReceived {
                    from: device_id,
                    message,
                })
                .await
                .is_err()
            {
                return;
            }
        }

        while let Some(message) = messages.recv().await {
            if event_tx
                .send(ManagerEvent::MessageReceived {
                    from: device_id,
                    message,
                })
                .await
                .is_err()
            {
                break;
            }
        }

        let _ = event_tx.send(ManagerEvent::Disconnected(device_id)).await;
    });
}

impl ConnectionManager {
    pub fn new(local_device_id: DeviceId) -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);
        let transport = QuicTransport::new(local_device_id);
        let pool = Arc::new(ConnectionPool::new(local_device_id));

        Self {
            local_device_id,
            connections: HashMap::new(),
            transport,
            pool,
            event_tx,
            event_rx: Some(event_rx),
        }
    }

    pub async fn start_server(&mut self, bind_addr: &str) -> Result<()> {
        self.transport.start_server(bind_addr).await?;

        let mut incoming = self.transport.incoming();
        let event_tx = self.event_tx.clone();
        let pool = self.pool.clone();
        let local_device_id = self.local_device_id;

        tokio::spawn(async move {
            while let Some(mut incoming) = incoming.recv().await {
                let (device_id, first_message) =
                    match receive_incoming_handshake(&mut incoming.connection, local_device_id)
                        .await
                    {
                        Ok((device_id, first_message)) => (device_id, first_message),
                        Err(error) => {
                            let device_id = incoming.device_id.unwrap_or_else(DeviceId::new_v4);
                            let _ = event_tx
                                .send(ManagerEvent::Error {
                                    device_id,
                                    error: error.to_string(),
                                })
                                .await;
                            (device_id, None)
                        }
                    };
                incoming.connection.set_device_id(device_id);
                let messages = incoming.connection.message_channel();

                spawn_message_reader(device_id, messages, first_message, event_tx.clone());
                pool.insert(device_id, incoming.connection).await;

                if event_tx
                    .send(ManagerEvent::Connected(device_id))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        Ok(())
    }

    pub async fn connect(&mut self, device_id: DeviceId, address: &str) -> Result<()> {
        if self.connections.contains_key(&device_id) {
            anyhow::bail!("Already connected to device {}", device_id);
        }

        let info = ConnectionInfo::new(device_id, address.to_string());
        self.connections.insert(device_id, info);

        match self.transport.connect(address, device_id).await {
            Ok(mut conn) => {
                let first_message =
                    match perform_outbound_handshake(&mut conn, self.local_device_id).await {
                        OutboundHandshake::HelloBack {
                            device_id: remote_id,
                        } => {
                            if remote_id != device_id {
                                tracing::warn!(
                                    "Connected device id mismatch: expected {}, got {}",
                                    device_id,
                                    remote_id
                                );
                            }
                            None
                        }
                        OutboundHandshake::Prefetched(message) => Some(message),
                        OutboundHandshake::Unavailable(error) => {
                            tracing::debug!("Outbound handshake unavailable: {}", error);
                            None
                        }
                    };

                self.update_connection_state(device_id, ConnectionState::Connected);
                conn.set_device_id(device_id);
                let messages = conn.message_channel();
                spawn_message_reader(device_id, messages, first_message, self.event_tx.clone());

                self.pool.insert(device_id, conn).await;

                let _ = self.event_tx.send(ManagerEvent::Connected(device_id)).await;

                Ok(())
            }
            Err(e) => {
                self.update_connection_state(device_id, ConnectionState::Error);
                self.connections.remove(&device_id);
                let _ = self
                    .event_tx
                    .send(ManagerEvent::Error {
                        device_id,
                        error: e.to_string(),
                    })
                    .await;
                Err(e)
            }
        }
    }

    pub async fn disconnect(&mut self, device_id: &DeviceId) -> Result<()> {
        if self.connections.remove(device_id).is_some() {
            self.pool.remove(device_id).await;
            let _ = self
                .event_tx
                .send(ManagerEvent::Disconnected(*device_id))
                .await;
        }
        Ok(())
    }

    pub async fn send_to(&mut self, device_id: &DeviceId, message: Message) -> Result<()> {
        self.pool.send_to(device_id, &message).await?;

        if let Some(info) = self.connections.get_mut(device_id) {
            info.messages_sent += 1;
            info.last_activity = Instant::now();
        }

        Ok(())
    }

    pub async fn broadcast(&mut self, message: Message) -> Result<()> {
        self.pool.broadcast(&message).await
    }

    pub fn events(&mut self) -> Option<mpsc::Receiver<ManagerEvent>> {
        self.event_rx.take()
    }

    pub fn get_connection(&self, device_id: &DeviceId) -> Option<&ConnectionInfo> {
        self.connections.get(device_id)
    }

    pub fn connections(&self) -> Vec<ConnectionInfo> {
        self.connections.values().cloned().collect()
    }

    pub fn is_connected(&self, device_id: &DeviceId) -> bool {
        self.connections
            .get(device_id)
            .map(|info| info.state == ConnectionState::Connected)
            .unwrap_or(false)
    }

    pub fn connected_count(&self) -> usize {
        self.connections
            .values()
            .filter(|info| info.state == ConnectionState::Connected)
            .count()
    }

    fn update_connection_state(&mut self, device_id: DeviceId, state: ConnectionState) {
        if let Some(info) = self.connections.get_mut(&device_id) {
            info.state = state;
            info.last_activity = Instant::now();
        }
    }

    pub async fn cleanup_stale(&mut self, timeout: Duration) -> Vec<DeviceId> {
        let stale: Vec<DeviceId> = self
            .connections
            .iter()
            .filter(|(_, info)| info.is_stale(timeout))
            .map(|(id, _)| *id)
            .collect();

        for id in &stale {
            let _ = self.disconnect(id).await;
        }

        self.pool.cleanup().await;
        stale
    }

    pub fn pool(&self) -> &Arc<ConnectionPool> {
        &self.pool
    }
}

enum OutboundHandshake {
    HelloBack { device_id: DeviceId },
    Prefetched(Message),
    Unavailable(anyhow::Error),
}

async fn receive_incoming_handshake(
    conn: &mut super::transport::QuicConnection,
    local_device_id: DeviceId,
) -> Result<(DeviceId, Option<Message>)> {
    match tokio::time::timeout(Duration::from_millis(250), conn.receive_message()).await {
        Ok(Ok(Message::Hello {
            app_id,
            device_id,
            device_name: _,
            hostname: _,
            protocol_version,
            ..
        })) if protocol_version == PROTOCOL_VERSION
            && app_id.eq_ignore_ascii_case(rshare_core::DISCOVERY_APP_ID) =>
        {
            conn.send_message(&hello_back_message(
                local_device_id,
                "R-ShareMouse".to_string(),
                hostname::get()
                    .unwrap_or_else(|_| "unknown".into())
                    .to_string_lossy()
                    .to_string(),
                ScreenInfo::primary(),
            ))
            .await?;
            Ok((device_id, None))
        }
        Ok(Ok(message)) => Ok((DeviceId::new_v4(), Some(message))),
        Ok(Err(error)) => Err(error),
        Err(_) => Ok((DeviceId::new_v4(), None)),
    }
}

async fn perform_outbound_handshake(
    conn: &mut super::transport::QuicConnection,
    local_device_id: DeviceId,
) -> OutboundHandshake {
    if let Err(error) = conn
        .send_message(&hello_message(
            local_device_id,
            "R-ShareMouse".to_string(),
            hostname::get()
                .unwrap_or_else(|_| "unknown".into())
                .to_string_lossy()
                .to_string(),
        ))
        .await
    {
        return OutboundHandshake::Unavailable(error);
    }

    match tokio::time::timeout(Duration::from_millis(250), conn.receive_message()).await {
        Ok(Ok(Message::HelloBack {
            app_id,
            device_id,
            protocol_version,
            ..
        })) if protocol_version == PROTOCOL_VERSION
            && app_id.eq_ignore_ascii_case(rshare_core::DISCOVERY_APP_ID) =>
        {
            OutboundHandshake::HelloBack { device_id }
        }
        Ok(Ok(message)) => OutboundHandshake::Prefetched(message),
        Ok(Err(error)) => OutboundHandshake::Unavailable(error),
        Err(_) => OutboundHandshake::Unavailable(anyhow::anyhow!("Handshake timed out")),
    }
}

pub type SharedConnectionManager = Arc<RwLock<ConnectionManager>>;

pub fn create_shared_manager(local_device_id: DeviceId) -> SharedConnectionManager {
    Arc::new(RwLock::new(ConnectionManager::new(local_device_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_info() {
        let info = ConnectionInfo::new(DeviceId::new_v4(), "192.168.1.100:27431".to_string());
        assert_eq!(info.state, ConnectionState::Connecting);
    }

    #[tokio::test]
    async fn test_manager_new() {
        let manager = ConnectionManager::new(DeviceId::new_v4());
        assert_eq!(manager.connected_count(), 0);
    }

    #[tokio::test]
    async fn message_reader_emits_disconnected_when_channel_closes() {
        let device_id = DeviceId::new_v4();
        let (_message_tx, message_rx) = mpsc::channel(1);
        let (event_tx, mut event_rx) = mpsc::channel(1);

        spawn_message_reader(device_id, message_rx, None, event_tx);
        drop(_message_tx);

        let event = tokio::time::timeout(Duration::from_secs(1), event_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(event, ManagerEvent::Disconnected(id) if id == device_id));
    }

    #[tokio::test]
    async fn outbound_connect_failure_emits_error_event() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();

        assert!(manager
            .connect(remote_id, &address.to_string())
            .await
            .is_err());

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(
            event,
            ManagerEvent::Error {
                device_id,
                error: _
            } if device_id == remote_id
        ));
    }

    #[tokio::test]
    async fn explicit_disconnect_emits_disconnected_event() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let mut manager = ConnectionManager::new(local_id);
        manager.connections.insert(
            remote_id,
            ConnectionInfo {
                device_id: remote_id,
                address: "127.0.0.1:27431".to_string(),
                state: ConnectionState::Connected,
                last_activity: Instant::now(),
                messages_sent: 0,
                messages_received: 0,
            },
        );
        let mut events = manager.events().unwrap();

        manager.disconnect(&remote_id).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(event, ManagerEvent::Disconnected(id) if id == remote_id));
    }

    #[tokio::test]
    async fn manager_emits_message_received_for_connected_device() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            let sender = crate::transport::QuicConnection::new(stream, remote_id, peer_addr);
            sender
                .send_message(&Message::MouseMove { x: 7, y: 9 })
                .await
                .unwrap();
        });

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::MessageReceived { from, message } => {
                        break (from, message);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(received.0, remote_id);
        assert!(matches!(received.1, Message::MouseMove { x: 7, y: 9 }));
    }

    #[tokio::test]
    async fn manager_emits_message_received_for_incoming_connection() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server(&address.to_string()).await.unwrap();

        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let sender = crate::transport::QuicConnection::new(stream, remote_id, address);
        sender
            .send_message(&Message::MouseMove { x: 11, y: 13 })
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.unwrap() {
                    ManagerEvent::MessageReceived { from, message } => {
                        break (from, message);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();

        assert!(matches!(received.1, Message::MouseMove { x: 11, y: 13 }));
    }

    #[tokio::test]
    async fn incoming_hello_binds_connection_to_remote_device_id() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = probe.local_addr().unwrap();
        drop(probe);

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager.start_server(&address.to_string()).await.unwrap();

        let stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let sender = crate::transport::QuicConnection::new(stream, remote_id, address);
        sender
            .send_message(&hello_message(
                remote_id,
                "remote".to_string(),
                "remote-host".to_string(),
            ))
            .await
            .unwrap();

        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(device_id) = events.recv().await.unwrap() {
                    break device_id;
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(connected, remote_id);
    }

    #[tokio::test]
    async fn outbound_connect_accepts_hello_back_identity() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            let mut conn = crate::transport::QuicConnection::new(stream, remote_id, peer_addr);
            let hello = conn.receive_message().await.unwrap();
            assert!(matches!(hello, Message::Hello { device_id, .. } if device_id == local_id));
            conn.send_message(&hello_back_message(
                remote_id,
                "remote".to_string(),
                "remote-host".to_string(),
                ScreenInfo::primary(),
            ))
            .await
            .unwrap();
        });

        let mut manager = ConnectionManager::new(local_id);
        let mut events = manager.events().unwrap();
        manager
            .connect(remote_id, &address.to_string())
            .await
            .unwrap();

        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let ManagerEvent::Connected(device_id) = events.recv().await.unwrap() {
                    break device_id;
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(connected, remote_id);
        assert!(manager.is_connected(&remote_id));
    }
}
