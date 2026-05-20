//! Product readiness diagnostics for dual-machine validation.

use anyhow::Result;
use colored::Colorize;
use rshare_core::{
    daemon_client, BackendHealth, CapabilityRegistrySnapshot, CapabilityState,
    DaemonDeviceSnapshot, DeviceId, EndpointCapabilityKind, EndpointEvent, EndpointEventDirection,
    EndpointEventFilter, EndpointEventKind, EndpointEventPayload, EndpointInjectMode,
    EndpointInjectRequest, EndpointInjectResult, EndpointInjectTarget, LayoutGraph,
    LocalControlDeviceSnapshot, ServiceStatusSnapshot,
};

use crate::output::{header, kv};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckState {
    Pass,
    Warn,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DoctorCheck {
    key: &'static str,
    label: &'static str,
    state: CheckState,
    detail: String,
}

/// Execute the doctor command.
pub async fn execute(
    connect: bool,
    inject: bool,
    endpoint_events: u16,
    strict: bool,
) -> Result<()> {
    header("R-ShareMouse Doctor");

    let status_result = daemon_client::request_status().await;
    let status_error = status_result.as_ref().err().map(|error| error.to_string());
    let status = status_result.ok();

    let mut devices = if status.is_some() {
        daemon_client::request_devices().await.unwrap_or_default()
    } else {
        Vec::new()
    };

    if connect {
        connect_discovered_devices(&devices).await;
        devices = daemon_client::request_devices().await.unwrap_or_default();
    }
    let layout = if status.is_some() {
        daemon_client::request_layout().await.ok()
    } else {
        None
    };
    let local_controls = if status.is_some() {
        daemon_client::request_local_controls().await.ok()
    } else {
        None
    };
    let capabilities = if status.is_some() {
        daemon_client::request_capabilities(None).await.ok()
    } else {
        None
    };
    let inject_results = if inject {
        run_remote_inject_probe(&devices).await
    } else {
        Vec::new()
    };
    let endpoint_events = collect_endpoint_events(status.as_ref(), &devices, endpoint_events).await;

    let checks = build_doctor_checks(
        status.as_ref(),
        status_error.as_deref(),
        &devices,
        layout.as_ref(),
        local_controls.as_ref(),
        capabilities.as_ref(),
        &endpoint_events,
        inject,
        &inject_results,
    );

    for check in &checks {
        print_check(check);
    }

    if let Some(status) = &status {
        println!();
        kv(
            "Device",
            &format!("{} ({})", status.device_name, status.hostname),
        );
        kv("Endpoint", &status.bind_address);
        kv("Remote Devices", &devices.len().to_string());
        kv("Endpoint Events", &endpoint_events.len().to_string());
    }

    if strict && checks.iter().any(|check| check.state == CheckState::Block) {
        anyhow::bail!("Doctor found blocking readiness checks.");
    }

    Ok(())
}

async fn connect_discovered_devices(devices: &[DaemonDeviceSnapshot]) {
    for device in devices.iter().filter(|device| !device.connected) {
        match daemon_client::request_connect(device.id).await {
            Ok(()) => println!("  {}", format!("Connected {}", device.name).green()),
            Err(error) => eprintln!(
                "{}",
                format!("  [WARN] Connect {} failed: {error}", device.name).yellow()
            ),
        }
    }
}

async fn collect_endpoint_events(
    status: Option<&ServiceStatusSnapshot>,
    devices: &[DaemonDeviceSnapshot],
    limit: u16,
) -> Vec<EndpointEvent> {
    let Some(status) = status else {
        return Vec::new();
    };

    let mut collected = Vec::new();
    let connected: Vec<DeviceId> = devices
        .iter()
        .filter(|device| device.connected)
        .map(|device| device.id)
        .collect();

    if connected.is_empty() {
        if let Ok(events) = daemon_client::request_endpoint_events(
            EndpointEventFilter {
                endpoint_id: None,
                device_id: None,
                kinds: Vec::new(),
                sources: Vec::new(),
                include_loopback: true,
            },
            None,
            Some(limit),
        )
        .await
        {
            collected.extend(events);
        }
        return collected;
    }

    for endpoint_id in connected {
        if endpoint_id == status.device_id {
            continue;
        }
        if let Ok(events) = daemon_client::request_endpoint_events(
            EndpointEventFilter {
                endpoint_id: Some(endpoint_id),
                device_id: None,
                kinds: Vec::new(),
                sources: Vec::new(),
                include_loopback: true,
            },
            None,
            Some(limit),
        )
        .await
        {
            collected.extend(events);
        }
    }

    collected
}

async fn run_remote_inject_probe(devices: &[DaemonDeviceSnapshot]) -> Vec<EndpointInjectResult> {
    let Some(target) = devices
        .iter()
        .find(|device| device.connected)
        .map(|device| device.id)
    else {
        return Vec::new();
    };

    let requests = [
        EndpointInjectRequest {
            correlation_id: format!("doctor-key-press-{}", rshare_core::timestamp_ms()),
            device_kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "ShiftLeft".to_string(),
                state: "Pressed".to_string(),
            },
            mode: EndpointInjectMode::TestLoopback,
            timeout_ms: 1_000,
        },
        EndpointInjectRequest {
            correlation_id: format!("doctor-key-release-{}", rshare_core::timestamp_ms()),
            device_kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "ShiftLeft".to_string(),
                state: "Released".to_string(),
            },
            mode: EndpointInjectMode::TestLoopback,
            timeout_ms: 1_000,
        },
    ];

    let mut results = Vec::new();
    for request in requests {
        match daemon_client::request_endpoint_inject(EndpointInjectTarget::Remote(target), request)
            .await
        {
            Ok(result) => results.push(result),
            Err(error) => {
                eprintln!(
                    "{}",
                    format!("  [WARN] Remote inject request failed: {error}").yellow()
                );
            }
        }
    }
    results
}

