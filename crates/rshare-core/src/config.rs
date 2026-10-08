//! Application configuration.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::protocol::{DeviceId, Direction};

/// Application configuration shared by the GUI and engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub gui: GuiConfig,
    #[serde(default)]
    pub input: InputConfig,
    #[serde(default)]
    pub gamepad: GamepadConfig,
    #[serde(default)]
    pub features: FeatureConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    /// Known device hostnames
    #[serde(default)]
    pub known_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkConfig {
    pub port: u16,
    pub bind_address: String,
    /// Automatically connect and reconnect only certificate-pinned, operator-approved peers.
    #[serde(default = "default_true")]
    pub auto_connect_trusted: bool,
    /// Legacy mDNS switch. Discovery currently uses UDP broadcast only.
    #[serde(default)]
    pub mdns_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiConfig {
    pub minimize_to_tray: bool,
    pub show_notifications: bool,
    pub start_minimized: bool,
    pub show_tray_icon: bool,
    #[serde(default)]
    pub screen_layout: Vec<ScreenLayoutEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScreenLayoutEntry {
    pub device_id: DeviceId,
    pub direction: Direction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputConfig {
    pub clipboard_sync: bool,
    pub edge_threshold: u32,
    /// Send mouse wheel events
    pub mouse_wheel_sync: bool,
    /// Key delay in milliseconds (for macro protection)
    pub key_delay_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GamepadConfig {
    /// Enable gamepad sharing.
    pub enabled: bool,
    /// How gamepad input is routed.
    #[serde(default)]
    pub routing_mode: GamepadRoutingMode,
    /// Deadzone in basis points. 800 = 8%.
    pub deadzone_basis_points: u16,
    /// Maximum state snapshot rate.
    pub max_update_hz: u16,
    /// Enable future vibration passthrough when supported.
    pub vibration: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureConfig {
    /// Enable the experimental plaintext LAN mobile controller gateway.
    #[serde(default)]
    pub mobile_gateway_enabled: bool,
    /// Suppress local OS shortcuts while this machine is controlling a remote target.
    #[serde(default = "default_true")]
    pub suppress_local_shortcuts_when_remote: bool,
    /// Allow edge-triggered automatic forwarding of captured input to a remote target.
    ///
    /// Enabled by default so a connected, linked peer can be controlled from the
    /// local screen edge without requiring an extra diagnostics-only toggle.
    #[serde(default = "default_true")]
    pub automatic_input_forwarding: bool,
    /// Ask the remote endpoint to run the reverse latency probe automatically.
    #[serde(default = "default_true")]
    pub auto_endpoint_latency_probe: bool,
    /// Allow local audio capture for diagnostics.
    #[serde(default = "default_true")]
    pub audio_capture: bool,
    /// Allow audio forwarding to a remote active target.
    #[serde(default = "default_true")]
    pub audio_forwarding: bool,
    /// Enable the experimental USB forwarding host path.
    #[serde(default)]
    pub usb_forwarding_experimental: bool,
    /// Advertise local USB devices to connected peers when USB forwarding is enabled.
    #[serde(default = "default_true")]
    pub usb_device_advertising: bool,
    /// Allow descriptor probes against USB devices advertised by remote peers.
    #[serde(default = "default_true")]
    pub usb_descriptor_probe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GamepadRoutingMode {
    Disabled,
    LocalOnly,
    FixedTarget { device_id: DeviceId },
    FollowActiveKeyboardMouseTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityConfig {
    pub password_required: bool,
    /// Enable TLS/SSL encryption
    pub encryption: bool,
    /// Password hash (bcrypt)
    pub password_hash: Option<String>,
    /// Trusted device IDs
    pub trusted_devices: Vec<DeviceId>,
    /// Allow LAN only (prevent WAN access)
    pub lan_only: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: 27431,
            bind_address: "0.0.0.0".to_string(),
            auto_connect_trusted: true,
            mdns_enabled: false,
        }
    }
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            show_notifications: true,
            start_minimized: false,
            show_tray_icon: true,
            screen_layout: Vec::new(),
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            clipboard_sync: true,
            edge_threshold: 10,
            mouse_wheel_sync: true,
            key_delay_ms: 0,
        }
    }
}

impl Default for GamepadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            routing_mode: GamepadRoutingMode::Disabled,
            deadzone_basis_points: 800,
            max_update_hz: 120,
            vibration: false,
        }
    }
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            mobile_gateway_enabled: false,
            suppress_local_shortcuts_when_remote: true,
            automatic_input_forwarding: true,
            auto_endpoint_latency_probe: true,
            audio_capture: true,
            audio_forwarding: true,
            usb_forwarding_experimental: false,
            usb_device_advertising: true,
            usb_descriptor_probe: true,
        }
    }
}

