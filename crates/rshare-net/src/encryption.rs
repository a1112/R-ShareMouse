//! QUIC certificate and TOFU trust utilities.

use anyhow::{Context, Result};
use rshare_core::DeviceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const QUIC_CERT_FILE: &str = "quic-cert.der";
const QUIC_KEY_FILE: &str = "quic-key.pkcs8.der";
const QUIC_TRUST_FILE: &str = "quic-trust.json";
static IDENTITY_LOCK: Mutex<()> = Mutex::new(());
static TRUST_STORE_LOCK: Mutex<()> = Mutex::new(());
static TRUST_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static DEFAULT_IDENTITY_LOADS: AtomicU64 = AtomicU64::new(0);

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
        serde_json::from_slice(&data)
            .with_context(|| format!("Failed to parse QUIC trust store {}", path.display()))
    }

    pub fn save_default(&self) -> Result<()> {
        self.save(trust_store_path()?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let _guard = TRUST_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("QUIC trust store lock poisoned"))?;
        let mut merged = Self::load(path)?;
        for (device_id, fingerprint) in &self.peers {
            match merged.peers.get(device_id) {
                Some(existing) if existing != fingerprint => {
                    anyhow::bail!(
                        "Refusing to overwrite QUIC trust pin for {}: existing {}, requested {}",
                        device_id,
                        existing,
                        fingerprint
                    );
                }
                Some(_) => {}
                None => {
                    merged.peers.insert(*device_id, fingerprint.clone());
                }
            }
        }
        write_trust_store_atomic(path, &merged)
    }

    pub fn check(
        &self,
        device_id: DeviceId,
        fingerprint: &PeerCertificateFingerprint,
    ) -> QuicTrustDecision {
        match self.peers.get(&device_id) {
            None => QuicTrustDecision::FirstSeen,
            Some(expected) if expected == fingerprint => QuicTrustDecision::Trusted,
            Some(expected) => QuicTrustDecision::Rejected {
                expected: expected.clone(),
                actual: fingerprint.clone(),
            },
        }
    }

    pub fn trust_first_seen(
        &mut self,
        device_id: DeviceId,
        fingerprint: PeerCertificateFingerprint,
    ) -> QuicTrustDecision {
        let decision = self.check(device_id, &fingerprint);
        if decision == QuicTrustDecision::FirstSeen {
            self.peers.insert(device_id, fingerprint);
        }
        decision
    }

    pub fn trust_first_seen_default(
        device_id: DeviceId,
        fingerprint: PeerCertificateFingerprint,
    ) -> Result<QuicTrustDecision> {
        Self::trust_first_seen_at(trust_store_path()?, device_id, fingerprint)
    }

    pub fn trust_first_seen_at(
        path: impl AsRef<Path>,
        device_id: DeviceId,
        fingerprint: PeerCertificateFingerprint,
    ) -> Result<QuicTrustDecision> {
        let path = path.as_ref();
        let _guard = TRUST_STORE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("QUIC trust store lock poisoned"))?;
        let mut store = Self::load(path)?;
        let decision = store.trust_first_seen(device_id, fingerprint);
        if decision == QuicTrustDecision::FirstSeen {
            write_trust_store_atomic(path, &store)?;
        }
        Ok(decision)
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
        #[cfg(test)]
        DEFAULT_IDENTITY_LOADS.fetch_add(1, Ordering::AcqRel);
        let _guard = IDENTITY_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("QUIC identity lock poisoned"))?;
        let state_dir = rshare_core::service::state_dir()?;
        Self::load_or_generate_identity_in(&state_dir)
    }

    #[cfg(test)]
    pub(crate) fn reset_default_identity_loads_for_test() {
        DEFAULT_IDENTITY_LOADS.store(0, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn default_identity_loads_for_test() -> u64 {
        DEFAULT_IDENTITY_LOADS.load(Ordering::Acquire)
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

fn write_trust_store_atomic(path: &Path, store: &QuicTrustStore) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "Failed to create QUIC trust store parent {}",
            parent.display()
        )
    })?;

    let data = serde_json::to_vec_pretty(store)?;
    let (temp_path, mut temp_file) = create_trust_temp_file(path, parent)?;
    let write_result = temp_file
        .write_all(&data)
        .and_then(|_| temp_file.sync_all())
        .with_context(|| {
            format!(
                "Failed to write temporary QUIC trust store {}",
                temp_path.display()
            )
        });
    drop(temp_file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    if let Err(error) = atomic_replace(&temp_path, path).with_context(|| {
        format!(
            "Failed to replace QUIC trust store {} with {}",
            path.display(),
            temp_path.display()
        )
    }) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(())
}

