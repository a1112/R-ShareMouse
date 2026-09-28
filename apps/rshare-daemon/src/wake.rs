//! Daemon-owned Wake-on-LAN target persistence and attempt tracking.

use anyhow::{bail, Context, Result};
use rshare_core::{DeviceId, WakeAttemptSnapshot, WakeAttemptStatus, WakeTarget, WakeTargetInput};
use std::collections::HashMap;
use std::fs;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;

pub struct WakeManager {
    path: Option<PathBuf>,
    targets: HashMap<DeviceId, WakeTarget>,
    attempts: HashMap<DeviceId, WakeAttemptSnapshot>,
    read_error: Option<String>,
}

impl WakeManager {
    pub fn empty() -> Self {
        Self {
            path: None,
            targets: HashMap::new(),
            attempts: HashMap::new(),
            read_error: None,
        }
    }

    pub fn unavailable(path: PathBuf, error: String) -> Self {
        Self {
            path: Some(path),
            targets: HashMap::new(),
            attempts: HashMap::new(),
            read_error: Some(error),
        }
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let saved: Vec<WakeTarget> = if path.exists() {
            serde_json::from_slice(
                &fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
            )
            .with_context(|| format!("parsing {}", path.display()))?
        } else {
            Vec::new()
        };
        let mut manager = Self {
            path: Some(path),
            ..Self::empty()
        };
        for target in saved {
            let input = WakeTargetInput {
                id: Some(target.id),
                name: target.name.clone(),
                mac: target.mac.clone(),
                peer_id: target.peer_id,
                ipv4: target.ipv4,
            };
            let validated = input.validate().map_err(anyhow::Error::msg)?;
            if validated != target
                || manager.targets.contains_key(&target.id)
                || manager
                    .targets
                    .values()
                    .any(|other| other.peer_id.is_some() && other.peer_id == target.peer_id)
            {
                bail!("Invalid or duplicate saved Wake-on-LAN target");
            }
            manager.targets.insert(target.id, target);
        }
        Ok(manager)
    }

    pub fn targets(&self) -> Result<Vec<WakeTarget>> {
        self.check_available()?;
        let mut targets: Vec<_> = self.targets.values().cloned().collect();
        targets.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(targets)
    }

    pub fn get(&self, id: DeviceId) -> Option<WakeTarget> {
        self.targets.get(&id).cloned()
    }

    pub fn save_target(&mut self, input: WakeTargetInput) -> Result<WakeTarget> {
        self.check_available()?;
        let is_update = input.id.is_some();
        let target = input.validate().map_err(anyhow::Error::msg)?;
        if is_update && !self.targets.contains_key(&target.id) {
            bail!("Wake target not found");
        }
        if self.targets.values().any(|other| {
            other.id != target.id && other.peer_id.is_some() && other.peer_id == target.peer_id
        }) {
            bail!("A wake target already exists for this peer");
        }
        let mut next = self.targets.clone();
        next.insert(target.id, target.clone());
        self.persist(&next)?;
        self.targets = next;
        Ok(target)
    }

    pub fn delete_target(&mut self, id: DeviceId) -> Result<()> {
        self.check_available()?;
        let mut next = self.targets.clone();
        if next.remove(&id).is_none() {
            bail!("Wake target not found");
        }
        self.persist(&next)?;
        self.targets = next;
        Ok(())
    }

    fn check_available(&self) -> Result<()> {
        if let Some(error) = &self.read_error {
            bail!("Wake target store unavailable: {error}");
        }
        Ok(())
    }