impl Default for GamepadRoutingMode {
    fn default() -> Self {
        Self::Disabled
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            password_required: false,
            encryption: true,
            password_hash: None,
            trusted_devices: Vec::new(),
            lan_only: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: NetworkConfig::default(),
            gui: GuiConfig::default(),
            input: InputConfig::default(),
            gamepad: GamepadConfig::default(),
            features: FeatureConfig::default(),
            security: SecurityConfig::default(),
            known_devices: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

impl Config {
    /// Load configuration from the default config path, creating it if missing.
    pub fn load() -> Result<Self> {
        Self::load_from_path(default_config_path()?)
    }

    /// Save configuration to the default config path.
    pub fn save(&self) -> Result<()> {
        self.save_to_path(default_config_path()?)
    }

    /// Load configuration from a specific path, creating a default file if missing.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            let config = Self::default();
            config.save_to_path(path)?;
            return Ok(config);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Save configuration to a specific path.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        use std::io::Write;

        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create config directory: {}", parent.display())
        })?;
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        let temporary = parent.join(format!(".rshare-config-{}.tmp", DeviceId::new_v4()));
        let result = (|| -> std::io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            if let Ok(metadata) = std::fs::metadata(path) {
                file.set_permissions(metadata.permissions())?;
            }
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.with_context(|| format!("Failed to write config file: {}", path.display()))
    }

    /// Get the bind address for the server
    pub fn bind_address(&self) -> Result<SocketAddr> {
        let host = self.network.bind_address.as_str();
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        let ip = host.parse().context("Invalid bind IP address")?;
        Ok(SocketAddr::new(ip, self.network.port))
    }

    /// Check if a device is trusted
    pub fn is_trusted(&self, device_id: &DeviceId) -> bool {
        self.security.trusted_devices.contains(device_id)
    }

    /// Add a trusted device
    pub fn add_trusted_device(&mut self, device_id: DeviceId) {
        if !self.is_trusted(&device_id) {
            self.security.trusted_devices.push(device_id);
        }
    }

    /// Remove a trusted device
    pub fn remove_trusted_device(&mut self, device_id: &DeviceId) {
        self.security.trusted_devices.retain(|id| id != device_id);
    }

    /// Get the effective edge threshold
    pub fn edge_threshold(&self) -> u32 {
        self.input.edge_threshold.max(1).min(100)
    }

    /// Legacy method for compatibility
    pub fn config_path() -> PathBuf {
        default_config_path().unwrap_or_else(|_| PathBuf::from("config.toml"))
    }
}

/// Get the default configuration file path.
pub fn default_config_path() -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "macos") {
        dirs::home_dir().map(|p| p.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        dirs::config_dir()
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
    };

    Ok(base_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rshare")
        .join("config.toml"))
}