fn create_trust_temp_file(path: &Path, parent: &Path) -> Result<(PathBuf, File)> {
    let file_name = path.file_name().ok_or_else(|| {
        anyhow::anyhow!("QUIC trust store path has no file name: {}", path.display())
    })?;

    for _ in 0..16 {
        let counter = TRUST_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut temp_name = OsString::from(".");
        temp_name.push(file_name);
        temp_name.push(format!(".{}.{}.tmp", std::process::id(), counter));
        let temp_path = parent.join(temp_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to create temporary QUIC trust store {}",
                        temp_path.display()
                    )
                });
            }
        }
    }

    anyhow::bail!(
        "Failed to allocate a unique temporary QUIC trust store beside {}",
        path.display()
    )
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
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

    fn temp_dir(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("rshare-state")
            .join(format!("rshare-{name}-{}", uuid::Uuid::new_v4()))
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
        let store = QuicTrustStore::default();

        assert_eq!(
            store.check(device_id, &fingerprint_a),
            QuicTrustDecision::FirstSeen
        );
        let mut store = store;
        assert_eq!(
            store.trust_first_seen(device_id, fingerprint_a.clone()),
            QuicTrustDecision::FirstSeen
        );
        assert_eq!(
            store.check(device_id, &fingerprint_a),
            QuicTrustDecision::Trusted
        );
        assert!(matches!(
            store.check(device_id, &fingerprint_b),
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
        store.trust_first_seen(device_id, fingerprint.clone());
        store.save(&path).unwrap();

        let loaded = QuicTrustStore::load(&path).unwrap();
        assert_eq!(loaded.fingerprint_for(&device_id), Some(&fingerprint));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn checking_first_seen_peer_does_not_mutate_store() {
        let device_id = DeviceId::new_v4();
        let fingerprint = PeerCertificateFingerprint::from_der(b"cert-a");
        let store = QuicTrustStore::default();

        assert_eq!(
            store.check(device_id, &fingerprint),
            QuicTrustDecision::FirstSeen
        );
        assert!(store.fingerprint_for(&device_id).is_none());
    }

    #[test]
    fn empty_trust_store_file_fails_closed() {
        let dir = temp_dir("quic-trust-empty");
        let path = dir.join("trust.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"").unwrap();

        assert!(QuicTrustStore::load(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_trust_store_file_fails_closed() {
        let dir = temp_dir("quic-trust-malformed");
        let path = dir.join("trust.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"{ not valid json").unwrap();

        assert!(QuicTrustStore::load(&path).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_saves_preserve_existing_peer_pins() {
        let dir = temp_dir("quic-trust-merge");
        let path = dir.join("trust.json");
        let first_id = DeviceId::new_v4();
        let second_id = DeviceId::new_v4();
        let first_fingerprint = PeerCertificateFingerprint::from_der(b"cert-a");
        let second_fingerprint = PeerCertificateFingerprint::from_der(b"cert-b");
        let mut first = QuicTrustStore::load(&path).unwrap();
        let mut stale = QuicTrustStore::load(&path).unwrap();

        first.trust_first_seen(first_id, first_fingerprint.clone());
        first.save(&path).unwrap();
        stale.trust_first_seen(second_id, second_fingerprint.clone());
        stale.save(&path).unwrap();

        let loaded = QuicTrustStore::load(&path).unwrap();
        assert_eq!(loaded.fingerprint_for(&first_id), Some(&first_fingerprint));
        assert_eq!(
            loaded.fingerprint_for(&second_id),
            Some(&second_fingerprint)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_first_seen_commits_preserve_all_peer_pins() {
        let dir = temp_dir("quic-trust-concurrent");
        let path = dir.join("trust.json");
        let first_id = DeviceId::new_v4();
        let second_id = DeviceId::new_v4();
        let first_fingerprint = PeerCertificateFingerprint::from_der(b"cert-a");
        let second_fingerprint = PeerCertificateFingerprint::from_der(b"cert-b");

        let first_path = path.clone();
        let first_fingerprint_for_thread = first_fingerprint.clone();
        let first = std::thread::spawn(move || {
            QuicTrustStore::trust_first_seen_at(&first_path, first_id, first_fingerprint_for_thread)
                .unwrap()
        });
        let second_path = path.clone();
        let second_fingerprint_for_thread = second_fingerprint.clone();
        let second = std::thread::spawn(move || {
            QuicTrustStore::trust_first_seen_at(
                &second_path,
                second_id,
                second_fingerprint_for_thread,
            )
            .unwrap()
        });

        assert_eq!(first.join().unwrap(), QuicTrustDecision::FirstSeen);
        assert_eq!(second.join().unwrap(), QuicTrustDecision::FirstSeen);
        let loaded = QuicTrustStore::load(&path).unwrap();
        assert_eq!(loaded.fingerprint_for(&first_id), Some(&first_fingerprint));
        assert_eq!(
            loaded.fingerprint_for(&second_id),
            Some(&second_fingerprint)
        );
        let _ = fs::remove_dir_all(dir);
    }
}
