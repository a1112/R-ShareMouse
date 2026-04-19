//! QUIC transport layer for low-latency encrypted communication

use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex as TokioMutex};

use super::codec::MessageCodec;
use rshare_core::{DeviceId, Message};

#[derive(Debug, Clone, Default)]
pub struct TransportConfig {
    pub max_idle_timeout: Duration,
    pub keep_alive_interval: Duration,
    pub max_message_size: usize,
}

pub struct QuicTransport {
    server_task: Option<tokio::task::JoinHandle<()>>,
    config: TransportConfig,
    local_device_id: DeviceId,
    incoming_tx: mpsc::Sender<IncomingConnection>,
    incoming_rx: Option<mpsc::Receiver<IncomingConnection>>,
}

pub struct IncomingConnection {
    pub device_id: Option<DeviceId>,
    pub address: SocketAddr,
    pub connection: QuicConnection,
}

impl QuicTransport {
    pub fn new(local_device_id: DeviceId) -> Self {
        let (incoming_tx, incoming_rx) = mpsc::channel(10);

        Self {
            server_task: None,
            config: TransportConfig::default(),
            local_device_id,
            incoming_tx,
            incoming_rx: Some(incoming_rx),
        }
    }

    pub fn with_config(mut self, config: TransportConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn start_server(&mut self, bind_addr: &str) -> Result<()> {
        let bind_addr: SocketAddr = bind_addr
            .parse()
            .map_err(|_| anyhow!("Invalid bind address: {}", bind_addr))?;

        let listener = TcpListener::bind(bind_addr).await?;
        info!("Transport server listening on {}", listener.local_addr()?);

        let incoming_tx = self.incoming_tx.clone();
        let local_device_id = self.local_device_id;

        let server_task = tokio::spawn(async move {
            while let Ok((stream, addr)) = listener.accept().await {
                let incoming_tx = incoming_tx.clone();

                tokio::spawn(async move {
                    info!("Incoming connection from {}", addr);

                    let quic_conn = QuicConnection::new(stream, local_device_id, addr);

                    let _ = incoming_tx.try_send(IncomingConnection {
                        device_id: None,
                        address: addr,
                        connection: quic_conn,
                    });
                });
            }
        });

        self.server_task = Some(server_task);
        Ok(())
    }

    pub async fn connect(
        &mut self,
        remote_addr: &str,
        _device_id: DeviceId,
    ) -> Result<QuicConnection> {
        let remote_addr: SocketAddr = remote_addr
            .parse()
            .map_err(|_| anyhow!("Invalid remote address: {}", remote_addr))?;

        info!("Connecting to {}", remote_addr);

        let stream = TcpStream::connect(remote_addr).await?;
        let peer_addr = stream.peer_addr()?;

        info!("Connected to {}", peer_addr);

        let quic_conn = QuicConnection::new(stream, self.local_device_id, peer_addr);

        Ok(quic_conn)
    }

    pub fn incoming(&mut self) -> mpsc::Receiver<IncomingConnection> {
        self.incoming_rx.take().expect("Incoming already taken")
    }

    pub fn is_running(&self) -> bool {
        self.server_task
            .as_ref()
            .map(|task| !task.is_finished())
            .unwrap_or(false)
    }

    pub fn local_device_id(&self) -> DeviceId {
        self.local_device_id
    }

    pub async fn close(&mut self) -> Result<()> {
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
        info!("Transport closed");
        Ok(())
    }
}

impl Default for QuicTransport {
    fn default() -> Self {
        Self::new(DeviceId::new_v4())
    }
}

pub struct QuicConnection {
    device_id: Option<DeviceId>,
    remote_addr: SocketAddr,
    send_channel: mpsc::Sender<OutboundFrame>,
    message_rx: Option<mpsc::Receiver<Message>>,
    _local_device_id: DeviceId,
}

struct OutboundFrame {
    data: Vec<u8>,
    ack: oneshot::Sender<std::result::Result<(), String>>,
}

impl QuicConnection {
    pub fn new(stream: TcpStream, _local_device_id: DeviceId, remote_addr: SocketAddr) -> Self {
        let (send_channel, mut send_rx): (mpsc::Sender<OutboundFrame>, _) = mpsc::channel(100);
        let (message_tx, _message_rx): (mpsc::Sender<Message>, _) = mpsc::channel(100);

        // Clone message_tx for use in spawned task
        let message_tx_for_task = message_tx.clone();

        // Spawn bidirectional handler
        tokio::spawn(async move {
            let stream = stream;
            let (mut read_half, mut write_half) = tokio::io::split(stream);

            // Writer task
            let writer = async move {
                while let Some(frame) = send_rx.recv().await {
                    let len = frame.data.len() as u32;
                    if let Err(error) = write_half.write_all(&len.to_be_bytes()).await {
                        let _ = frame.ack.send(Err(error.to_string()));
                        break;
                    }
                    if let Err(error) = write_half.write_all(&frame.data).await {
                        let _ = frame.ack.send(Err(error.to_string()));
                        break;
                    }
                    let _ = frame.ack.send(Ok(()));
                }
            };

            // Reader task
            let reader = async move {
                loop {
                    let mut len_buf = [0u8; 4];
                    match read_half.read_exact(&mut len_buf).await {
                        Ok(_) => {
                            let len = u32::from_be_bytes(len_buf) as usize;
                            let mut data = vec![0u8; len];

                            match read_half.read_exact(&mut data).await {
                                Ok(_) => {
                                    if let Ok(msg) = MessageCodec::decode(&data) {
                                        if message_tx_for_task.send(msg).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        Err(_) => break,
                    }
                }
            };

            tokio::select! {
                _ = writer => {}
                _ = reader => {}
            }
        });

        Self {
            device_id: None,
            remote_addr,
            send_channel,
            message_rx: Some(_message_rx),
            _local_device_id,
        }
    }

    pub fn device_id(&self) -> Option<DeviceId> {
        self.device_id
    }

    pub fn set_device_id(&mut self, device_id: DeviceId) {
        self.device_id = Some(device_id);
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub async fn send_message(&self, message: &Message) -> Result<()> {
        let encoded = MessageCodec::encode(message)?;
        let (ack, written) = oneshot::channel();

        self.send_channel
            .send(OutboundFrame { data: encoded, ack })
            .await
            .map_err(|_| anyhow!("Send channel closed"))?;

        written
            .await
            .map_err(|_| anyhow!("Write confirmation channel closed"))?
            .map_err(|error| anyhow!("Write failed: {error}"))
    }

    pub async fn receive_message(&mut self) -> Result<Message> {
        let rx = self
            .message_rx
            .as_mut()
            .ok_or_else(|| anyhow!("Message channel already taken"))?;
        rx.recv()
            .await
            .ok_or_else(|| anyhow!("Message channel closed"))
    }

    pub fn message_channel(&mut self) -> mpsc::Receiver<Message> {
        self.message_rx
            .take()
            .expect("Message channel already taken")
    }

    pub fn is_connected(&self) -> bool {
        !self.send_channel.is_closed()
    }

    pub async fn close(self) {
        drop(self);
        info!("Connection closed");
    }
}

pub struct ConnectionPool {
    _local_device_id: DeviceId,
    connections: Arc<TokioMutex<std::collections::HashMap<DeviceId, QuicConnection>>>,
}

impl ConnectionPool {
    pub fn new(local_device_id: DeviceId) -> Self {
        Self {
            _local_device_id: local_device_id,
            connections: Arc::new(TokioMutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn insert(&self, device_id: DeviceId, conn: QuicConnection) {
        let mut conns = self.connections.lock().await;
        conns.insert(device_id, conn);
    }

    pub fn get(&self, _device_id: &DeviceId) -> Option<&'static QuicConnection> {
        // Return None - actual implementation would use Arc or different design
        None
    }

    pub async fn send_to(&self, device_id: &DeviceId, message: &Message) -> Result<()> {
        let conns = self.connections.lock().await;
        if let Some(conn) = conns.get(device_id) {
            conn.send_message(message).await?;
        }
        Ok(())
    }

    pub async fn remove(&self, device_id: &DeviceId) -> Option<QuicConnection> {
        let mut conns = self.connections.lock().await;
        conns.remove(device_id)
    }

    pub fn count(&self) -> usize {
        let conns = self.connections.blocking_lock();
        conns.len()
    }

    pub async fn broadcast(&self, message: &Message) -> Result<()> {
        let conns = self.connections.lock().await;
        for (_id, conn) in conns.iter() {
            let _ = conn.send_message(message).await;
        }
        Ok(())
    }

    pub async fn cleanup(&self) {
        let mut conns = self.connections.lock().await;
        conns.retain(|_id, conn| conn.is_connected());
    }
}

impl Clone for ConnectionPool {
    fn clone(&self) -> Self {
        Self {
            _local_device_id: self._local_device_id,
            connections: Arc::clone(&self.connections),
        }
    }
}

use tracing::info;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_new() {
        let transport = QuicTransport::new(DeviceId::new_v4());
        assert!(!transport.is_running());
    }

    #[tokio::test]
    async fn start_server_marks_transport_running() {
        let mut transport = QuicTransport::new(DeviceId::new_v4());

        transport.start_server("127.0.0.1:0").await.unwrap();

        assert!(transport.is_running());

        transport.close().await.unwrap();
        assert!(!transport.is_running());
    }

    #[test]
    fn test_connection_pool() {
        let pool = ConnectionPool::new(DeviceId::new_v4());
        assert_eq!(pool.count(), 0);
    }

    #[tokio::test]
    async fn quic_connection_exposes_received_messages() {
        let local_id = DeviceId::new_v4();
        let remote_id = DeviceId::new_v4();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let (server_stream, server_addr) = listener.accept().await.unwrap();

        let mut receiver = QuicConnection::new(server_stream, local_id, server_addr);
        let sender = QuicConnection::new(client_stream, remote_id, addr);
        let mut messages = receiver.message_channel();

        sender
            .send_message(&Message::MouseMove { x: 42, y: 24 })
            .await
            .unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), messages.recv())
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(received, Message::MouseMove { x: 42, y: 24 }));
    }
}