/// Hotkey configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyConfig {
    /// Toggle sharing hotkey (Ctrl+Alt+S by default)
    pub toggle_sharing: Option<String>,
    /// Lock cursor hotkey (Ctrl+Alt+L by default)
    pub lock_cursor: Option<String>,
    /// Hotkey to switch to specific screen
    pub switch_screen: Vec<SwitchScreenHotkey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SwitchScreenHotkey {
    pub hotkey: String,
    pub device_id: DeviceId,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            toggle_sharing: Some("Ctrl+Alt+S".to_string()),
            lock_cursor: Some("Ctrl+Alt+L".to_string()),
            switch_screen: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_config_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("rshare-config-test-{}-{}", name, Uuid::new_v4()))
            .join("config.toml")
    }

    #[test]
    fn default_config_matches_expected_values() {
        let config = Config::default();
        assert_eq!(config.network.port, 27431);
        assert_eq!(config.network.bind_address, "0.0.0.0");
        assert!(!config.network.mdns_enabled);
        assert!(config.gui.minimize_to_tray);
        assert!(config.gui.show_notifications);
        assert!(config.gui.show_tray_icon);
        assert!(!config.gui.start_minimized);
        assert!(config.gui.screen_layout.is_empty());
        assert!(config.input.clipboard_sync);
        assert_eq!(config.input.edge_threshold, 10);
        assert!(config.input.mouse_wheel_sync);
        assert!(!config.gamepad.enabled);
        assert_eq!(config.gamepad.routing_mode, GamepadRoutingMode::Disabled);
        assert_eq!(config.gamepad.deadzone_basis_points, 800);
        assert_eq!(config.gamepad.max_update_hz, 120);
        assert!(!config.gamepad.vibration);
        assert!(config.features.suppress_local_shortcuts_when_remote);
        assert!(config.features.automatic_input_forwarding);
        assert!(config.features.auto_endpoint_latency_probe);
        assert!(config.features.audio_capture);
        assert!(config.features.audio_forwarding);
        assert!(!config.features.usb_forwarding_experimental);
        assert!(config.features.usb_device_advertising);
        assert!(config.features.usb_descriptor_probe);
        assert!(!config.security.password_required);
        assert!(config.security.encryption);
        assert!(config.security.lan_only);
    }

    #[test]
    fn mobile_gateway_is_disabled_by_default() {
        assert!(!Config::default().features.mobile_gateway_enabled);
    }

    #[test]
    fn legacy_mdns_true_remains_deserializable_but_default_is_udp_only() {
        let legacy: Config = toml::from_str(
            "[network]\nport = 27431\nbind_address = \"0.0.0.0\"\nmdns_enabled = true\n",
        )
        .unwrap();
        assert!(legacy.network.mdns_enabled);
        assert!(!Config::default().network.mdns_enabled);
    }

    #[test]
    fn trusted_auto_connect_defaults_for_legacy_config_and_preserves_opt_out() {
        let legacy: Config = toml::from_str(
            "[network]\nport = 27431\nbind_address = \"0.0.0.0\"\n",
        )
        .unwrap();
        assert!(legacy.network.auto_connect_trusted);

        let disabled: Config = toml::from_str(
            "[network]\nport = 27431\nbind_address = \"0.0.0.0\"\nauto_connect_trusted = false\n",
        )
        .unwrap();
        assert!(!disabled.network.auto_connect_trusted);
    }

    #[test]
    fn load_creates_missing_config_file() {
        let path = temp_config_path("missing");
        let config = Config::load_from_path(&path).unwrap();
        assert_eq!(config, Config::default());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_round_trips_config() {
        let path = temp_config_path("roundtrip");
        let mut config = Config::default();
        config.network.port = 4242;
        config.gui.start_minimized = true;
        config.input.edge_threshold = 24;
        config.gamepad.enabled = true;
        config.gamepad.routing_mode = GamepadRoutingMode::FixedTarget {
            device_id: DeviceId::new_v4(),
        };
        config.features.usb_forwarding_experimental = true;
        config.features.audio_forwarding = false;
        config.security.password_required = true;
        config.gui.screen_layout.push(ScreenLayoutEntry {
            device_id: DeviceId::new_v4(),
            direction: Direction::Right,
        });

        config.save_to_path(&path).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();

        assert_eq!(loaded, config);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_replaces_existing_config_and_cleans_failed_temporary_files() {
        let path = temp_config_path("atomic-replace");
        Config::default().save_to_path(&path).unwrap();
        let mut changed = Config::default();
        changed.network.auto_connect_trusted = false;
        changed.save_to_path(&path).unwrap();
        assert_eq!(Config::load_from_path(&path).unwrap(), changed);
        let parent = path.parent().unwrap();
        let blocked_path = parent.join("blocked.toml");
        std::fs::create_dir(&blocked_path).unwrap();
        let marker = blocked_path.join("preserved");
        std::fs::write(&marker, b"original").unwrap();
        assert!(changed.save_to_path(&blocked_path).is_err());
        assert_eq!(std::fs::read(&marker).unwrap(), b"original");
        assert_eq!(Config::load_from_path(&path).unwrap(), changed);
        assert!(std::fs::read_dir(parent).unwrap().all(|entry| {
            !entry.unwrap().file_name().to_string_lossy().starts_with(".rshare-config-")
        }));
        let _ = std::fs::remove_dir_all(parent);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_config_save_preserves_private_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_config_path("permissions");
        Config::default().save_to_path(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        Config::default().save_to_path(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn feature_config_defaults_when_section_is_missing_or_partial() {
        let loaded: Config = toml::from_str("known_devices = []").unwrap();
        assert_eq!(loaded.features, FeatureConfig::default());

        let partial: Config =
            toml::from_str("[features]\nusb_forwarding_experimental = true\n").unwrap();
        assert!(partial.features.automatic_input_forwarding);
        assert!(partial.features.usb_forwarding_experimental);
        assert!(partial.features.audio_capture);
        assert!(partial.features.audio_forwarding);
        assert!(partial.features.usb_device_advertising);
        assert!(partial.features.usb_descriptor_probe);
    }

    #[test]
    fn missing_mobile_gateway_field_defaults_to_disabled() {
        let loaded: Config =
            toml::from_str("[features]\nautomatic_input_forwarding = true\n").unwrap();

        assert!(!loaded.features.mobile_gateway_enabled);
    }

    #[test]
    fn load_preserves_explicit_automatic_forwarding_opt_out() {
        let path = temp_config_path("explicit-forwarding-opt-out");
        let mut config = Config::default();
        config.features.automatic_input_forwarding = false;
        config.save_to_path(&path).unwrap();

        let loaded = Config::load_from_path(&path).unwrap();

        assert!(!loaded.features.automatic_input_forwarding);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_bind_address() {
        let config = Config::default();
        let addr = config.bind_address().unwrap();
        assert_eq!(addr.port(), 27431);
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
    }

    #[test]
    fn bind_address_accepts_ipv6_without_requiring_manual_brackets() {
        let mut config = Config::default();
        for host in ["::1", "[::1]", "::"] {
            config.network.bind_address = host.to_string();
            let address = config.bind_address().unwrap();
            assert!(address.is_ipv6());
            assert_eq!(address.port(), config.network.port);
        }
        for host in ["[::1", "::1]", "127.0.0.1:1234", "example.com"] {
            config.network.bind_address = host.to_string();
            assert!(config.bind_address().is_err(), "invalid bind host: {host}");
        }
    }

    #[test]
    fn test_edge_threshold_bounds() {
        let mut config = Config::default();
        config.input.edge_threshold = 0;
        assert_eq!(config.edge_threshold(), 1);

        config.input.edge_threshold = 200;
        assert_eq!(config.edge_threshold(), 100);
    }

    #[test]
    fn test_trusted_devices() {
        let mut config = Config::default();
        let id = DeviceId::new_v4();

        assert!(!config.is_trusted(&id));
        config.add_trusted_device(id);
        assert!(config.is_trusted(&id));
        config.remove_trusted_device(&id);
        assert!(!config.is_trusted(&id));
    }
}
