//! QUIC certificate and TOFU trust utilities.

use anyhow::{Context, Result};
use rshare_core::DeviceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const QUIC_CERT_FILE: &str = "quic-cert.der";
const QUIC_KEY_FILE: &str = "quic-key.pkcs8.der";
const QUIC_TRUST_FILE: &str = "quic-trust.json";
static IDENTITY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone)]
pub struct QuicIdentity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCertificateFingerprint(String);

impl PeerCertificateFingerprint {
    pub fn from_der(cert_der: &[u8]) -> Self {
        let digest = Sha256::digest(cert_der);
        Self(hex_encode(&digest))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerCertificateFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuicTrustDecision {
    FirstSeen,
    Trusted,
    Rejected {
        expected: PeerCertificateFingerprint,
        actual: PeerCertificateFingerprint,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuicTrustStore {
    peers: HashMap<DeviceId, PeerCertificateFingerprint>,
}

impl QuicTrustStore {
    pub fn load_default() -> Result<Self> {
        Self::load(trust_store_path()?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read(path)
            .with_context(|| format!("Failed to read QUIC trust store {}", path.display()))?;
        if data.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        match serde_json::from_slice(&data) {
            Ok(store) => Ok(store),
            Err(error) => {
                tracing::warn!(
                    "Ignoring malformed QUIC trust store {}: {}",
                    path.display(),
                    error
                );
                Ok(Self::default())
            }
        }
    }

    pub fn save_default(&self) -> Result<()> {
        self.save(trust_store_path()?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create QUIC trust store parent {}",
                    parent.display()
                )
            })?;
        }
        let data = serde_json::to_vec_pretty(self)?;
        fs::write(path, data)
            .with_context(|| format!("Failed to write QUIC trust store {}", path.display()))
    }

    pub fn verify_or_trust(
        &mut self,
        device_id: DeviceId,
        fingerprint: PeerCertificateFingerprint,
    ) -> QuicTrustDecision {
        match self.peers.get(&device_id) {
            None => {
                self.peers.insert(device_id, fingerprint);
                QuicTrustDecision::FirstSeen
            }
            Some(expected) if expected == &fingerprint => QuicTrustDecision::Trusted,
            Some(expected) => QuicTrustDecision::Rejected {
                expected: expected.clone(),
                actual: fingerprint,
            },
        }
    }

    pub fn fingerprint_for(&self, device_id: &DeviceId) -> Option<&PeerCertificateFingerprint> {
        self.peers.get(device_id)
    }
}

/// Encryption using rustls (via QUIC).
pub struct Encryption;

impl Encryption {
    /// Generate a self-signed certificate for QUIC device transport.
    pub fn generate_cert() -> Result<(Vec<u8>, Vec<u8>)> {
        let certified_key = rcgen::generate_simple_self_signed(vec!["rshare.local".into()])
            .context("Failed to generate QUIC self-signed certificate")?;
        let cert_der = certified_key.cert.der().to_vec();
        let key_der = certified_key.signing_key.serialize_der();
        Ok((cert_der, key_der))
    }

    /// Load certificate from file.
    pub fn load_cert(path: &str) -> Result<Vec<u8>> {
        fs::read(path).with_context(|| format!("Failed to load certificate {}", path))
    }

    pub fn load_or_generate_default_identity() -> Result<QuicIdentity> {
        let _guard = IDENTITY_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("QUIC identity lock poisoned"))?;
        let state_dir = rshare_core::service::state_dir()?;
        Self::load_or_generate_identity_in(&state_dir)
    }

    pub fn regenerate_default_identity() -> Result<QuicIdentity> {
        let _guard = IDENTITY_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("QUIC identity lock poisoned"))?;
        let state_dir = rshare_core::service::state_dir()?;
        Self::regenerate_identity_in(&state_dir)
    }

