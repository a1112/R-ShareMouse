//! Service process management
//!
//! Handles service lifecycle, PID files, daemon mode, and graceful shutdown.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::sleep;
use tracing;

/// Service manager for controlling the R-ShareMouse service lifecycle
pub struct ServiceManager {
    /// PID file path
    pid_file: PathBuf,

    /// Service state directory
    state_dir: PathBuf,

    /// Shutdown signal sender
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
}

impl ServiceManager {
    /// Create a new service manager
    pub fn new() -> Result<Self> {
        let state_dir = get_state_dir()?;
        let pid_file = state_dir.join("rshare.pid");

        // Ensure state directory exists
        fs::create_dir_all(&state_dir).context("Failed to create state directory")?;

        Ok(Self {
            pid_file,
            state_dir,
            shutdown_tx: None,
        })
    }

    /// Check if service is running
    pub fn is_running(&self) -> bool {
        if !self.pid_file.exists() {
            return false;
        }

        match self.read_pid() {
            Ok(pid) => self.is_process_alive(pid),
            Err(_) => false,
        }
    }

    /// Get the PID of the running service
    pub fn get_pid(&self) -> Option<u32> {
        self.read_pid()
            .ok()
            .filter(|&pid| self.is_process_alive(pid))
    }

    /// Start the service
    pub async fn start(&mut self) -> Result<ServiceHandle> {
        // Check if already running
        if self.is_running() {
            let pid = self.read_pid()?;
            anyhow::bail!("Service is already running (PID: {})", pid);
        }

        tracing::info!("Starting R-ShareMouse service...");

        // Create shutdown channel
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Write PID file
        self.write_pid()?;

        tracing::info!("Service started (PID: {})", std::process::id());

        Ok(ServiceHandle {
            shutdown_tx,
            pid_file: self.pid_file.clone(),
        })
    }

    /// Stop the service gracefully
    pub async fn stop(&self) -> Result<()> {
        let pid = match self.get_pid() {
            Some(p) => p,
            None => {
                tracing::warn!("Service is not running");
                // Clean up stale PID file
                self.remove_pid_file()?;
                return Ok(());
            }
        };

        tracing::info!("Stopping service (PID: {})...", pid);

        // Send graceful shutdown signal
        // Note: In real implementation, this would use IPC to notify the service
        // For now, we'll use a signal (Unix) or terminate (Windows)

        #[cfg(unix)]
        {
            use std::process::Command;
            // Try SIGTERM first for graceful shutdown
            let _ = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }

        #[cfg(windows)]
        {
            use std::process::Command;
            // Use taskkill on Windows
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .status();
        }

        // Wait for process to terminate
        for _ in 0..50 {
            sleep(Duration::from_millis(100)).await;
            if !self.is_process_alive(pid) {
                self.remove_pid_file()?;
                tracing::info!("Service stopped");
                return Ok(());
            }
        }

        // Force kill if still running
        tracing::warn!("Service did not stop gracefully, forcing...");
        self.force_kill(pid)?;
        self.remove_pid_file()?;

        Ok(())
    }

    /// Force kill the service
    fn force_kill(&self, pid: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::process::Command;
            Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status()
                .context("Failed to kill process")?;
        }

        #[cfg(windows)]
        {
            use std::process::Command;
            Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .status()
                .context("Failed to kill process")?;
        }

