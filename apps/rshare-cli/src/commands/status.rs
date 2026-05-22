//! Status command implementation.

use anyhow::Result;
use colored::Colorize;

use crate::output::{header, kv, status_err, status_ok};

/// Execute the status command.
pub async fn execute(detailed: bool) -> Result<()> {
    header("Service Status");

    let manager = rshare_core::service::ServiceManager::new()?;
    if !manager.is_running() {
        status_err("Service Status: Stopped");
        return Ok(());
    }

    match rshare_core::daemon_client::request_status().await {
        Ok(status) => {
            let capabilities = if detailed {
                rshare_core::daemon_client::request_capabilities(None)
                    .await
                    .ok()
            } else {
                None
            };
            status_ok("Service Status: Running");
            kv("PID", &status.pid.to_string());
            kv("Device", &status.device_name);
            kv("Hostname", &status.hostname);

            if detailed {
                print_detailed_status(&status, capabilities.as_ref());
            }
        }
        Err(err) => {
            status_err("Service Status: Unresponsive");
            if let Some(pid) = manager.get_pid() {
                kv("PID", &pid.to_string());
            }
            kv("Error", &err.to_string());
        }
    }

    Ok(())
}

fn latency_feedback_lines(
    feedback: &rshare_core::LatencyFeedbackSnapshot,
) -> Vec<(String, String)> {
    let mut lines = vec![
        (
            "Local Input".to_string(),
            local_input_latency_feedback_value(&feedback.local_input),
        ),
        (
            "Transport Feedback".to_string(),
            transport_latency_feedback_value(&feedback.transport),
        ),
    ];

    for device in &feedback.remote_latency.devices {
        lines.push((
            remote_device_latency_feedback_key(device),
            remote_device_latency_feedback_value(device),
        ));
    }

    lines
}

fn local_input_latency_feedback_value(feedback: &rshare_core::LocalInputFeedback) -> String {
    let mut parts = vec![
        latency_feedback_status_label(feedback.status).to_string(),
        event_count_label(feedback.event_count),
    ];

    if let Some(capture_path) = non_empty(feedback.capture_path.as_deref()) {
        parts.push(format!("capture {capture_path}"));
    }
    if let Some(sequence) = feedback.latest_sequence {
        parts.push(format!("seq {sequence}"));
    }

    parts.join(", ")
}

fn transport_latency_feedback_value(feedback: &rshare_core::TransportFeedback) -> String {
    let mut parts = vec![
        latency_feedback_status_label(feedback.status).to_string(),
        feedback.transport.clone(),
    ];

    if let Some(rtt_ms) = feedback.rtt_ms {
        parts.push(format!("{rtt_ms} ms RTT"));
    }

    parts.push(if feedback.datagram_available {
        "datagram available".to_string()
    } else {
        "datagram unavailable".to_string()
    });

    if feedback.realtime_degraded {
        parts.push("realtime degraded".to_string());
    }
    if feedback.datagram_tx_dropped > 0 {
        parts.push(format!(
            "{} datagram {}",
            feedback.datagram_tx_dropped,
            plural(feedback.datagram_tx_dropped, "drop", "drops")
        ));
    }
    if feedback.reliable_stream_reset_count > 0 {
        parts.push(format!(
            "{} reliable {}",
            feedback.reliable_stream_reset_count,
            plural(feedback.reliable_stream_reset_count, "reset", "resets")
        ));
    }

    parts.join(", ")
}

fn remote_device_latency_feedback_key(
    feedback: &rshare_core::RemoteDeviceLatencyFeedback,
) -> String {
    non_empty(feedback.device_name.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| short_device_id(&feedback.device_id))
}

fn remote_device_latency_feedback_value(
    feedback: &rshare_core::RemoteDeviceLatencyFeedback,
) -> String {
    let mut parts = vec![latency_feedback_status_label(feedback.status).to_string()];

    match feedback.status {
        rshare_core::LatencyFeedbackStatus::Pending => {
            if let Some(duration_ms) = feedback.pending_duration_ms {
                parts.push(format!("{duration_ms} ms pending"));
            } else {
                parts.push("awaiting ack".to_string());
            }
        }
        rshare_core::LatencyFeedbackStatus::Timeout => {
            if let Some(duration_ms) = feedback.pending_duration_ms {
                parts.push(format!("{duration_ms} ms pending"));
            } else {
                parts.push("probe timed out".to_string());
            }
        }
        _ => {}
    }

    if let Some(rtt_ms) = feedback.network_round_trip_ms {
        parts.push(format!("{rtt_ms} ms RTT"));
    }
    if let Some(one_way_ms) = feedback.estimated_one_way_ms {
        parts.push(format!("{one_way_ms} ms one-way"));
    }
    if feedback.network_round_trip_ms.is_none() {
        if let Some(raw_rtt_ms) = feedback.raw_round_trip_ms {
            parts.push(format!("{raw_rtt_ms} ms raw RTT"));
        }
    }
    if let Some(remote_processing_ms) = feedback.remote_processing_ms {
        parts.push(format!("{remote_processing_ms} ms remote processing"));
    }
    if let Some(direction) = non_empty(feedback.direction.as_deref()) {
        parts.push(format!("direction {direction}"));
    }
    if parts.len() == 1 {
        parts.push(latency_feedback_empty_detail(feedback.status).to_string());
    }

    parts.join(", ")
}

