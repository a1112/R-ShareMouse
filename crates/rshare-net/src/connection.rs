//! Connection management

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{timeout, Instant};

use rshare_core::{hello_back_message, hello_message, DeviceId, Message, ScreenInfo};

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

        let event_tx = self.event_tx.clone();
        let pool = self.pool.clone();
        let mut incoming_rx = self.transport.incoming();
        let local_device_id = self.local_device_id;

        tokio::spawn(async move {
            while let Some(incoming) = incoming_rx.recv().await {
                let event_tx = event_tx.clone();
                let pool = pool.clone();

                tokio::spawn(async move {
                    let address = incoming.address;
                    let mut connection = incoming.connection;
                    let Some(mut message_rx) = connection.take_message_channel() else {
                        let _ = event_tx
                            .send(ManagerEvent::Error {
                                device_id: DeviceId::nil(),
                                error: format!(
                                    "Incoming connection from {} has no message channel",
                                    address
                                ),
                            })
                            .await;
                        return;
                    };

                    let first_message = timeout(Duration::from_secs(5), message_rx.recv()).await;
                    let device_id = match first_message {
                        Ok(Some(Message::Hello { device_id, .. }))
                        | Ok(Some(Message::HelloBack { device_id, .. })) => device_id,
                        Ok(Some(other)) => {
                            let _ = event_tx
                                .send(ManagerEvent::Error {
                                    device_id: DeviceId::nil(),
                                    error: format!(
                                        "Incoming connection from {} sent non-handshake message first: {:?}",
                                        address, other
                                    ),
                                })
                                .await;
                            return;
                        }
                        Ok(None) => {
                            let _ = event_tx
                                .send(ManagerEvent::Error {
                                    device_id: DeviceId::nil(),
                                    error: format!(
                                        "Incoming connection from {} closed during handshake",
                                        address
                                    ),
                                })
                                .await;
                            return;
                        }
                        Err(_) => {
                            let _ = event_tx
                                .send(ManagerEvent::Error {
                                    device_id: DeviceId::nil(),
                                    error: format!(
                                        "Incoming connection from {} timed out during handshake",
                                        address
                                    ),
                                })
                                .await;
                            return;
                        }
                    };

                    connection.set_device_id(device_id);
                    let hello_back = hello_back_message(
                        local_device_id,
                        local_device_name(),
                        local_hostname(),
                        ScreenInfo::primary(),
                    );
                    if let Err(err) = connection.send_message(&hello_back).await {
                        let _ = event_tx
                            .send(ManagerEvent::Error {
                                device_id,
                                error: format!("Failed to send handshake response: {}", err),
                            })
                            .await;
                        return;
                    }

                    pool.insert(device_id, connection).await;
                    let _ = event_tx.send(ManagerEvent::Connected(device_id)).await;
                    spawn_message_forwarder(device_id, message_rx, event_tx);
                });
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
                conn.set_device_id(device_id);
                conn.send_message(&hello_message(
                    self.local_device_id,
                    local_device_name(),
                    local_hostname(),
                ))
                .await?;
                let message_rx = conn.take_message_channel();
                self.update_connection_state(device_id, ConnectionState::Connected);
                self.pool.insert(device_id, conn).await;

                let _ = self.event_tx.send(ManagerEvent::Connected(device_id)).await;

                if let Some(message_rx) = message_rx {
                    spawn_message_forwarder(device_id, message_rx, self.event_tx.clone());
                }

                // Note: message handling would need a different design
                // since QuicConnection doesn't support clone

                Ok(())
            }
            Err(e) => {
                self.update_connection_state(device_id, ConnectionState::Error);
                self.connections.remove(&device_id);
                let _ = self.event_tx.send(ManagerEvent::Error {
                    device_id,
                    error: e.to_string(),
                });
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

fn local_hostname() -> String {
    hostname::get()
        .unwrap_or_else(|_| "unknown".into())
        .to_string_lossy()
        .to_string()
}

fn local_device_name() -> String {
    format!("{}-R-ShareMouse", local_hostname())
}

fn spawn_message_forwarder(
    device_id: DeviceId,
    mut message_rx: mpsc::Receiver<Message>,
    event_tx: mpsc::Sender<ManagerEvent>,
) {
    tokio::spawn(async move {
        while let Some(message) = message_rx.recv().await {
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
}