        Ok(())
    }

    /// Read PID from file
    fn read_pid(&self) -> Result<u32> {
        let content = fs::read_to_string(&self.pid_file).context("Failed to read PID file")?;
        content.trim().parse::<u32>().context("Invalid PID in file")
    }

    /// Write current PID to file
    fn write_pid(&self) -> Result<()> {
        let pid = std::process::id();
        fs::write(&self.pid_file, pid.to_string()).context("Failed to write PID file")?;
        tracing::debug!("Wrote PID file: {}", self.pid_file.display());
        Ok(())
    }

    /// Remove PID file
    fn remove_pid_file(&self) -> Result<()> {
        if self.pid_file.exists() {
            fs::remove_file(&self.pid_file).context("Failed to remove PID file")?;
        }
        Ok(())
    }

    /// Check if a process with given PID is alive
    #[cfg(unix)]
    fn is_process_alive(&self, pid: u32) -> bool {
        use std::process::Command;
        // Try to send signal 0 to check if process exists
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Check if a process with given PID is alive (Windows)
    #[cfg(windows)]
    fn is_process_alive(&self, pid: u32) -> bool {
        use std::process::Command;
        // Use tasklist to check if process exists
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .map(|o| {
                let output = String::from_utf8_lossy(&o.stdout);
                output.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }

    /// Get the state directory
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new().expect("Failed to create ServiceManager")
    }
}

/// Get the state directory for R-ShareMouse.
pub fn state_dir() -> Result<PathBuf> {
    get_state_dir()
}

/// Get the PID file path for the local daemon.
pub fn pid_file_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("rshare.pid"))
}

/// Handle for a running service
pub struct ServiceHandle {
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
    pid_file: PathBuf,
}

impl ServiceHandle {
    /// Get a receiver for shutdown signals
    pub fn shutdown_rx(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    /// Trigger graceful shutdown
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }

    /// Wait for shutdown signal
    pub async fn wait_for_shutdown(&self) {
        let mut rx = self.shutdown_rx();
        rx.recv().await.ok();
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // Clean up PID file on drop
        let _ = std::fs::remove_file(&self.pid_file);
    }
}

/// Get the state directory for R-ShareMouse
fn get_state_dir() -> Result<PathBuf> {
    let base_dir = if cfg!(target_os = "windows") {
        dirs::config_dir()
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|p| p.join("Library").join("Application Support"))
    } else {
        // Linux/Unix: use XDG_CONFIG_HOME or ~/.config
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".config")))
    };

    let state_dir = base_dir
        .map(|p| p.join("rshare"))
        .unwrap_or_else(|| PathBuf::from(".rshare"));

    Ok(state_dir)
}

/// Spawn service in daemon mode (Unix only)
#[cfg(unix)]
pub fn spawn_daemon() -> Result<()> {
    use anyhow::bail;

    // Double-fork to daemonize
    unsafe {
        // First fork
        match libc::fork() {
            -1 => bail!("First fork failed: {}", std::io::Error::last_os_error()),
            0 => {
                // Child process
                // Create new session
                if libc::setsid() == -1 {
                    bail!("setsid failed: {}", std::io::Error::last_os_error());
                }

                // Second fork
                match libc::fork() {
                    -1 => bail!("Second fork failed: {}", std::io::Error::last_os_error()),
                    0 => {
                        // Daemon process
                        // Change to root directory
                        libc::chdir(b"/\0".as_ptr() as *const i8);

                        // Redirect stdio to /dev/null
                        let dev_null =
                            libc::open(b"/dev/null\0".as_ptr() as *const i8, libc::O_RDWR);
                        if dev_null != -1 {
                            libc::dup2(dev_null, 0);
                            libc::dup2(dev_null, 1);
                            libc::dup2(dev_null, 2);
                            libc::close(dev_null);
                        }

                        Ok(())
                    }
                    _ => std::process::exit(0),
                }
            }
            _ => std::process::exit(0),
        }
    }
}

/// No-op for daemon mode on non-Unix platforms
#[cfg(not(unix))]
pub fn spawn_daemon() -> Result<()> {
    anyhow::bail!("Daemon mode is not supported on this platform");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_manager_creation() {
        let manager = ServiceManager::new().unwrap();
        assert!(manager.state_dir().ends_with("rshare"));
    }

    #[test]
    fn test_is_process_alive() {
        // Test with current process
        let manager = ServiceManager::new().unwrap();
        assert!(manager.is_process_alive(std::process::id()));
        assert!(!manager.is_process_alive(999999));
    }
}