fn latency_feedback_status_label(status: rshare_core::LatencyFeedbackStatus) -> &'static str {
    match status {
        rshare_core::LatencyFeedbackStatus::Idle => "idle",
        rshare_core::LatencyFeedbackStatus::Pending => "pending",
        rshare_core::LatencyFeedbackStatus::Healthy => "healthy",
        rshare_core::LatencyFeedbackStatus::Degraded => "degraded",
        rshare_core::LatencyFeedbackStatus::Timeout => "timeout",
        rshare_core::LatencyFeedbackStatus::Unavailable => "unavailable",
    }
}

fn latency_feedback_empty_detail(status: rshare_core::LatencyFeedbackStatus) -> &'static str {
    match status {
        rshare_core::LatencyFeedbackStatus::Idle => "no active probe",
        rshare_core::LatencyFeedbackStatus::Pending => "awaiting ack",
        rshare_core::LatencyFeedbackStatus::Healthy => "no RTT metrics",
        rshare_core::LatencyFeedbackStatus::Degraded => "no RTT metrics",
        rshare_core::LatencyFeedbackStatus::Timeout => "probe timed out",
        rshare_core::LatencyFeedbackStatus::Unavailable => "no active connection",
    }
}

fn event_count_label(count: u64) -> String {
    format!("{} {}", count, plural(count, "event", "events"))
}

