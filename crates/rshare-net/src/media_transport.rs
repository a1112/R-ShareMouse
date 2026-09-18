//! Isolated, mutually authenticated audio QUIC transport. Not advertised until
//! the daemon has installed a working audio backend and control binding.
use crate::encryption::{PeerCertificateFingerprint, QuicIdentity};
use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use quinn::{Connection, Endpoint};
use rshare_core::{
    network_audio::{MediaControl, MAX_CONTROL_BYTES, MEDIA_VERSION},
    DeviceId,
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    DigitallySignedStruct, DistinguishedName, SignatureScheme,
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
const ALPN: &[u8] = b"rshare-audio/1";
#[derive(Debug, Clone)]
pub struct Binding {
    pub peer: DeviceId,
    pub fingerprint: PeerCertificateFingerprint,
    pub token: DeviceId,
    pub generation: u64,
}
#[derive(Debug, Default)]
struct State {
    bindings: HashMap<DeviceId, Binding>,
    active: HashMap<DeviceId, Connection>,
}
#[derive(Debug, Clone, Default)]
pub struct Bindings(Arc<Mutex<State>>);
impl Bindings {
    /// Call only after authenticated control connection establishment.
    pub fn authorize(&self, binding: Binding) -> Result<()> {
        if binding.generation == 0 {
            bail!("zero media generation")
        }
        let mut state = self
            .0
            .lock()
            .map_err(|_| anyhow!("media binding lock poisoned"))?;
        if state.bindings.len() >= 32 && !state.bindings.contains_key(&binding.peer) {
            bail!("audio peer limit exceeded")
        }
        if let Some(conn) = state.active.remove(&binding.peer) {
            conn.close(0u32.into(), b"control generation changed");
        }
        state.bindings.insert(binding.peer, binding);
        Ok(())
    }
    /// Revocation synchronously invalidates the token and closes the media path.
    pub fn revoke(&self, peer: DeviceId) {
        if let Ok(mut s) = self.0.lock() {
            s.bindings.remove(&peer);
            if let Some(c) = s.active.remove(&peer) {
                c.close(0u32.into(), b"control revoked");
            }
        }
    }
    fn accepts(&self, cert: &[u8]) -> bool {
        let pin = PeerCertificateFingerprint::from_der(cert);
        self.0
            .lock()
            .is_ok_and(|s| s.bindings.values().any(|b| b.fingerprint == pin))
    }
    fn bind(
        &self,
        conn: &Connection,
        peer: DeviceId,
        token: DeviceId,
        generation: u64,
    ) -> Result<()> {
        let identity = conn.peer_identity().context("missing client certificate")?;
        let certs = identity
            .downcast::<Vec<CertificateDer<'static>>>()
            .map_err(|_| anyhow!("unexpected certificate identity"))?;
        let pin = PeerCertificateFingerprint::from_der(
            certs.first().context("empty certificate chain")?.as_ref(),
        );
        let mut s = self
            .0
            .lock()
            .map_err(|_| anyhow!("media binding lock poisoned"))?;
        let binding = s
            .bindings
            .get(&peer)
            .context("no authenticated control binding")?;
        if binding.fingerprint != pin || binding.token != token || binding.generation != generation
        {
            bail!("media control binding mismatch")
        }
        if let Some(previous) = s.active.insert(peer, conn.clone()) {
            previous.close(0u32.into(), b"media replaced");
        }
        Ok(())
    }
}
#[derive(Debug)]
struct Verifier {
    provider: Arc<rustls::crypto::CryptoProvider>,
    bindings: Bindings,
    pin: Option<PeerCertificateFingerprint>,
}
impl Verifier {
    fn check(&self, cert: &CertificateDer<'_>) -> std::result::Result<(), rustls::Error> {
        let accepted = self.pin.as_ref().map_or_else(
            || self.bindings.accepts(cert.as_ref()),
            |pin| *pin == PeerCertificateFingerprint::from_der(cert.as_ref()),
        );
        if accepted {
            Ok(())
        } else {
            Err(rustls::Error::General(
                "unapproved audio peer certificate".into(),
            ))
        }
    }
    fn signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
        tls13: bool,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        if tls13 {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &self.provider.signature_verification_algorithms,
            )
        } else {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &self.provider.signature_verification_algorithms,
            )
        }
    }
}
impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.check(cert)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(m, c, d, false)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(m, c, d, true)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
impl ClientCertVerifier for Verifier {
    fn client_auth_mandatory(&self) -> bool {
        true
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.check(cert)?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(m, c, d, false)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.signature(m, c, d, true)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
fn transport() -> Arc<quinn::TransportConfig> {
    let mut c = quinn::TransportConfig::default();
    c.max_concurrent_bidi_streams(4u8.into())
        .max_concurrent_uni_streams(0u8.into());
    c.datagram_receive_buffer_size(Some(128 * 1024))
        .datagram_send_buffer_size(32 * 1024);
    c.max_idle_timeout(Some(Duration::from_secs(3).try_into().unwrap()))
        .keep_alive_interval(Some(Duration::from_secs(1)));
    c.stream_receive_window((MAX_CONTROL_BYTES as u32).into())
        .receive_window((MAX_CONTROL_BYTES as u32 * 4).into());
    Arc::new(c)
}
pub struct MediaListener {
    endpoint: Endpoint,
    bindings: Bindings,
}
impl MediaListener {
    pub fn bind(address: SocketAddr, identity: &QuicIdentity, bindings: Bindings) -> Result<Self> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = Arc::new(Verifier {
            provider: provider.clone(),
            bindings: bindings.clone(),
            pin: None,
        });
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![CertificateDer::from(identity.cert_der.clone())],
                PrivatePkcs8KeyDer::from(identity.key_der.clone()).into(),
            )?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
        ));
        server.transport = transport();
        Ok(Self {
            endpoint: Endpoint::server(server, address)?,
            bindings,
        })
    }
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }
    pub async fn accept(&self) -> Result<MediaSession> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .context("media listener closed")?;
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            let conn = incoming.await?;
            let bind_result = async {
                let (send, mut recv) = conn.accept_bi().await?;
                let message = read_control(&mut recv).await?;
                let MediaControl::Bind {
                    version,
                    peer,
                    control_token,
                    generation,
                } = message
                else {
                    bail!("expected media bind")
                };
                if version != MEDIA_VERSION {
                    bail!("unsupported media version")
                }
                self.bindings.bind(&conn, peer, control_token, generation)?;
                Ok(MediaSession {
                    connection: conn.clone(),
                    send,
                    recv,
                    rate: RateLimit::new(),
                    receive_rate: Mutex::new(RateLimit::new()),
                })
            }
            .await;
            if bind_result.is_err() {
                conn.close(1u32.into(), b"media binding rejected");
            }
            bind_result
        })
        .await;
        result.context("media handshake timeout")?
    }
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"audio disabled");
    }
}
struct RateLimit {
    tokens: f64,
    last: std::time::Instant,
}
impl RateLimit {
    fn new() -> Self {
        Self {
            tokens: 32768.0,
            last: std::time::Instant::now(),
        }
    }
    fn allow(&mut self, bytes: usize) -> bool {
        let now = std::time::Instant::now();
        // 8 channels x 96 kHz x Float32 + bounded protocol overhead.
        self.tokens =
            (self.tokens + now.duration_since(self.last).as_secs_f64() * 4_000_000.0).min(32768.0);
        self.last = now;
        if self.tokens < bytes as f64 {
            return false;
        }
        self.tokens -= bytes as f64;
        true
    }
}
pub struct MediaSession {
    pub connection: Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    rate: RateLimit,
    receive_rate: Mutex<RateLimit>,
}
impl MediaSession {
    pub async fn connect(
        endpoint: &Endpoint,
        address: SocketAddr,
        identity: &QuicIdentity,
        server_pin: PeerCertificateFingerprint,
        binding: &Binding,
    ) -> Result<Self> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = Arc::new(Verifier {
            provider: provider.clone(),
            bindings: Bindings::default(),
            pin: Some(server_pin),
        });
        let mut tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(
                vec![CertificateDer::from(identity.cert_der.clone())],
                PrivatePkcs8KeyDer::from(identity.key_der.clone()).into(),
            )?;
        tls.alpn_protocols = vec![ALPN.to_vec()];
        let mut config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls)?,
        ));
        config.transport_config(transport());
        let conn = tokio::time::timeout(
            Duration::from_secs(2),
            endpoint.connect_with(config, address, "rshare-audio")?,
        )
        .await??;
        let (send, recv) = conn.open_bi().await?;
        let mut session = Self {
            connection: conn,
            send,
            recv,
            rate: RateLimit::new(),
            receive_rate: Mutex::new(RateLimit::new()),
        };
        session
            .send_control(&MediaControl::Bind {
                version: MEDIA_VERSION,
                peer: binding.peer,
                control_token: binding.token,
                generation: binding.generation,
            })
            .await?;
        Ok(session)
    }
    pub async fn send_control(&mut self, message: &MediaControl) -> Result<()> {
        let bytes = serde_json::to_vec(message)?;
        if bytes.len() > MAX_CONTROL_BYTES {
            bail!("audio control frame too large")
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            self.send
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .await?;
            self.send.write_all(&bytes).await
        })
        .await??;
        Ok(())
    }
    pub async fn receive_control(&mut self) -> Result<MediaControl> {
        read_control(&mut self.recv).await
    }
    pub fn max_datagram_size(&self) -> Option<usize> {
        self.connection.max_datagram_size()
    }
    pub fn send_datagram(&mut self, data: Bytes) -> Result<()> {
        if data.len() > 9216 || !self.rate.allow(data.len()) {
            bail!("audio rate or packet limit exceeded")
        }
        self.connection.send_datagram(data)?;
        Ok(())
    }
    pub async fn receive_datagram(&self) -> Result<Bytes> {
        let data = self.connection.read_datagram().await?;
        if data.len() > 9216
            || !self
                .receive_rate
                .lock()
                .map_err(|_| anyhow!("media rate lock poisoned"))?
                .allow(data.len())
        {
            bail!("audio receive rate or packet limit exceeded")
        }
        Ok(data)
    }
}
async fn read_control(recv: &mut quinn::RecvStream) -> Result<MediaControl> {
    let mut length = [0; 4];
    recv.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONTROL_BYTES {
        bail!("invalid audio control length")
    }
    let mut bytes = vec![0; length];
    tokio::time::timeout(Duration::from_secs(2), recv.read_exact(&mut bytes)).await??;
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::Encryption;
    fn identity() -> QuicIdentity {
        let (cert_der, key_der) = Encryption::generate_cert().unwrap();
        QuicIdentity { cert_der, key_der }
    }
    #[tokio::test]
    async fn authenticated_datagrams_and_control_revocation() {
        let server = identity();
        let client = identity();
        let bindings = Bindings::default();
        let binding = Binding {
            peer: DeviceId::new_v4(),
            fingerprint: PeerCertificateFingerprint::from_der(&client.cert_der),
            token: DeviceId::new_v4(),
            generation: 1,
        };
        bindings.authorize(binding.clone()).unwrap();
        let listener =
            MediaListener::bind("127.0.0.1:0".parse().unwrap(), &server, bindings.clone()).unwrap();
        let endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let (client, server) = tokio::join!(
            MediaSession::connect(
                &endpoint,
                listener.local_addr().unwrap(),
                &client,
                PeerCertificateFingerprint::from_der(&server.cert_der),
                &binding
            ),
            listener.accept()
        );
        let mut client = client.unwrap();
        let server = server.unwrap();
        client.send_datagram(Bytes::from_static(b"pcm")).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), server.receive_datagram())
                .await
                .unwrap()
                .unwrap(),
            Bytes::from_static(b"pcm")
        );
        bindings.revoke(binding.peer);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), client.connection.closed())
                .await
                .is_ok()
        );
        listener.close();
        endpoint.close(0u32.into(), b"test done");
    }
    #[tokio::test]
    async fn invalid_control_token_cannot_attach() {
        let server = identity();
        let client = identity();
        let bindings = Bindings::default();
        let mut binding = Binding {
            peer: DeviceId::new_v4(),
            fingerprint: PeerCertificateFingerprint::from_der(&client.cert_der),
            token: DeviceId::new_v4(),
            generation: 1,
        };
        bindings.authorize(binding.clone()).unwrap();
        binding.token = DeviceId::new_v4();
        let listener =
            MediaListener::bind("127.0.0.1:0".parse().unwrap(), &server, bindings).unwrap();
        let endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        let (_, accepted) = tokio::join!(
            MediaSession::connect(
                &endpoint,
                listener.local_addr().unwrap(),
                &client,
                PeerCertificateFingerprint::from_der(&server.cert_der),
                &binding
            ),
            listener.accept()
        );
        assert!(accepted.is_err());
        listener.close();
        endpoint.close(0u32.into(), b"test done");
    }
}
