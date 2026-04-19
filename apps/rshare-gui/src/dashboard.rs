use rshare_core::{DaemonDeviceSnapshot, ServiceStatusSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardSummary {
    pub service_label: String,
    pub service_running: bool,
    pub device_count: usize,
    pub connected_count: usize,
    pub network_label: String,
    pub clipboard_label: String,
    pub input_backend_label: String,
    pub backend_healthy: bool,
}

impl DashboardSummary {
    pub fn from_snapshots(
        status: Option<&ServiceStatusSnapshot>,
        devices: &[DaemonDeviceSnapshot],
    ) -> Self {
        match status {
            Some(status) => {
                let (input_backend_label, backend_healthy) = if let Some(backend_health) = &status.backend_health {
                    match backend_health {
                        rshare_core::BackendHealth::Healthy => {
                            (format!("{:?}", status.input_mode.as_ref().unwrap_or(&rshare_core::ResolvedInputMode::Portable)), true)
                        }
                        rshare_core::BackendHealth::Degraded { .. } => {
                            ("Degraded".to_string(), false)
                        }
                    }
                } else {
                    ("Unknown".to_string(), false)
                };

                Self {
                    service_label: "● Running".to_string(),
                    service_running: true,
                    device_count: devices.len(),
                    connected_count: devices.iter().filter(|device| device.connected).count(),
                    network_label: status.bind_address.clone(),
                    clipboard_label: if status.healthy {
                        "Ready".to_string()
                    } else {
                        "Degraded".to_string()
                    },
                    input_backend_label,
                    backend_healthy,
                }
            }
            None => Self {
                service_label: "○ Stopped".to_string(),
                service_running: false,
                device_count: 0,
                connected_count: 0,
                network_label: "Daemon offline".to_string(),
                clipboard_label: "Unavailable".to_string(),
                input_backend_label: "Unavailable".to_string(),
                backend_healthy: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn summary_uses_real_status_when_daemon_is_running() {
        let status = ServiceStatusSnapshot {
            device_id: Uuid::nil(),
            device_name: "desktop".to_string(),
            hostname: "desktop-host".to_string(),
            bind_address: "0.0.0.0:27431".to_string(),
            discovery_port: 27432,
            pid: 77,
            discovered_devices: 2,
            connected_devices: 1,
            healthy: true,
        };
        let devices = vec![
            DaemonDeviceSnapshot {
                id: Uuid::nil(),
                name: "desktop".to_string(),
                hostname: "desktop-host".to_string(),
                addresses: vec!["192.168.1.10:27431".to_string()],
                connected: true,
                last_seen_secs: Some(1),
            },
            DaemonDeviceSnapshot {
                id: Uuid::from_u128(1),
                name: "macbook".to_string(),
                hostname: "macbook-host".to_string(),
                addresses: vec!["192.168.1.11:27431".to_string()],
                connected: false,
                last_seen_secs: Some(5),
            },
        ];

        let summary = DashboardSummary::from_snapshots(Some(&status), &devices);

        assert!(summary.service_running);
        assert_eq!(summary.service_label, "● Running");
        assert_eq!(summary.device_count, 2);
        assert_eq!(summary.connected_count, 1);
        assert_eq!(summary.network_label, "0.0.0.0:27431");
        assert_eq!(summary.clipboard_label, "Ready");
    }

    #[test]
    fn summary_falls_back_to_stopped_when_daemon_is_unavailable() {
        let summary = DashboardSummary::from_snapshots(None, &[]);

        assert!(!summary.service_running);
        assert_eq!(summary.service_label, "○ Stopped");
        assert_eq!(summary.device_count, 0);
        assert_eq!(summary.connected_count, 0);
        assert_eq!(summary.network_label, "Daemon offline");
        assert_eq!(summary.clipboard_label, "Unavailable");
    }
}
