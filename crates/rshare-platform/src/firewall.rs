//! Windows firewall configuration for R-ShareMouse
//!
//! Automatically configures Windows Defender Firewall to allow
//! R-ShareMouse discovery and QUIC device transport ports.

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
    pub const MOBILE_GATEWAY_PORT: u16 = 27437;

    const DISCOVERY_RULE_NAME: &str = "R-ShareMouse Discovery (UDP-In)";
    const TRANSPORT_RULE_NAME: &str = "R-ShareMouse Transport (QUIC UDP-In)";
    const MOBILE_GATEWAY_RULE_NAME: &str = "R-ShareMouse Mobile Gateway (TCP-In)";

    #[derive(Debug, Clone, Copy)]
    enum FirewallRuleTarget {
        Discovery,
        Transport,
        MobileGateway,
    }

    #[derive(Debug, Clone, Copy)]
    struct FirewallRuleSpec {
        name: &'static str,
        port: &'static str,
        protocol: &'static str,
        target: FirewallRuleTarget,
    }

    fn required_firewall_rules(mobile_gateway_enabled: bool) -> Vec<FirewallRuleSpec> {
        let mut rules = vec![
            FirewallRuleSpec {
                name: DISCOVERY_RULE_NAME,
                port: "27432",
                protocol: "UDP",
                target: FirewallRuleTarget::Discovery,
            },
            FirewallRuleSpec {
                name: TRANSPORT_RULE_NAME,
                port: "27431",
                protocol: "UDP",
                target: FirewallRuleTarget::Transport,
            },
        ];
        if mobile_gateway_enabled {
            rules.push(FirewallRuleSpec {
                name: MOBILE_GATEWAY_RULE_NAME,
                port: "27437",
                protocol: "TCP",
                target: FirewallRuleTarget::MobileGateway,
            });
        }
        rules
    }

    fn firewall_rule_names_for_removal() -> [&'static str; 3] {
        [
            DISCOVERY_RULE_NAME,
            TRANSPORT_RULE_NAME,
            MOBILE_GATEWAY_RULE_NAME,
        ]
    }

    fn manual_firewall_instructions(mobile_gateway_enabled: bool) -> String {
        required_firewall_rules(mobile_gateway_enabled)
            .into_iter()
            .map(|rule| {
                format!(
                    "netsh advfirewall firewall add rule name=\"{}\" dir=in action=allow protocol={} localport={}",
                    rule.name, rule.protocol, rule.port
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Configure Windows Firewall to allow R-ShareMouse
    ///
    /// This function adds firewall rules for:
    /// - UDP port 27432 (device discovery)
    /// - UDP port 27431 (QUIC device transport)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The process is not running with administrator privileges
    /// - `netsh` command fails
    pub fn configure_firewall(mobile_gateway_enabled: bool) -> Result<FirewallConfigResult> {
        let mut result = FirewallConfigResult::for_mobile_gateway(mobile_gateway_enabled);

        // Check if running as admin
        if !is_elevated() {
            return Err(anyhow::anyhow!(
                "Administrator privileges required to configure firewall. \
                 Please restart as administrator or manually add firewall rules:\n{}",
                manual_firewall_instructions(mobile_gateway_enabled)
            ));
        }

        for rule in required_firewall_rules(mobile_gateway_enabled) {
            let status = match add_firewall_rule(rule.name, rule.port, rule.protocol) {
                Ok(existed) => {
                    if existed {
                        FirewallRuleStatus::AlreadyExisted
                    } else {
                        FirewallRuleStatus::Created
                    }
                }
                Err(error) => {
                    tracing::warn!("Failed to add firewall rule '{}': {}", rule.name, error);
                    FirewallRuleStatus::Failed(error.to_string())
                }
            };
            result.set_status(rule.target, status);
        }

        Ok(result)
    }

    /// Check if every required firewall rule exists.
    pub fn check_firewall_rules(mobile_gateway_enabled: bool) -> bool {
        if !is_elevated() {
            return false;
        }

        required_firewall_rules(mobile_gateway_enabled)
            .into_iter()
            .all(|rule| check_rule_exists(rule.name))
    }

    /// Remove R-ShareMouse firewall rules, including legacy optional rules.
    pub fn remove_firewall_rules() -> Result<()> {
        if !is_elevated() {
            return Err(anyhow::anyhow!(
                "Administrator privileges required to remove firewall rules"
            ));
        }

        for name in firewall_rule_names_for_removal() {
            let _ = remove_firewall_rule(name);
        }

        Ok(())
    }

    /// Result of firewall configuration
    #[derive(Debug, Clone, Default)]
    pub struct FirewallConfigResult {
        pub udp_discovery: FirewallRuleStatus,
        pub quic_transport: FirewallRuleStatus,
        pub mobile_gateway: Option<FirewallRuleStatus>,
    }

    impl FirewallConfigResult {
        fn for_mobile_gateway(enabled: bool) -> Self {
            Self {
                mobile_gateway: if enabled {
                    Some(FirewallRuleStatus::default())
                } else {
                    None
                },
                ..Self::default()
            }
        }

        fn set_status(&mut self, target: FirewallRuleTarget, status: FirewallRuleStatus) {
            match target {
                FirewallRuleTarget::Discovery => self.udp_discovery = status,
                FirewallRuleTarget::Transport => self.quic_transport = status,
                FirewallRuleTarget::MobileGateway => self.mobile_gateway = Some(status),
            }
        }

        /// Check if all required rules were successfully configured
        pub fn is_success(&self) -> bool {
            fn succeeded(status: &FirewallRuleStatus) -> bool {
                matches!(
                    status,
                    FirewallRuleStatus::AlreadyExisted | FirewallRuleStatus::Created
                )
            }

            succeeded(&self.udp_discovery)
                && succeeded(&self.quic_transport)
                && self.mobile_gateway.as_ref().map(succeeded).unwrap_or(true)
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
        if check_rule_exists(name) {
            tracing::debug!("Firewall rule '{}' already exists", name);
            return Ok(true);
        }

        let output = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &firewall_rule_name_arg(name),
                "dir=in",
                "action=allow",
                &format!("protocol={}", protocol),
                &format!("localport={}", port),
                "profile=any",
            ])
            .output()
            .context("Failed to execute netsh command")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Check if rule already existed
        if stdout.contains("The object already exists")
            || stderr.contains("The object already exists")
        {
            tracing::debug!("Firewall rule '{}' already exists", name);
            return Ok(true);
        }

        if output.status.success() {
            tracing::info!(
                "Added firewall rule '{}' for {} port {}",
                name,
                protocol,
                port
            );
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
                &firewall_rule_name_arg(name),
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
                &firewall_rule_name_arg(name),
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
            use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
                )
                .is_ok();

                return result && elevation.TokenIsElevated != 0;
            }
            false
        }
    }

    fn firewall_rule_name_arg(name: &str) -> String {
        format!("name={}", name)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_firewall_rule_name_formatting() {
            let name = "R-ShareMouse Discovery (UDP-In)";
            assert_eq!(
                firewall_rule_name_arg(name),
                "name=R-ShareMouse Discovery (UDP-In)"
            );
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
            result.quic_transport = FirewallRuleStatus::AlreadyExisted;
            assert!(result.is_success());
        }

        #[test]
        fn mobile_gateway_rule_is_required_only_when_enabled() {
            let disabled_rules = required_firewall_rules(false);
            assert!(!disabled_rules
                .iter()
                .any(|rule| { rule.protocol == "TCP" && rule.port == "27437" }));

            let enabled_rules = required_firewall_rules(true);
            assert!(enabled_rules.iter().any(|rule| {
                rule.name == "R-ShareMouse Mobile Gateway (TCP-In)"
                    && rule.protocol == "TCP"
                    && rule.port == "27437"
            }));
        }

        #[test]
        fn disabled_mobile_gateway_does_not_affect_configuration_success() {
            let mut result = FirewallConfigResult::for_mobile_gateway(false);
            result.udp_discovery = FirewallRuleStatus::Created;
            result.quic_transport = FirewallRuleStatus::AlreadyExisted;

            assert!(result.mobile_gateway.is_none());
            assert!(result.is_success());
        }

        #[test]
        fn enabled_mobile_gateway_requires_a_successful_tcp_rule() {
            let mut result = FirewallConfigResult::for_mobile_gateway(true);
            result.udp_discovery = FirewallRuleStatus::Created;
            result.quic_transport = FirewallRuleStatus::AlreadyExisted;

            assert!(result.mobile_gateway.is_some());
            assert!(!result.is_success());

            result.mobile_gateway = Some(FirewallRuleStatus::Created);
            assert!(result.is_success());
        }

        #[test]
        fn firewall_removal_always_includes_the_legacy_mobile_rule() {
            assert!(
                firewall_rule_names_for_removal().contains(&"R-ShareMouse Mobile Gateway (TCP-In)")
            );
        }

        #[test]
        fn manual_instructions_include_mobile_tcp_only_when_enabled() {
            let disabled = manual_firewall_instructions(false);
            assert!(!disabled.contains("localport=27437"));

            let enabled = manual_firewall_instructions(true);
            assert!(enabled.contains("protocol=TCP localport=27437"));
        }
    }
}

// Stub implementation for non-Windows platforms
#[cfg(not(windows))]
mod no_op_impl {
    use anyhow::Result;

    pub const DISCOVERY_PORT: u16 = 27432;
    pub const SERVICE_PORT: u16 = 27431;
    pub const MOBILE_GATEWAY_PORT: u16 = 27437;

    #[derive(Debug, Clone, Default)]
    pub struct FirewallConfigResult {
        pub mobile_gateway: Option<FirewallRuleStatus>,
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

    pub fn configure_firewall(mobile_gateway_enabled: bool) -> Result<FirewallConfigResult> {
        Ok(FirewallConfigResult {
            mobile_gateway: mobile_gateway_enabled.then_some(FirewallRuleStatus::Created),
        })
    }

    pub fn check_firewall_rules(_mobile_gateway_enabled: bool) -> bool {
        true
    }

    pub fn remove_firewall_rules() -> Result<()> {
        Ok(())
    }
}
