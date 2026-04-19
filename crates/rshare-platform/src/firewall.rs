//! Windows firewall configuration for R-ShareMouse
//!
//! Automatically configures Windows Defender Firewall to allow
//! R-ShareMouse discovery and service ports.

cfg_if::cfg_if! {
    if #[cfg(windows)] {
        pub use windows_impl::*;
    } else {
        pub use no_op_impl::*;
    }
}

#[cfg(windows)]
mod windows_impl {
    use anyhow::{Context, Result};
use std::process::Command;

/// Default ports used by R-ShareMouse
pub const DISCOVERY_PORT: u16 = 27432;
pub const SERVICE_PORT: u16 = 27431;

/// Configure Windows Firewall to allow R-ShareMouse
///
/// This function adds firewall rules for:
/// - UDP port 27432 (device discovery)
/// - TCP port 27431 (service communication)
///
/// # Errors
///
/// Returns an error if:
/// - The process is not running with administrator privileges
/// - `netsh` command fails
pub fn configure_firewall() -> Result<FirewallConfigResult> {
    let mut result = FirewallConfigResult::default();

    // Check if running as admin
    if !is_elevated() {
        return Err(anyhow::anyhow!(
            "Administrator privileges required to configure firewall. \
             Please restart as administrator or manually add firewall rules:\n\
             netsh advfirewall firewall add rule name=\"R-ShareMouse Discovery (UDP-In)\" \
             dir=in action=allow protocol=UDP localport=27432\n\
             netsh advfirewall firewall add rule name=\"R-ShareMouse Service (TCP-In)\" \
             dir=in action=allow protocol=TCP localport=27431"
        ));
    }

    // Add UDP discovery rule
    match add_firewall_rule(
        "R-ShareMouse Discovery (UDP-In)",
        "27432",
        "UDP",
    ) {
        Ok(existed) => {
            result.udp_discovery = if existed {
                FirewallRuleStatus::AlreadyExisted
            } else {
                FirewallRuleStatus::Created
            };
        }
        Err(e) => {
            tracing::warn!("Failed to add UDP discovery rule: {}", e);
            result.udp_discovery = FirewallRuleStatus::Failed(e.to_string());
        }
    }

    // Add TCP service rule
    match add_firewall_rule(
        "R-ShareMouse Service (TCP-In)",
        "27431",
        "TCP",
    ) {
        Ok(existed) => {
            result.tcp_service = if existed {
                FirewallRuleStatus::AlreadyExisted
            } else {
                FirewallRuleStatus::Created
            };
        }
        Err(e) => {
            tracing::warn!("Failed to add TCP service rule: {}", e);
            result.tcp_service = FirewallRuleStatus::Failed(e.to_string());
        }
    }

    Ok(result)
}

/// Check if a firewall rule exists
pub fn check_firewall_rules() -> bool {
    if !is_elevated() {
        return false;
    }

    check_rule_exists("R-ShareMouse Discovery (UDP-In)")
        && check_rule_exists("R-ShareMouse Service (TCP-In)")
}

/// Remove R-ShareMouse firewall rules
pub fn remove_firewall_rules() -> Result<()> {
    if !is_elevated() {
        return Err(anyhow::anyhow!(
            "Administrator privileges required to remove firewall rules"
        ));
    }

    let _ = remove_firewall_rule("R-ShareMouse Discovery (UDP-In)");
    let _ = remove_firewall_rule("R-ShareMouse Service (TCP-In)");

    Ok(())
}

/// Result of firewall configuration
#[derive(Debug, Clone, Default)]
pub struct FirewallConfigResult {
    pub udp_discovery: FirewallRuleStatus,
    pub tcp_service: FirewallRuleStatus,
}

impl FirewallConfigResult {
    /// Check if all rules were successfully configured
    pub fn is_success(&self) -> bool {
        matches!(
            self.udp_discovery,
            FirewallRuleStatus::Created | FirewallRuleStatus::AlreadyExisted
        ) && matches!(
            self.tcp_service,
            FirewallRuleStatus::Created | FirewallRuleStatus::AlreadyExisted
        )
    }
}

/// Status of a firewall rule
#[derive(Debug, Clone)]
pub enum FirewallRuleStatus {
    /// Rule was created successfully
    Created,
    /// Rule already existed
    AlreadyExisted,
    /// Failed to create rule
    Failed(String),
}

impl Default for FirewallRuleStatus {
    fn default() -> Self {
        Self::Failed("Not attempted".to_string())
    }
}

/// Add a firewall rule using netsh
fn add_firewall_rule(name: &str, port: &str, protocol: &str) -> Result<bool> {
    let output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name=\"{}\"", name),
            "dir=in",
            "action=allow",
            &format!("protocol={}", protocol),
            &format!("localport={}", port),
            "profile=domain,private,public",
        ])
        .output()
        .context("Failed to execute netsh command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check if rule already existed
    if stdout.contains("The object already exists") || stderr.contains("The object already exists") {
        tracing::debug!("Firewall rule '{}' already exists", name);
        return Ok(true);
    }

    if output.status.success() {
        tracing::info!("Added firewall rule '{}' for {} port {}", name, protocol, port);
        Ok(false)
    } else {
        Err(anyhow::anyhow!(
            "netsh failed: {}",
            stdout.trim().trim_end_matches('\n')
        ))
    }
}