fn build_doctor_checks(
    status: Option<&ServiceStatusSnapshot>,
    status_error: Option<&str>,
    devices: &[DaemonDeviceSnapshot],
    layout: Option<&LayoutGraph>,
    local_controls: Option<&LocalControlDeviceSnapshot>,
    capabilities: Option<&CapabilityRegistrySnapshot>,
    endpoint_events: &[EndpointEvent],
    inject_requested: bool,
    inject_results: &[EndpointInjectResult],
) -> Vec<DoctorCheck> {
    let daemon_online = status.is_some();
    let connected_count = devices.iter().filter(|device| device.connected).count();
    let remote_ids: Vec<DeviceId> = devices.iter().map(|device| device.id).collect();
    let remote_event_count = endpoint_events
        .iter()
        .filter(|event| remote_ids.contains(&event.endpoint_id))
        .count();
    let injected_remote_event_count = endpoint_events
        .iter()
        .filter(|event| {
            remote_ids.contains(&event.endpoint_id)
                && matches!(
                    event.direction,
                    EndpointEventDirection::Injected | EndpointEventDirection::InjectedLoopback
                )
        })
        .count();
    let inject_probe_passed = inject_requested
        && !inject_results.is_empty()
        && inject_results.iter().all(|result| result.accepted);
    let input_backend_ready = status
        .and_then(|snapshot| snapshot.backend_health.as_ref())
        .map_or(false, |health| matches!(health, BackendHealth::Healthy));
    let network_detail = status
        .map(|snapshot| {
            let rtt = snapshot
                .network
                .rtt_ms
                .map(|value| format!("{value} ms"))
                .unwrap_or_else(|| "-".to_string());
            let trust = snapshot
                .network
                .cert_trust_state
                .as_deref()
                .unwrap_or("unknown");
            format!(
                "transport={} datagram={} rtt={} dropped={} resets={} cert={}",
                snapshot.network.transport,
                if snapshot.network.datagram_available {
                    "yes"
                } else {
                    "no"
                },
                rtt,
                snapshot.network.datagram_tx_dropped,
                snapshot.network.reliable_stream_reset_count,
                trust
            )
        })
        .unwrap_or_else(|| "network status unavailable".to_string());
    let network_state = status.map_or(CheckState::Block, |snapshot| {
        if snapshot.network.transport == "quic"
            && snapshot.network.datagram_available
            && !snapshot.network.realtime_degraded
        {
            CheckState::Pass
        } else if daemon_online && snapshot.network.transport == "quic" {
            CheckState::Warn
        } else {
            CheckState::Block
        }
    });
    let local_capture_ready = local_controls.map_or(false, |snapshot| {
        snapshot.keyboard.detected
            || snapshot.mouse.detected
            || snapshot.gamepads.iter().any(|gamepad| gamepad.connected)
            || !snapshot.recent_events.is_empty()
    });
    let layout_nodes = layout.map_or(0, |layout| layout.nodes.len());
    let capability_registry_ready = capabilities.is_some();
    let local_capabilities = capabilities.and_then(|registry| {
        registry
            .devices
            .iter()
            .find(|device| device.device_id == registry.local_device_id)
    });
    let capability_input_ready = local_capabilities
        .and_then(|device| {
            device
                .capabilities
                .iter()
                .find(|capability| capability.kind == EndpointCapabilityKind::Input)
        })
        .map_or(false, |capability| {
            capability.state == CapabilityState::Available
        });
    let capability_diagnostics_ready = local_capabilities
        .and_then(|device| {
            device
                .capabilities
                .iter()
                .find(|capability| capability.kind == EndpointCapabilityKind::Diagnostics)
        })
        .map_or(false, |capability| {
            capability.state == CapabilityState::Available
        });
    let capability_usb_boundary_clear = local_capabilities.map_or(false, |device| {
        let usb_host = device
            .capabilities
            .iter()
            .find(|capability| capability.kind == EndpointCapabilityKind::UsbHost)
            .map(|capability| capability.state);
        let usb_receiver = device
            .capabilities
            .iter()
            .find(|capability| capability.kind == EndpointCapabilityKind::UsbReceiver)
            .map(|capability| capability.state);
        matches!(
            usb_host,
            Some(CapabilityState::Unavailable | CapabilityState::Experimental)
        ) && matches!(usb_receiver, Some(CapabilityState::Unavailable))
    });
    let capability_detail = capabilities
        .map(|registry| {
            format!(
                "devices={} input={} diagnostics={} usb_boundary={}",
                registry.devices.len(),
                capability_input_ready,
                capability_diagnostics_ready,
                capability_usb_boundary_clear
            )
        })
        .unwrap_or_else(|| "capability registry unavailable".to_string());

    let inject_success_count = inject_results
        .iter()
        .filter(|result| result.accepted)
        .count();
    let inject_detail = if inject_requested {
        if inject_results.is_empty() && connected_count == 0 {
            "未发现已连接远端，未执行注入探测".to_string()
        } else if inject_results.is_empty() {
            "远端注入探测未返回结果".to_string()
        } else {
            match inject_latency_summary(inject_results) {
                Some((average_ms, max_ms)) => format!(
                    "注入结果 {}/{} 成功，平均 {} ms，最大 {} ms",
                    inject_success_count,
                    inject_results.len(),
                    average_ms,
                    max_ms
                ),
                None => format!(
                    "注入结果 {}/{} 成功",
                    inject_success_count,
                    inject_results.len()
                ),
            }
        }
    } else {
        "未运行注入探测；使用 --inject 执行 Shift loopback".to_string()
    };

    vec![
        DoctorCheck {
            key: "ipc",
            label: "IPC",
            state: if daemon_online {
                CheckState::Pass
            } else {
                CheckState::Block
            },
            detail: status
                .map(|snapshot| format!("daemon PID {}", snapshot.pid))
                .unwrap_or_else(|| status_error.unwrap_or("daemon unavailable").to_string()),
        },
        DoctorCheck {
            key: "discovery",
            label: "局域网发现",
            state: if devices.is_empty() {
                if daemon_online {
                    CheckState::Warn
                } else {
                    CheckState::Block
                }
            } else {
                CheckState::Pass
            },
            detail: format!("已发现 {} 台，已连接 {} 台", devices.len(), connected_count),
        },
        DoctorCheck {
            key: "network",
            label: "QUIC 通道",
            state: network_state,
            detail: network_detail,
        },
        DoctorCheck {
            key: "capabilities",
            label: "能力注册",
            state: if capability_registry_ready
                && capability_input_ready
                && capability_diagnostics_ready
                && capability_usb_boundary_clear
            {
                CheckState::Pass
            } else if daemon_online && capability_registry_ready {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: capability_detail,
        },
        DoctorCheck {
            key: "layout",
            label: "布局图",
            state: if layout_nodes > 1 {
                CheckState::Pass
            } else if daemon_online {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: format!("LayoutGraph 节点 {} 个", layout_nodes),
        },
        DoctorCheck {
            key: "local-capture",
            label: "本机捕获",
            state: if local_capture_ready {
                CheckState::Pass
            } else if daemon_online {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: local_controls
                .map(|snapshot| {
                    format!(
                        "keyboard={} mouse={} recent_events={}",
                        snapshot.keyboard.detected,
                        snapshot.mouse.detected,
                        snapshot.recent_events.len()
                    )
                })
                .unwrap_or_else(|| "local controls unavailable".to_string()),
        },
        DoctorCheck {
            key: "input-backend",
            label: "输入后端",
            state: if input_backend_ready {
                CheckState::Pass
            } else if daemon_online {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: status
                .and_then(|snapshot| snapshot.backend_health.as_ref())
                .map(|health| format!("{health:?}"))
                .unwrap_or_else(|| "backend health unavailable".to_string()),
        },
        DoctorCheck {
            key: "remote-events",
            label: "远端事件镜像",
            state: if remote_event_count > 0 {
                CheckState::Pass
            } else if connected_count > 0 {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: format!("远端事件 {} 条", remote_event_count),
        },
        DoctorCheck {
            key: "remote-inject",
            label: "远端注入",
            state: if inject_probe_passed || injected_remote_event_count > 0 {
                CheckState::Pass
            } else if inject_requested {
                CheckState::Block
            } else if connected_count > 0 {
                CheckState::Warn
            } else {
                CheckState::Block
            },
            detail: inject_detail,
        },
    ]
}

fn inject_latency_summary(results: &[EndpointInjectResult]) -> Option<(u64, u64)> {
    if results.is_empty() {
        return None;
    }

    let total = results
        .iter()
        .map(|result| result.elapsed_ms as u128)
        .sum::<u128>();
    let average = (total / results.len() as u128) as u64;
    let max = results
        .iter()
        .map(|result| result.elapsed_ms)
        .max()
        .unwrap_or_default();
    Some((average, max))
}

fn print_check(check: &DoctorCheck) {
    let state = match check.state {
        CheckState::Pass => "OK".green(),
        CheckState::Warn => "WARN".yellow(),
        CheckState::Block => "BLOCK".red(),
    };
    println!(
        "  [{}] {:<12} {}",
        state,
        check.label,
        check.detail.replace('\n', " ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rshare_core::{EndpointDeviceRef, EndpointEventSource};
    use uuid::Uuid;

    fn status(id: DeviceId) -> ServiceStatusSnapshot {
        let mut status = ServiceStatusSnapshot::new(
            id,
            "local-R-ShareMouse".to_string(),
            "local".to_string(),
            "0.0.0.0:27431".to_string(),
            27432,
            42,
        );
        status.backend_health = Some(BackendHealth::Healthy);
        status.network.datagram_available = true;
        status.network.realtime_degraded = false;
        status.network.rtt_ms = Some(2);
        status.network.cert_trust_state = Some("trusted".to_string());
        status
    }

    fn device(id: DeviceId, connected: bool) -> DaemonDeviceSnapshot {
        DaemonDeviceSnapshot {
            id,
            name: "remote-R-ShareMouse".to_string(),
            hostname: "remote".to_string(),
            addresses: vec!["192.168.1.10".to_string()],
            connected,
            last_seen_secs: Some(1),
        }
    }

    fn endpoint_event(endpoint_id: DeviceId, direction: EndpointEventDirection) -> EndpointEvent {
        EndpointEvent {
            event_id: 1,
            sequence: 1,
            timestamp_ms: 1,
            endpoint_id,
            origin_endpoint_id: endpoint_id,
            device: EndpointDeviceRef {
                device_id: "keyboard".to_string(),
                instance_id: None,
                display_name: "Keyboard".to_string(),
                kind: EndpointEventKind::Keyboard,
                attribution: rshare_core::DeviceAttribution::Aggregate,
            },
            direction,
            source: EndpointEventSource::RemoteMirror,
            kind: EndpointEventKind::Keyboard,
            payload: EndpointEventPayload::Keyboard {
                key: "A".to_string(),
                state: "Pressed".to_string(),
            },
            correlation_id: None,
        }
    }

    fn endpoint_inject_result(target: DeviceId, elapsed_ms: u64) -> EndpointInjectResult {
        EndpointInjectResult {
            correlation_id: format!("test-{elapsed_ms}"),
            target: EndpointInjectTarget::Remote(target),
            accepted: true,
            backend_kind: Some(rshare_core::BackendKind::Portable),
            health: BackendHealth::Healthy,
            elapsed_ms,
            loopback_event_id: Some(elapsed_ms),
            error: None,
        }
    }

    fn capabilities(local: DeviceId) -> CapabilityRegistrySnapshot {
        CapabilityRegistrySnapshot {
            local_device_id: local,
            generated_at_ms: 1,
            devices: vec![rshare_core::DeviceCapabilitySnapshot {
                device_id: local,
                device_name: "local-R-ShareMouse".to_string(),
                hostname: "local".to_string(),
                connected: true,
                capabilities: vec![
                    rshare_core::EndpointCapabilitySnapshot::new(
                        EndpointCapabilityKind::Input,
                        CapabilityState::Available,
                    ),
                    rshare_core::EndpointCapabilitySnapshot::new(
                        EndpointCapabilityKind::Diagnostics,
                        CapabilityState::Available,
                    ),
                    rshare_core::EndpointCapabilitySnapshot::new(
                        EndpointCapabilityKind::UsbHost,
                        CapabilityState::Unavailable,
                    ),
                    rshare_core::EndpointCapabilitySnapshot::new(
                        EndpointCapabilityKind::UsbReceiver,
                        CapabilityState::Unavailable,
                    ),
                ],
            }],
        }
    }

    #[test]
    fn doctor_checks_pass_when_remote_events_and_inject_loopback_exist() {
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let mut layout = LayoutGraph::new(local);
        layout.add_node(rshare_core::LayoutNode::new(local, 0, 0, 1920, 1080));
        layout.add_node(rshare_core::LayoutNode::new(remote, 1920, 0, 1920, 1080));
        let mut controls = LocalControlDeviceSnapshot::default();
        controls.keyboard.detected = true;

        let checks = build_doctor_checks(
            Some(&status(local)),
            None,
            &[device(remote, true)],
            Some(&layout),
            Some(&controls),
            Some(&capabilities(local)),
            &[
                endpoint_event(remote, EndpointEventDirection::Observed),
                endpoint_event(remote, EndpointEventDirection::InjectedLoopback),
            ],
            false,
            &[],
        );

        assert_eq!(
            checks
                .iter()
                .map(|check| (check.key, check.state))
                .collect::<Vec<_>>(),
            vec![
                ("ipc", CheckState::Pass),
                ("discovery", CheckState::Pass),
                ("network", CheckState::Pass),
                ("capabilities", CheckState::Pass),
                ("layout", CheckState::Pass),
                ("local-capture", CheckState::Pass),
                ("input-backend", CheckState::Pass),
                ("remote-events", CheckState::Pass),
                ("remote-inject", CheckState::Pass),
            ],
        );
    }

    #[test]
    fn doctor_checks_warn_for_connected_remote_without_events() {
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let checks = build_doctor_checks(
            Some(&status(local)),
            None,
            &[device(remote, true)],
            None,
            Some(&LocalControlDeviceSnapshot::default()),
            Some(&capabilities(local)),
            &[],
            false,
            &[],
        );

        assert_eq!(
            checks
                .iter()
                .filter(|check| matches!(check.key, "remote-events" | "remote-inject"))
                .map(|check| check.state)
                .collect::<Vec<_>>(),
            vec![CheckState::Warn, CheckState::Warn],
        );
    }

    #[test]
    fn doctor_checks_report_remote_inject_latency_summary() {
        let local = Uuid::new_v4();
        let remote = Uuid::new_v4();
        let checks = build_doctor_checks(
            Some(&status(local)),
            None,
            &[device(remote, true)],
            None,
            Some(&LocalControlDeviceSnapshot::default()),
            Some(&capabilities(local)),
            &[],
            true,
            &[
                endpoint_inject_result(remote, 12),
                endpoint_inject_result(remote, 18),
            ],
        );

        let remote_inject = checks
            .iter()
            .find(|check| check.key == "remote-inject")
            .expect("remote inject check");
        assert_eq!(remote_inject.state, CheckState::Pass);
        assert!(remote_inject.detail.contains("2/2 成功"));
        assert!(remote_inject.detail.contains("平均 15 ms"));
        assert!(remote_inject.detail.contains("最大 18 ms"));
    }

    #[test]
    fn doctor_checks_explain_missing_remote_inject_probe() {
        let local = Uuid::new_v4();
        let checks = build_doctor_checks(
            Some(&status(local)),
            None,
            &[],
            None,
            Some(&LocalControlDeviceSnapshot::default()),
            Some(&capabilities(local)),
            &[],
            true,
            &[],
        );

        let remote_inject = checks
            .iter()
            .find(|check| check.key == "remote-inject")
            .expect("remote inject check");
        assert_eq!(remote_inject.state, CheckState::Block);
        assert_eq!(remote_inject.detail, "未发现已连接远端，未执行注入探测");
    }
}
