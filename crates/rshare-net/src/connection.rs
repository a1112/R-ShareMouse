//! Connection management

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Instant;

use rshare_core::{DeviceId, Message};

use super::transport::{QuicTransport, ConnectionPool};

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
        let _pool = self.pool.clone();

        tokio::spawn(async move {
            // Handle incoming connections here
            // For now, this is a placeholder
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
            Ok(conn) => {
                self.update_connection_state(device_id, ConnectionState::Connected);
                self.pool.insert(device_id, conn);

                let _ = self.event_tx.send(ManagerEvent::Connected(device_id));

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
            self.pool.remove(device_id);
            let _ = self.event_tx.send(ManagerEvent::Disconnected(*device_id));
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

        self.pool.cleanup();
        stale
    }

    pub fn pool(&self) -> &Arc<ConnectionPool> {
        &self.pool
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
        let info = ConnectionInfo::new(
            DeviceId::new_v4(),
            "192.168.1.100:27431".to_string(),
        );
        assert_eq!(info.state, ConnectionState::Connecting);
    }

    #[tokio::test]
    async fn test_manager_new() {
        let manager = ConnectionManager::new(DeviceId::new_v4());
        assert_eq!(manager.connected_count(), 0);
    }
}