    pub fn load_or_generate_identity_in(state_dir: impl AsRef<Path>) -> Result<QuicIdentity> {
        let state_dir = state_dir.as_ref();
        fs::create_dir_all(state_dir).with_context(|| {
            format!(
                "Failed to create QUIC identity state directory {}",
                state_dir.display()
            )
        })?;

        let cert_path = state_dir.join(QUIC_CERT_FILE);
        let key_path = state_dir.join(QUIC_KEY_FILE);
        if cert_path.exists() && key_path.exists() {
            return Ok(QuicIdentity {
                cert_der: fs::read(&cert_path).with_context(|| {
                    format!("Failed to read QUIC certificate {}", cert_path.display())
                })?,
                key_der: fs::read(&key_path)
                    .with_context(|| format!("Failed to read QUIC key {}", key_path.display()))?,
            });
        }

        let (cert_der, key_der) = Self::generate_cert()?;
        fs::write(&cert_path, &cert_der).with_context(|| {
            format!("Failed to persist QUIC certificate {}", cert_path.display())
        })?;
        fs::write(&key_path, &key_der)
            .with_context(|| format!("Failed to persist QUIC key {}", key_path.display()))?;
        Ok(QuicIdentity { cert_der, key_der })
    }

    pub fn regenerate_identity_in(state_dir: impl AsRef<Path>) -> Result<QuicIdentity> {
        let state_dir = state_dir.as_ref();
        fs::create_dir_all(state_dir).with_context(|| {
            format!(
                "Failed to create QUIC identity state directory {}",
                state_dir.display()
            )
        })?;
        let cert_path = state_dir.join(QUIC_CERT_FILE);
        let key_path = state_dir.join(QUIC_KEY_FILE);
        let (cert_der, key_der) = Self::generate_cert()?;
        fs::write(&cert_path, &cert_der).with_context(|| {
            format!("Failed to persist QUIC certificate {}", cert_path.display())
        })?;
        fs::write(&key_path, &key_der)
            .with_context(|| format!("Failed to persist QUIC key {}", key_path.display()))?;
        Ok(QuicIdentity { cert_der, key_der })
    }
}

pub fn trust_store_path() -> Result<PathBuf> {
    Ok(rshare_core::service::state_dir()?.join(QUIC_TRUST_FILE))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rshare-{name}-{suffix}"))
    }

    #[test]
    fn generates_and_reloads_quic_identity() {
        let dir = temp_dir("quic-identity");
        let first = Encryption::load_or_generate_identity_in(&dir).unwrap();
        let second = Encryption::load_or_generate_identity_in(&dir).unwrap();

        assert!(!first.cert_der.is_empty());
        assert!(!first.key_der.is_empty());
        assert_eq!(first.cert_der, second.cert_der);
        assert_eq!(first.key_der, second.key_der);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tofu_first_seen_repeat_and_mismatch() {
        let device_id = DeviceId::new_v4();
        let fingerprint_a = PeerCertificateFingerprint::from_der(b"cert-a");
        let fingerprint_b = PeerCertificateFingerprint::from_der(b"cert-b");
        let mut store = QuicTrustStore::default();

        assert_eq!(
            store.verify_or_trust(device_id, fingerprint_a.clone()),
            QuicTrustDecision::FirstSeen
        );
        assert_eq!(
            store.verify_or_trust(device_id, fingerprint_a),
            QuicTrustDecision::Trusted
        );
        assert!(matches!(
            store.verify_or_trust(device_id, fingerprint_b),
            QuicTrustDecision::Rejected { .. }
        ));
    }

    #[test]
    fn trust_store_roundtrips() {
        let dir = temp_dir("quic-trust");
        let path = dir.join("trust.json");
        let device_id = DeviceId::new_v4();
        let fingerprint = PeerCertificateFingerprint::from_der(b"cert-a");
        let mut store = QuicTrustStore::default();
        store.verify_or_trust(device_id, fingerprint.clone());
        store.save(&path).unwrap();

        let loaded = QuicTrustStore::load(&path).unwrap();
        assert_eq!(loaded.fingerprint_for(&device_id), Some(&fingerprint));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_trust_store_file_loads_as_default() {
        let dir = temp_dir("quic-trust-empty");
        let path = dir.join("trust.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"").unwrap();

        let loaded = QuicTrustStore::load(&path).unwrap();

        assert!(loaded.peers.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_trust_store_file_loads_as_default() {
        let dir = temp_dir("quic-trust-malformed");
        let path = dir.join("trust.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"{ not valid json").unwrap();

        let loaded = QuicTrustStore::load(&path).unwrap();

        assert!(loaded.peers.is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