    fn persist(&self, targets: &HashMap<DeviceId, WakeTarget>) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut ordered: Vec<_> = targets.values().collect();
        ordered.sort_by_key(|target| target.id);
        let encoded = serde_json::to_vec_pretty(&ordered)?;
        let temporary = path.with_extension(format!("json.tmp-{}", DeviceId::new_v4()));
        fs::write(&temporary, encoded)?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| format!("saving {}", path.display()));
        }
        Ok(())
    }

    pub fn begin(
        &mut self,
        target_id: DeviceId,
    ) -> Result<(WakeTarget, WakeAttemptSnapshot, bool)> {
        self.check_available()?;
        let target = self.get(target_id).context("Wake target not found")?;
        if let Some(active) = self.attempts.values().find(|attempt| {
            attempt.target_id == target_id && attempt.status == WakeAttemptStatus::Waiting
        }) {
            return Ok((target, active.clone(), false));
        }
        let now = timestamp_ms();
        let attempt = WakeAttemptSnapshot {
            id: DeviceId::new_v4(),
            target_id,
            status: WakeAttemptStatus::Waiting,
            message: "Wake packet is being sent".into(),
            started_at_ms: now,
            deadline_at_ms: now + 90_000,
        };
        self.attempts.insert(attempt.id, attempt.clone());
        self.prune_attempts();
        Ok((target, attempt, true))
    }

    pub fn attempt(&self, id: DeviceId) -> Option<WakeAttemptSnapshot> {
        self.attempts.get(&id).cloned()
    }

    pub fn attempts(&self) -> Vec<WakeAttemptSnapshot> {
        let mut attempts: Vec<_> = self.attempts.values().cloned().collect();
        attempts.sort_by_key(|attempt| std::cmp::Reverse(attempt.started_at_ms));
        attempts
    }

    pub fn finish(&mut self, id: DeviceId, status: WakeAttemptStatus, message: String) {
        if let Some(attempt) = self.attempts.get_mut(&id) {
            if attempt.status == WakeAttemptStatus::Waiting {
                attempt.status = status;
                attempt.message = message;
            }
        }
    }

    pub fn set_waiting_message(&mut self, id: DeviceId, message: String) {
        if let Some(attempt) = self.attempts.get_mut(&id) {
            if attempt.status == WakeAttemptStatus::Waiting {
                attempt.message = message;
            }
        }
    }

    fn prune_attempts(&mut self) {
        if self.attempts.len() <= 128 {
            return;
        }
        let mut finished: Vec<_> = self
            .attempts
            .values()
            .filter(|attempt| attempt.status != WakeAttemptStatus::Waiting)
            .map(|attempt| (attempt.started_at_ms, attempt.id))
            .collect();
        finished.sort_unstable();
        for (_, id) in finished.into_iter().take(self.attempts.len() - 128) {
            self.attempts.remove(&id);
        }
    }
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    Responded,
    NoReply,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    pub status: WakeAttemptStatus,
    pub message: String,
}

pub async fn wait_for_confirmation<F, Fut>(
    timeout: Duration,
    period: Duration,
    mut probe: F,
) -> ProbeOutcome
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ProbeResult>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match probe().await {
            ProbeResult::Responded => {
                return ProbeOutcome {
                    status: WakeAttemptStatus::Confirmed,
                    message: "Target responded".into(),
                }
            }
            ProbeResult::Unavailable(reason) => {
                return ProbeOutcome {
                    status: WakeAttemptStatus::Unconfirmed,
                    message: format!("Could not confirm target: {reason}"),
                }
            }
            ProbeResult::NoReply => {}
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + period).min(deadline)).await;
    }
    ProbeOutcome {
        status: WakeAttemptStatus::Unconfirmed,
        message: "No response within 90 seconds; power state is unconfirmed".into(),
    }
}

pub async fn ping_ipv4(ip: Ipv4Addr) -> ProbeResult {
    let mut command = Command::new("ping");
    command.stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(windows)]
    command.args(["-n", "1", "-w", "1000"]);
    #[cfg(target_os = "linux")]
    command.args(["-n", "-c", "1", "-W", "1"]);
    #[cfg(target_os = "macos")]
    command.args(["-n", "-c", "1", "-W", "1000"]);
    command.arg(ip.to_string()).kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(2), command.status()).await {
        Ok(Ok(status)) if status.success() => ProbeResult::Responded,
        Ok(Ok(_)) => ProbeResult::NoReply,
        Ok(Err(error)) => ProbeResult::Unavailable(error.to_string()),
        Err(_) => ProbeResult::NoReply,
    }
}