/// Check if a firewall rule exists
fn check_rule_exists(name: &str) -> bool {
    match Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "show",
            "rule",
            &format!("name=\"{}\"", name),
        ])
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.contains("OK") || stdout.contains(name)
        }
        Err(_) => false,
    }
}

/// Remove a firewall rule
fn remove_firewall_rule(name: &str) -> Result<()> {
    let output = Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name=\"{}\"", name),
        ])
        .output()
        .context("Failed to execute netsh delete command")?;

    if output.status.success() {
        tracing::info!("Removed firewall rule '{}'", name);
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No rules match") {
            // Rule didn't exist, that's fine
            Ok(())
        } else {
            Err(anyhow::anyhow!("netsh delete failed: {}", stderr))
        }
    }
}

/// Check if the process is running with administrator privileges
fn is_elevated() -> bool {
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Security::{
            GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
        };
        use windows::Win32::System::Threading::{
            GetCurrentProcess, OpenProcessToken,
        };

        let mut token: HANDLE = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_ok() {
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut size = 0;

            let result = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elevation as *mut _ as *mut _),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut size,
            ).is_ok();

            return result && elevation.TokenIsElevated != 0;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_firewall_rule_name_formatting() {
        // Verify that rule names don't have unescaped quotes
        let name = "R-ShareMouse Discovery (UDP-In)";
        let formatted = format!("name=\"{}\"", name);
        assert_eq!(formatted, "name=\"R-ShareMouse Discovery (UDP-In)\"");
    }

    #[test]
    fn test_firewall_config_result_default() {
        let result = FirewallConfigResult::default();
        // By default, rules are in failed state
        assert!(!result.is_success());
    }

    #[test]
    fn test_firewall_config_result_success() {
        let mut result = FirewallConfigResult::default();
        result.udp_discovery = FirewallRuleStatus::Created;
        result.tcp_service = FirewallRuleStatus::AlreadyExisted;
        assert!(result.is_success());
    }
}
}

// Stub implementation for non-Windows platforms
#[cfg(not(windows))]
mod no_op_impl {
    use anyhow::Result;

    pub const DISCOVERY_PORT: u16 = 27432;
    pub const SERVICE_PORT: u16 = 27431;

    #[derive(Debug, Clone, Default)]
    pub struct FirewallConfigResult {
        pub dummy: bool,
    }

    impl FirewallConfigResult {
        pub fn is_success(&self) -> bool {
            true
        }
    }

    #[derive(Debug, Clone)]
    pub enum FirewallRuleStatus {
        Created,
        AlreadyExisted,
        Failed(String),
    }

    impl Default for FirewallRuleStatus {
        fn default() -> Self {
            Self::Created
        }
    }

    pub fn configure_firewall() -> Result<FirewallConfigResult> {
        Ok(FirewallConfigResult::default())
    }

    pub fn check_firewall_rules() -> bool {
        true
    }

    pub fn remove_firewall_rules() -> Result<()> {
        Ok(())
    }
}