fn plural(count: u64, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn short_device_id(device_id: &uuid::Uuid) -> String {
    device_id.to_string().chars().take(8).collect()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::latency_feedback_lines;

    #[test]
    fn latency_feedback_lines_include_remote_metrics() {
        let remote_id = uuid::Uuid::nil();
        let feedback = rshare_core::LatencyFeedbackSnapshot {
            remote_latency: rshare_core::RemoteLatencyFeedback {
                status: rshare_core::LatencyFeedbackStatus::Healthy,
                devices: vec![rshare_core::RemoteDeviceLatencyFeedback {
                    device_id: remote_id,
                    status: rshare_core::LatencyFeedbackStatus::Healthy,
                    device_name: Some("remote".to_string()),
                    last_probe_sent_ms: None,
                    last_ack_ms: Some(1_000),
                    pending_duration_ms: None,
                    network_round_trip_ms: Some(24),
                    raw_round_trip_ms: None,
                    estimated_one_way_ms: Some(12),
                    remote_processing_ms: None,
                    direction: Some("origin_to_endpoint".to_string()),
                    summary: Some("Latency to remote: 24 ms RTT / ~12 ms one-way".to_string()),
                }],
            },
            ..rshare_core::LatencyFeedbackSnapshot::default()
        };

        let lines = latency_feedback_lines(&feedback);

        assert!(lines.iter().any(|(key, value)| {
            key == "remote" && value.contains("24 ms RTT") && value.contains("12 ms one-way")
        }));
    }

    #[test]
    fn latency_feedback_lines_include_pending_duration() {
        let feedback = rshare_core::LatencyFeedbackSnapshot {
            remote_latency: rshare_core::RemoteLatencyFeedback {
                status: rshare_core::LatencyFeedbackStatus::Pending,
                devices: vec![rshare_core::RemoteDeviceLatencyFeedback {
                    device_id: uuid::Uuid::nil(),
                    status: rshare_core::LatencyFeedbackStatus::Pending,
                    device_name: Some("pending-device".to_string()),
                    last_probe_sent_ms: Some(1_000),
                    last_ack_ms: None,
                    pending_duration_ms: Some(250),
                    network_round_trip_ms: None,
                    raw_round_trip_ms: None,
                    estimated_one_way_ms: None,
                    remote_processing_ms: None,
                    direction: None,
                    summary: None,
                }],
            },
            ..rshare_core::LatencyFeedbackSnapshot::default()
        };

        let lines = latency_feedback_lines(&feedback);

        assert!(lines.iter().any(|(key, value)| {
            key == "pending-device" && value.contains("pending") && value.contains("250 ms")
        }));
    }

    #[test]
    fn latency_feedback_lines_include_timeout_duration() {
        let feedback = rshare_core::LatencyFeedbackSnapshot {
            remote_latency: rshare_core::RemoteLatencyFeedback {
                status: rshare_core::LatencyFeedbackStatus::Timeout,
                devices: vec![rshare_core::RemoteDeviceLatencyFeedback {
                    device_id: uuid::Uuid::nil(),
                    status: rshare_core::LatencyFeedbackStatus::Timeout,
                    device_name: Some("timeout-device".to_string()),
                    last_probe_sent_ms: Some(1_000),
                    last_ack_ms: None,
                    pending_duration_ms: Some(2_000),
                    network_round_trip_ms: None,
                    raw_round_trip_ms: None,
                    estimated_one_way_ms: None,
                    remote_processing_ms: None,
                    direction: None,
                    summary: None,
                }],
            },
            ..rshare_core::LatencyFeedbackSnapshot::default()
        };

        let lines = latency_feedback_lines(&feedback);

        assert!(lines.iter().any(|(key, value)| {
            key == "timeout-device" && value.contains("timeout") && value.contains("2000 ms")
        }));
    }

    #[test]
    fn latency_feedback_lines_always_include_local_input_and_transport() {
        let feedback = rshare_core::LatencyFeedbackSnapshot {
            local_input: rshare_core::LocalInputFeedback {
                status: rshare_core::LatencyFeedbackStatus::Healthy,
                event_count: 2,
                latest_sequence: Some(4),
                latest_event_ms: None,
                latest_keyboard_event_ms: None,
                latest_mouse_event_ms: None,
                capture_path: Some("portable".to_string()),
            },
            transport: rshare_core::TransportFeedback {
                status: rshare_core::LatencyFeedbackStatus::Healthy,
                transport: "quic".to_string(),
                datagram_available: true,
                realtime_degraded: false,
                rtt_ms: Some(18),
                last_datagram_rx_ms: None,
                datagram_tx_dropped: 0,
                reliable_stream_reset_count: 0,
                cert_trust_state: None,
            },
            ..rshare_core::LatencyFeedbackSnapshot::default()
        };

        let lines = latency_feedback_lines(&feedback);

        assert!(lines
            .iter()
            .any(|(key, value)| key == "Local Input" && value.contains("healthy, 2 events")));
        assert!(lines.iter().any(|(key, value)| {
            key == "Transport Feedback"
                && value.contains("healthy")
                && value.contains("quic")
                && value.contains("18 ms RTT")
        }));
    }
}

fn print_detailed_status(
    status: &rshare_core::ServiceStatusSnapshot,
    capabilities: Option<&rshare_core::CapabilityRegistrySnapshot>,
) {
    println!();
    println!("{}", "Network".bold());
    kv("Listening", &status.bind_address);
    kv("Discovery Port", &status.discovery_port.to_string());
    kv("Discovered Devices", &status.discovered_devices.to_string());
    kv("Connected Devices", &status.connected_devices.to_string());
    kv("Transport", &status.network.transport);
    kv(
        "Realtime Datagram",
        if status.network.datagram_available {
            "available"
        } else {
            "unavailable"
        },
    );
    if let Some(rtt_ms) = status.network.rtt_ms {
        kv("RTT", &format!("{rtt_ms} ms"));
    }
    kv(
        "Datagram Dropped",
        &status.network.datagram_tx_dropped.to_string(),
    );
    kv(
        "Reliable Resets",
        &status.network.reliable_stream_reset_count.to_string(),
    );
    if let Some(cert_state) = &status.network.cert_trust_state {
        kv("Certificate Trust", cert_state);
    }

    println!();
    println!("{}", "Latency Feedback".bold());
    for (key, value) in latency_feedback_lines(&status.latency_feedback) {
        kv(&key, &value);
    }

    println!();
    println!("{}", "Input Backend".bold());
    if let Some(input_mode) = &status.input_mode {
        kv("Mode", &format!("{:?}", input_mode));
    } else {
        kv("Mode", "unknown");
    }
    if let Some(backend_health) = &status.backend_health {
        match backend_health {
            rshare_core::BackendHealth::Healthy => {
                let health = "healthy".green();
                kv("Health", &format!("{}", health));
            }
            rshare_core::BackendHealth::Degraded { reason } => {
                let health = format!("degraded: {:?}", reason).yellow();
                kv("Health", &format!("{}", health));
            }
        }
    }
    if let Some(available) = &status.available_backends {
        let backends: String = available
            .iter()
            .map(|k| format!("{:?}", k))
            .collect::<Vec<_>>()
            .join(", ");
        kv("Available", &backends);
    }
    if let Some(privilege_state) = &status.privilege_state {
        kv("Session", &format!("{:?}", privilege_state));
    }
    if let Some(error) = &status.last_backend_error {
        let err = error.red();
        kv("Last Error", &format!("{}", err));
    }

    println!();
    println!("{}", "Identity".bold());
    kv("Device ID", &status.device_id.to_string());
    kv("Healthy", if status.healthy { "yes" } else { "no" });

    println!();
    println!("{}", "Capabilities".bold());
    match capabilities {
        Some(registry) => {
            for device in &registry.devices {
                let local_suffix = if device.device_id == registry.local_device_id {
                    " (local)"
                } else {
                    ""
                };
                kv(
                    &format!("{}{}", device.device_name, local_suffix),
                    &device
                        .capabilities
                        .iter()
                        .map(|capability| {
                            let reason = capability
                                .health_reason
                                .as_deref()
                                .map(|value| format!(": {value}"))
                                .unwrap_or_default();
                            format!("{:?}={:?}{reason}", capability.kind, capability.state)
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
        }
        None => kv("Registry", "unavailable"),
    }
}
