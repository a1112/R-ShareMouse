//! Network-audio management is isolated from the input runtime. A compiled
//! driver or configured feature must not be reported as an active audio path.
use anyhow::{Context, Result};
use rshare_audio::{backend, registry::Registry};
use rshare_core::network_audio::*;
use std::{io::Write, path::PathBuf};

pub struct Manager {
    config: AudioConfig,
    registry: Registry,
    path: Option<PathBuf>,
    last_error: Option<String>,
}
impl Default for Manager {
    fn default() -> Self {
        Self {
            config: AudioConfig::default(),
            registry: Registry::default(),
            path: None,
            last_error: None,
        }
    }
}
impl Manager {
    pub fn load(path: PathBuf) -> Self {
        let mut manager = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<AudioConfig>(&bytes)
                .map_err(anyhow::Error::from)
                .and_then(|c| {
                    c.validate()?;
                    Ok(c)
                }) {
                Ok(config) => {
                    manager.registry.restore_grants(config.grants.clone());
                    manager.config = config;
                }
                Err(error) => {
                    manager.last_error = Some(format!(
                        "Invalid audio configuration; audio disabled: {error}"
                    ))
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                manager.last_error = Some(format!("Cannot load audio configuration: {error}"))
            }
        }
        manager
    }
    fn persist(&self, config: &AudioConfig) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().context("audio config has no parent")?;
        std::fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temp, config)?;
        temp.write_all(b"\n")?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn snapshot(&self) -> AudioSnapshot {
        let backend = backend::status();
        let unavailable = "Media orchestration and physical audio bridge are not attached; network audio is unavailable";
        let last_error = self.last_error.clone().or_else(|| {
            Some(match backend.error {
                Some(error) => format!("{unavailable}; {error}"),
                None => unavailable.to_string(),
            })
        });
        AudioSnapshot {
            config: self.config.clone(),
            local_endpoints: self.registry.local().to_vec(),
            devices: self.registry.devices().to_vec(),
            sessions: self.registry.sessions(),
            diagnostics: Diagnostics::default(),
            backend: backend.name,
            backend_ready: false,
            last_error,
        }
    }
    pub fn refresh(&mut self) {
        match backend::enumerate() {
            Ok(endpoints) => {
                if let Err(e) = self.registry.set_local(endpoints) {
                    self.last_error = Some(e.to_string());
                }
            }
            Err(error) => self.last_error = Some(error),
        }
    }
    pub fn command(&mut self, command: AudioCommand, trusted: bool) -> Result<AudioSnapshot> {
        match command {
            AudioCommand::Status => self.refresh(),
            AudioCommand::Configure(config) => {
                config.validate()?;
                // Grants have a separate operation requiring an approved peer.
                if config.grants != self.config.grants {
                    anyhow::bail!("Use Grant/Revoke to change audio permissions")
                }
                self.persist(&config)?;
                self.config = config;
            }
            AudioCommand::Grant(grant) => {
                if !trusted {
                    anyhow::bail!(AudioError::Unauthorized)
                }
                self.refresh();
                let mut registry = self.registry.clone();
                registry.grant(grant.peer, &grant.endpoint, grant.direction)?;
                let mut config = self.config.clone();
                config.grants = registry.grants().to_vec();
                self.persist(&config)?;
                self.config = config;
                self.registry = registry;
            }
            AudioCommand::Revoke(grant) => {
                let mut registry = self.registry.clone();
                registry.revoke(&grant);
                let mut config = self.config.clone();
                config.grants = registry.grants().to_vec();
                self.persist(&config)?;
                self.config = config;
                self.registry = registry;
            }
            AudioCommand::Open { .. } => anyhow::bail!(AudioError::BackendUnavailable),
            AudioCommand::Close { session } => self.registry.close(session),
        }
        Ok(self.snapshot())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_configuration_disables_only_audio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.json");
        std::fs::write(&path, b"{oops").unwrap();
        let manager = Manager::load(path);
        assert!(!manager.snapshot().config.enabled);
        assert!(manager
            .snapshot()
            .last_error
            .unwrap()
            .contains("Invalid audio configuration"));
    }
    #[test]
    fn configuration_round_trip_and_no_inline_permission_escalation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.json");
        let mut manager = Manager::load(path.clone());
        let config = AudioConfig {
            buffer_ms: 5,
            ..Default::default()
        };
        manager
            .command(AudioCommand::Configure(config.clone()), false)
            .unwrap();
        assert_eq!(Manager::load(path).snapshot().config, config);
        let mut invalid = config;
        invalid.grants.push(Grant {
            peer: rshare_core::DeviceId::new_v4(),
            endpoint: "mic".into(),
            direction: Direction::Input,
        });
        assert!(manager
            .command(AudioCommand::Configure(invalid), false)
            .is_err());
    }
}
