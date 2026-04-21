//! Cross-platform system integration helpers.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported host platform identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Macos,
    Windows,
    Linux,
    Unknown,
}

/// Command specification used to open a path with the platform shell.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenCommandSpec {
    program: &'static str,
    args: Vec<String>,
}

/// Return the current host platform.
pub fn platform_kind() -> PlatformKind {
    if cfg!(target_os = "macos") {
        PlatformKind::Macos
    } else if cfg!(target_os = "windows") {
        PlatformKind::Windows
    } else if cfg!(target_os = "linux") {
        PlatformKind::Linux
    } else {
        PlatformKind::Unknown
    }
}

/// Human-readable current host platform name.
pub fn platform_name() -> &'static str {
    match platform_kind() {
        PlatformKind::Macos => "macOS",
        PlatformKind::Windows => "Windows",
        PlatformKind::Linux => "Linux",
        PlatformKind::Unknown => "Unknown",
    }
}

/// Return the R-ShareMouse configuration directory.
pub fn config_dir() -> Result<PathBuf> {
    let config_path = rshare_core::config::default_config_path()?;
    Ok(config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(".")))
}

/// Ensure and return the R-ShareMouse configuration directory.
pub fn ensure_config_dir() -> Result<PathBuf> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create config directory: {}", dir.display()))?;
    Ok(dir)
}

/// Return the daemon log file path.
pub fn log_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("rshare-daemon.log"))
}

/// Open the R-ShareMouse configuration directory in the native file manager.
pub fn open_config_dir() -> Result<()> {
    let dir = ensure_config_dir()?;
    open_path(dir)
}

/// Open the daemon log file in the native application for log files.
///
/// The file is created when missing so the platform opener has a concrete target.
pub fn open_log_file() -> Result<()> {
    let log_file = log_file_path()?;
    if let Some(parent) = log_file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create log directory: {}", parent.display()))?;
    }
    if !log_file.exists() {
        std::fs::File::create(&log_file)
            .with_context(|| format!("Failed to create log file: {}", log_file.display()))?;
    }
    open_path(log_file)
}

/// Open a local path with the native platform opener.
pub fn open_path(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let spec = open_command_for_path(path);
    Command::new(spec.program)
        .args(&spec.args)
        .spawn()
        .with_context(|| format!("Failed to open path: {}", path.display()))?;
    Ok(())
}

fn open_command_for_path(path: &Path) -> OpenCommandSpec {
    let path = path.to_string_lossy().into_owned();
    match platform_kind() {
        PlatformKind::Macos => OpenCommandSpec {
            program: "open",
            args: vec![path],
        },
        PlatformKind::Windows => OpenCommandSpec {
            program: "cmd",
            args: vec!["/C".to_string(), "start".to_string(), String::new(), path],
        },
        PlatformKind::Linux | PlatformKind::Unknown => OpenCommandSpec {
            program: "xdg-open",
            args: vec![path],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_name_is_stable_for_supported_targets() {
        assert!(!platform_name().is_empty());
        assert_ne!(platform_name(), "Unknown");
    }

    #[test]
    fn config_and_log_paths_share_rshare_directory() {
        let config_dir = config_dir().expect("config dir should resolve");
        let log_file = log_file_path().expect("log path should resolve");

        assert_eq!(
            config_dir.file_name().and_then(|name| name.to_str()),
            Some("rshare")
        );
        assert_eq!(log_file.parent(), Some(config_dir.as_path()));
        assert_eq!(
            log_file.file_name().and_then(|name| name.to_str()),
            Some("rshare-daemon.log")
        );
    }

    #[test]
    fn opener_command_targets_current_platform() {
        let spec = open_command_for_path(Path::new("/tmp/rshare-test"));

        match platform_kind() {
            PlatformKind::Macos => assert_eq!(spec.program, "open"),
            PlatformKind::Windows => {
                assert_eq!(spec.program, "cmd");
                assert_eq!(spec.args.get(1).map(String::as_str), Some("start"));
            }
            PlatformKind::Linux | PlatformKind::Unknown => assert_eq!(spec.program, "xdg-open"),
        }
        assert_eq!(
            spec.args.last().map(String::as_str),
            Some("/tmp/rshare-test")
        );
    }
}
